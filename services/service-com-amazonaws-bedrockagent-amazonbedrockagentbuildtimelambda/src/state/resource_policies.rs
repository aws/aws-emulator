// SPDX-License-Identifier: CC0-1.0

//! The two readings a knowledge base's resource-based POLICY turns on: which
//! knowledge base a resource-policy operation's `resourceArn` names, and which
//! KIND of knowledge base carries a policy at all.
//!
//! `PutResourcePolicy` "associates a resource policy with a knowledge base. A
//! resource policy allows other AWS accounts to access the knowledge base", and it
//! does so only for a knowledge base Amazon Bedrock MANAGES
//! ([`MANAGED_KNOWLEDGE_BASE_TYPE`]): the recorded refusals of both
//! `PutResourcePolicy` and `GetResourcePolicy` against a knowledge base of another
//! kind are `ValidationException`s, so for every other `KnowledgeBaseType` a
//! resource policy cannot exist and naming one is a bad request rather than a
//! policy that is merely absent.
//!
//! The two readings live here, out of the handlers and out of this module root,
//! because they are what every resource-policy operation agrees on: an ARN
//! resolves through this plane's one ARN reader, and the KIND test is a single
//! sentence the write and the read have to make the same way. Both are now folded
//! into ONE resolution, [`ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::policy_bearing_knowledge_base`],
//! which answers either the knowledge base whose policy the call is about or the
//! reason there is none to be about — so `GetResourcePolicy`, the write beside it
//! and the delete that removes a policy cannot disagree on which ARN names what.
//!
//! What a write may ATTACH is here too ([`resource_policy_document_problem`]): the
//! document's own two clauses, read before the store is touched, so a read never
//! reports a document some write should not have stored.
//!
//! The policy itself is held on the knowledge base's own record
//! ([`KnowledgeBaseRecord::resource_policy`]), as one [`ResourcePolicyRecord`]: the
//! document a write stored — `None` once a removal took it off again — and the
//! revision that write left the policy at.

use super::*;

/// The knowledge base a resource-policy operation's `resourceArn` names, or `None`
/// when the value names no knowledge base of this plane.
///
/// The ARN is resolved through the one reader of this plane's own ARNs
/// ([`taggable_resource`]), so `arn:…:knowledge-base/<id>` selects the very record
/// the knowledge base's own reads report, and an ARN of another of this plane's
/// resources — an agent, a flow — names no knowledge base to carry a policy.
/// Whether that knowledge base EXISTS is a different question: an ARN of the right
/// syntax naming nothing stored is a missing resource.
pub fn policied_knowledge_base(resource_arn: &str) -> Option<String> {
    match taggable_resource(resource_arn)? {
        TaggableResource::KnowledgeBase(knowledge_base_id) => Some(knowledge_base_id),
        _ => None,
    }
}

/// Whether a knowledge base of this `knowledgeBaseConfiguration.type` carries a
/// resource-based policy at all: the service-managed kind, and no other (see the
/// module docs).
pub fn carries_resource_policy(knowledge_base_type: &str) -> bool {
    knowledge_base_type == MANAGED_KNOWLEDGE_BASE_TYPE
}

/// One knowledge base's resource-based policy: the document the last write left
/// attached, and the revision that write left the policy at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourcePolicyRecord {
    /// The policy document, held exactly as the write supplied it — the modelled
    /// `ResourcePolicy` is a string, so a read reports the same bytes back — or
    /// `None` when the last write was the REMOVAL that took the document off, which
    /// leaves the knowledge base carrying no policy at a revision of its own.
    pub document: Option<String>,
    /// How many writes this knowledge base's policy has taken — every attach and
    /// every removal — which is the revision it is now at ([`Self::revision_id`] is
    /// how the wire carries it).
    pub revision: u64,
}

