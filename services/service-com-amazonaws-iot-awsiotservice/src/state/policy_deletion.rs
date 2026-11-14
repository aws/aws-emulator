// SPDX-License-Identifier: CC0-1.0

//! What holds the DELETE of an IoT policy back, and what removing one does to the
//! registry every other policy operation reads.
//!
//! `DeletePolicy` documents the rule in one sentence: "A policy cannot be deleted if it
//! has non-default versions or it is attached to any certificate. To delete a policy,
//! use the DeletePolicyVersion action to delete all non-default versions of the policy;
//! use the DetachPolicy action to detach the policy from any certificate; and then use
//! the DeletePolicy action to delete the policy. When a policy is deleted using
//! DeletePolicy, its default version is deleted with it"
//! (`smithy_query(query="doc",shape="DeletePolicy")`). So the delete reads two relations
//! this service already holds — the policy's own versions
//! ([`super::RegisteredPolicy::versions`]) and the policy-attachment relation
//! ([`super::PolicyAttachments`]) — and removes the record only when neither holds it
//! back.
//!
//! The guard lives here, beside the records it reads, rather than in the handler, so the
//! one rule is stated once: whatever grew the relation — `AttachPolicy`,
//! `AttachPrincipalPolicy` — and whatever target form it recorded, a policy that any
//! target still names is a policy that is not deleted.

/// Why a policy was NOT deleted.
///
/// Each variant names the modelled error class the refusal answers with
/// (`smithy_query(query="operation",operation="DeletePolicy")`), so the handler chooses
/// the status from the state rather than restating the rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDeletionRefusal {
    /// A name the account holds no policy under: `ResourceNotFoundException` (HTTP 404).
    NotFound(String),
    /// A policy at least one target still names: `DeleteConflictException` (HTTP 409).
    AttachedPrincipals(String),
    /// A policy that still holds a version other than the one in effect:
    /// `DeleteConflictException` (HTTP 409).
    NonDefaultVersions(String),
}

impl PolicyDeletionRefusal {
    /// The message the refusal states, whichever class it names.
    pub fn message(&self) -> &str {
        match self {
            Self::NotFound(message)
            | Self::AttachedPrincipals(message)
            | Self::NonDefaultVersions(message) => message,
        }
    }
}

/// The message a delete refuses a policy a principal is still attached to with.
///
/// AWS IoT words a policy delete conflict after the relation that holds the policy back
/// and names the policy by the `name=` member the request carried, because what the
/// caller has to call next — `DetachPolicy` — takes that same name. The fixed text is
/// everything outside the name; the name is the request's `policyName`.
pub fn attached_principals_deletion_message(policy_name: &str) -> String {
    format!(
        "The policy cannot be deleted as the policy is attached to one or more principals \
         (name={policy_name})"
    )
}

/// The message a delete refuses a policy that still holds a non-default version with.
///
/// The second half of the rule `DeletePolicy` documents, worded in the same shape as
/// [`attached_principals_deletion_message`] — the relation that holds the policy back,
/// then the policy by the `name=` member the request carried — because the caller's next
/// step is the same kind of one, `DeletePolicyVersion` on that name. AWS publishes the
/// rule and not the sentence it states it in, and no recorded call reaches this branch,
/// so the fixed text here is this emulator's rendering of the documented rule.
pub fn non_default_versions_deletion_message(policy_name: &str) -> String {
    format!(
        "The policy cannot be deleted as the policy has one or more non-default versions \
         (name={policy_name})"
    )
}

impl super::ComAmazonawsIotAwsiotserviceState {
    /// Whether anything in this account's state holds the delete of `policy_name` back.
    ///
    /// Answers `None` when the policy may be deleted, so a reader can state the guard
    /// without removing anything.
    pub fn policy_deletion_refusal(&self, policy_name: &str) -> Option<PolicyDeletionRefusal> {
        let Some(policy) = self.policies.get(policy_name) else {
            return Some(PolicyDeletionRefusal::NotFound(
                super::missing_policy_message(policy_name),
            ));
        };

        // "it is attached to any certificate": read as the relation itself, which records
        // every form of policy target the model documents, so a policy a thing group or a
        // Cognito identity still names is held back for the same reason a certificate
        // holds it back — the recorded refusal names "one or more principals" rather than
        // one resource type. Read FIRST, because it is the half of the rule a caller
        // reaches by attaching, and the refusal is idempotent: it holds for as long as
        // ANY target remains, so detaching one of several changes nothing.
        if !self.policy_attachments.targets_of(policy_name).is_empty() {
            return Some(PolicyDeletionRefusal::AttachedPrincipals(
                attached_principals_deletion_message(policy_name),
            ));
        }

        // "if it has non-default versions": the delete takes the default version with it,
        // so a policy still holding any other version is held back until those are
        // deleted.
        if policy
            .versions
            .keys()
            .any(|version_id| *version_id != policy.default_version_id)
        {
            return Some(PolicyDeletionRefusal::NonDefaultVersions(
                non_default_versions_deletion_message(policy_name),
            ));
        }

        None
    }

    /// Delete the policy recorded under `policy_name`, answering the record that was
    /// removed, or the reason this account's state held it back.
    ///
    /// The guard is read under the same lock as the write, so a policy attached
    /// concurrently cannot be deleted between the check and the removal. The name is the
    /// policy's identity and its key in the registry, so removing that one record is the
    /// whole write — "its default version is deleted with it" — and every later reader of
    /// the registry stops reporting it while the name is free for a later create.
    pub fn delete_policy(
        &mut self,
        policy_name: &str,
    ) -> Result<super::RegisteredPolicy, PolicyDeletionRefusal> {
        if let Some(refusal) = self.policy_deletion_refusal(policy_name) {
            return Err(refusal);
        }
        self.policies.remove(policy_name).ok_or_else(|| {
            PolicyDeletionRefusal::NotFound(super::missing_policy_message(policy_name))
        })
    }
}