impl ResourcePolicyRecord {
    /// This revision as the modelled `revisionId`.
    ///
    /// The shape states only a length (1..255), and what the operations state the
    /// value is FOR settles its form: `PutResourcePolicy$expectedRevisionId` and
    /// `DeleteResourcePolicy$expectedRevisionId` are "the revisionId from the most
    /// recent GetResourcePolicy or PutResourcePolicy response", used "to prevent
    /// conflicts when multiple users update the same policy concurrently" — so the
    /// value has to change on every write and be comparable by equality, and
    /// nothing else about it is observable. It therefore COUNTS this policy's
    /// writes rather than being drawn at random: it is derived from stored state,
    /// stable across reads of an unchanged policy, and different after each change.
    pub fn revision_id(&self) -> String {
        self.revision.to_string()
    }
}

impl KnowledgeBaseRecord {
    /// Attach `policy` to this knowledge base, answering the `revisionId` the write
    /// left the policy at.
    ///
    /// The one writer of [`Self::resource_policy`], so every write advances the
    /// revision the same way: a first policy is revision 1 and each write past it
    /// counts one more.
    pub fn attach_resource_policy(&mut self, policy: String) -> String {
        let revision = self.next_resource_policy_revision();
        let record = ResourcePolicyRecord {
            document: Some(policy),
            revision,
        };
        let revision_id = record.revision_id();
        self.resource_policy = Some(record);
        revision_id
    }

    /// Remove the policy document this knowledge base carries, answering the
    /// `revisionId` the removal left the policy at — or `None` when it carries no
    /// document, so there is nothing to remove.
    ///
    /// A removal is a WRITE of [`Self::resource_policy`], so it advances the revision
    /// exactly as an attach does: `DeleteResourcePolicy$revisionId` is "the revision
    /// identifier after the resource policy was deleted", and keeping the count across
    /// the removal is what stops a later attach from re-issuing a revision an earlier
    /// write already answered — the value is compared by equality to guard concurrent
    /// updates, so no two writes of one knowledge base's policy may share one.
    pub fn remove_resource_policy(&mut self) -> Option<String> {
        self.attached_resource_policy()?;
        let revision = self.next_resource_policy_revision();
        let record = ResourcePolicyRecord {
            document: None,
            revision,
        };
        let revision_id = record.revision_id();
        self.resource_policy = Some(record);
        Some(revision_id)
    }

    /// The revision the NEXT write of this knowledge base's policy leaves it at: one
    /// past the revision the last write left, and 1 when none has written yet.
    fn next_resource_policy_revision(&self) -> u64 {
        self.resource_policy
            .as_ref()
            .map_or(0, |stored| stored.revision)
            .saturating_add(1)
    }

    /// The resource policy this knowledge base carries NOW — the document attached and
    /// the revision the write that attached it left the policy at — or `None` when it
    /// carries none, whether because nothing ever wrote one or because a removal took
    /// the document off.
    ///
    /// The one reading of "does this knowledge base have a policy", so the read, the
    /// write and the delete cannot come to answer it differently.
    pub fn attached_resource_policy(&self) -> Option<(&str, String)> {
        let stored = self.resource_policy.as_ref()?;
        let document = stored.document.as_deref()?;
        Some((document, stored.revision_id()))
    }

    /// Whether `expected` names the revision this knowledge base's policy is at NOW.
    ///
    /// The caller's precondition, which `PutResourcePolicy$expectedRevisionId` and
    /// `DeleteResourcePolicy$expectedRevisionId` both state as "the revisionId from the
    /// most recent GetResourcePolicy or PutResourcePolicy response", carried "to prevent
    /// conflicts when multiple users update the same policy concurrently" — so it holds
    /// exactly when the value equals the revision the store holds, compared by equality.
    /// A knowledge base carrying NO policy is at no revision, so no value names its
    /// revision: a precondition on a policy that is not there cannot hold.
    pub fn resource_policy_revision_matches(&self, expected: &str) -> bool {
        self.attached_resource_policy()
            .is_some_and(|(_, revision_id)| revision_id == expected)
    }
}

/// Why a resource-policy operation has no knowledge-base policy to act on.
///
/// The three readings are ordered as the questions nest: a value that names no
/// knowledge base of this plane is answered before any store is consulted, a
/// knowledge base nothing holds before its kind is read, and the kind last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourcePolicyRefusal {
    /// The `resourceArn` names no knowledge base of this plane — a bad request,
    /// however well-formed the value is as some other ARN.
    NotAKnowledgeBase,
    /// The ARN names a knowledge base this store does not hold (or holds only on
    /// its way out) — a missing resource, carrying the id the ARN named.
    NoSuchKnowledgeBase(String),
    /// The knowledge base is there, but its KIND carries no resource policy at all,
    /// so there is none to read or write: a bad request.
    KindCarriesNoPolicy,
}

impl ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState {
    /// The knowledge base whose resource-based policy a call about `resource_arn` is
    /// about, or why there is no such policy.
    ///
    /// The ONE resolution every resource-policy operation shares, so the read, the
    /// write and the delete answer one ARN alike.
    pub fn policy_bearing_knowledge_base(
        &self,
        resource_arn: &str,
    ) -> Result<&KnowledgeBaseRecord, ResourcePolicyRefusal> {
        let knowledge_base_id = policied_knowledge_base(resource_arn)
            .ok_or(ResourcePolicyRefusal::NotAKnowledgeBase)?;
        let record = self.live_knowledge_base(&knowledge_base_id).ok_or(
            ResourcePolicyRefusal::NoSuchKnowledgeBase(knowledge_base_id.clone()),
        )?;
        let knowledge_base_type = record.knowledge_base_configuration.r#type.as_str();
        if !carries_resource_policy(knowledge_base_type) {
            return Err(ResourcePolicyRefusal::KindCarriesNoPolicy);
        }
        Ok(record)
    }
}

/// The modelled `ResourceArn` pattern, as the model writes it. A refusal quotes it
/// through [`stated_pattern`], which drops the whole-value anchors.
pub const RESOURCE_POLICY_ARN_PATTERN: &str =
    "^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:knowledge-base/[0-9a-zA-Z]+$";

/// The `ValidationException` message a resource-policy operation answers for a
/// `resourceArn` that names no knowledge base of this plane: the member and the
/// constraint the value failed, with the value from the request.
///
/// The same counted-constraint frame this service answers for every other member
/// whose value is outside the syntax its shape spells (see
/// [`taggable_resource_arn_pattern_message`] and [`flow_identifier_pattern_message`]).
pub fn resource_policy_arn_pattern_message(resource_arn: &str) -> String {
    format!(
        "Value '{resource_arn}' at 'resourceArn' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {}",
        stated_pattern(RESOURCE_POLICY_ARN_PATTERN)
    )
}

/// The `ValidationException` message a resource-policy operation answers when the
/// knowledge base it names is of a kind that carries no resource policy.
///
/// AWS's wording for this error class is ENTIRELY fixed text: it names the one kind
/// that does carry a policy rather than the kind the request reached, so no part of
/// it comes from the request. The kind it names is the one the reading above admits
/// ([`MANAGED_KNOWLEDGE_BASE_TYPE`]), so the sentence and the rule it refuses cannot
/// drift apart.
pub fn unsupported_resource_policy_message() -> String {
    format!(
        "Resource policies are only supported for {MANAGED_KNOWLEDGE_BASE_TYPE} \
         type knowledge bases."
    )
}

/// Why this service will not attach a resource policy document a write carried.
///
/// Both readings are of the DOCUMENT alone — neither consults the store — and both are
/// bad requests the modelled `ValidationException` refuses, so a write judges them
/// before it touches the knowledge base it would attach to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourcePolicyDocumentProblem {
    /// The value is not a resource-based policy document at all, carrying the clause of
    /// the IAM JSON policy language it breaks.
    NotAPolicyDocument(::service_common::iam_policy::PolicyDefect),
    /// The document is a policy, and its grants reach a principal nobody named.
    GrantsPublicAccess,
}

impl ResourcePolicyDocumentProblem {
    /// The `ValidationException` message this problem is refused with.
    pub fn message(&self) -> String {
        match self {
            Self::NotAPolicyDocument(defect) => {
                format!("The resource policy is not a valid policy document: {defect}.")
            }
            Self::GrantsPublicAccess => PUBLIC_RESOURCE_POLICY_MESSAGE.to_string(),
        }
    }
}

/// The sentence a write is refused with when the document it carried would open the
/// knowledge base to every caller.
///
/// Entirely FIXED text: it states what was refused about the document and the two ways
/// the IAM policy language gives to fix it, and names nothing of the request, because
/// the document the caller sent is what the caller already holds.
pub const PUBLIC_RESOURCE_POLICY_MESSAGE: &str =
    "Resource policies must not grant access to all principals. Replace any wildcard \
     principal with specific principals, or add a condition that limits the callers to \
     specific AWS accounts.";

/// Why the document `policy` cannot be attached to a knowledge base, or `None` for one
/// this service stores.
///
/// Two readings, in the order a caller earns each refusal, both taken off the document
/// parsed ONCE:
///
/// * the value is a resource-based policy document — the member is modelled
///   `mediaType("application/json")` and documented as "the JSON-formatted resource
///   policy", so a text outside the IAM JSON policy language describes no grant this
///   service could enforce. The grammar is the workspace's ONE reading of a
///   resource-based policy ([`::service_common::iam_policy::resource_policy_defect_of`]),
///   so two services handed one document cannot disagree about whether it is a policy;
/// * its grants reach only principals it NAMES. A resource policy of a knowledge base
///   is what "allows other AWS accounts to access the knowledge base", so a document
///   granting to a principal nobody named would open it to every caller instead — read
///   through the workspace's one reading of that question
///   ([`::service_common::public_resource_policy::grants_public_access_of`]).
///
/// Nothing is written by judging, so a refused write leaves the knowledge base the
/// policy it had. A document's SIZE is the member's own `@length`, which the boundary
/// walk refuses before any handler runs.
pub fn resource_policy_document_problem(policy: &str) -> Option<ResourcePolicyDocumentProblem> {
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(policy) else {
        return Some(ResourcePolicyDocumentProblem::NotAPolicyDocument(
            ::service_common::iam_policy::PolicyDefect::NotJson,
        ));
    };
    if let Some(defect) = ::service_common::iam_policy::resource_policy_defect_of(&document) {
        return Some(ResourcePolicyDocumentProblem::NotAPolicyDocument(defect));
    }
    if ::service_common::public_resource_policy::grants_public_access_of(&document) {
        return Some(ResourcePolicyDocumentProblem::GrantsPublicAccess);
    }
    None
}

/// The `ResourceNotFoundException` message a resource-policy operation answers when
/// the knowledge base it names carries no policy: this service's own missing-resource
/// sentence, with the resource type that could not be retrieved being the policy
/// itself rather than the knowledge base, which is there.
///
/// The recorded refusals of this surface cover the knowledge base that is not there
/// ([`policied_knowledge_base_not_found_message`]) and the kind that carries no policy
/// ([`unsupported_resource_policy_message`]); for the knowledge base that is there
/// with no policy on it, this service's own sentence for a resource it could not
/// retrieve is the one answered, through the one helper that states it.
pub fn resource_policy_not_found_message(operation: &str) -> String {
    resource_not_found_message(operation, "ResourcePolicy")
}

/// The `ResourceNotFoundException` message a resource-policy operation answers when
/// the ARN names a knowledge base this store does not hold.
///
/// AWS's wording for this error class, which is fixed text but for the knowledge
/// base's id — the one the `resourceArn` named, taken from the request. It is this
/// plane's resource-policy sentence rather than [`resource_not_found_message`], which
/// the operations addressed by a `knowledgeBaseId` answer.
pub fn policied_knowledge_base_not_found_message(knowledge_base_id: &str) -> String {
    identified_resource_not_found_message("KnowledgeBase", knowledge_base_id)
}
