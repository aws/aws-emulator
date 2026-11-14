// SPDX-License-Identifier: CC0-1.0

//! The VERSIONS of an IoT policy: how a version is added, how many a policy holds,
//! and what a reader of them reports.
//!
//! A policy is a name plus a set of numbered document versions and a pointer to the
//! one in effect ([`super::RegisteredPolicy`]). This is the half of that record the
//! version operations write and read: `CreatePolicyVersion` adds one and may re-point
//! the pointer, `ListPolicyVersions` reports them, `GetPolicyVersion` reports one,
//! `SetDefaultPolicyVersion` re-points the pointer and `DeletePolicyVersion` removes
//! one that is not in effect.
//!
//! How MANY versions a policy holds is CAPPED, which is what makes
//! `CreatePolicyVersion`'s modelled `VersionsLimitExceededException` reachable: past
//! the cap nothing is written and the create is refused, so a policy's versions never
//! outnumber [`MAX_POLICY_VERSIONS`] however many callers add them at once.

use std::time::SystemTime;

use crate::ops::types::PolicyVersion as ModeledPolicyVersion;

/// How many versions one IoT policy holds.
///
/// AWS publishes the quota as five versions of a policy, which is the cap
/// `CreatePolicyVersion`'s modelled `VersionsLimitExceededException` ("The number of
/// policy versions exceeds the limit", HTTP 409) is refused past
/// (`smithy_query(query="operation",operation="CreatePolicyVersion")`): a policy at
/// the cap takes no further version until one is deleted.
pub const MAX_POLICY_VERSIONS: usize = 5;

/// How this service refuses a policy that already holds [`MAX_POLICY_VERSIONS`]
/// versions.
///
/// The fixed text is the class's, and the variable parts are the policy the request
/// named and the cap it is at.
pub fn policy_versions_limit_message(policy_name: &str) -> String {
    format!(
        "The policy {policy_name} already has the maximum number of versions \
         ({MAX_POLICY_VERSIONS})"
    )
}

/// How a reader of ONE version refuses a `policyVersionId` the policy does not hold.
pub fn missing_policy_version_message(policy_name: &str, policy_version_id: &str) -> String {
    format!("Version {policy_version_id} of policy {policy_name} does not exist")
}

/// How a reader of the policy's DEFAULT version refuses a name the registry does not
/// hold.
///
/// `GetPolicy` reports "the policy document of the default version"
/// (`smithy_query(query="doc",shape="GetPolicy")`), so it resolves the requested name
/// through that version — and AWS words its miss after the version it could not find
/// rather than after the name it was given, which is why this sentence is entirely fixed
/// text with no variable part, unlike [`super::missing_policy_message`], which every
/// operation addressing the policy RECORD states a miss with.
pub fn missing_policy_default_version_message() -> String {
    "Policy default version not found".to_string()
}

impl super::RegisteredPolicy {
    /// Add `document` as a new version, making it the default when the request asked
    /// for that, and answer the id it was given — or `None` when the policy is
    /// already at [`MAX_POLICY_VERSIONS`], in which case nothing is written.
    ///
    /// The id is one past the highest the policy has ever held, so an id is never
    /// reused by a later version: a caller that deleted version 2 and adds another
    /// gets 3, not 2. The cap is read under the same lock as the write, so two
    /// concurrent creates cannot both find room for the same slot.
    pub fn add_version(
        &mut self,
        document: String,
        set_as_default: bool,
        now: SystemTime,
    ) -> Option<u32> {
        if self.versions.len() >= MAX_POLICY_VERSIONS {
            return None;
        }
        let version_id = self
            .versions
            .keys()
            .next_back()
            .map_or(super::FIRST_POLICY_VERSION_ID, |highest| highest + 1);
        self.versions.insert(
            version_id,
            super::PolicyVersion {
                document,
                created_at: now,
            },
        );
        if set_as_default {
            self.default_version_id = version_id;
        }
        self.last_modified_date = now;
        self.generation_id = super::fresh_uuid();
        Some(version_id)
    }

    /// The document of the version `policy_version_id`, or `None` for an id the
    /// policy does not hold.
    ///
    /// The id is the modelled `PolicyVersionId` (`^[0-9]+$`), so anything that is not
    /// a number names no version of any policy.
    pub fn version(&self, policy_version_id: &str) -> Option<&super::PolicyVersion> {
        let version_id = policy_version_id.parse::<u32>().ok()?;
        self.versions.get(&version_id)
    }

    /// Whether `policy_version_id` names the version in effect.
    pub fn is_default_version(&self, policy_version_id: &str) -> bool {
        policy_version_id
            .parse::<u32>()
            .is_ok_and(|version_id| version_id == self.default_version_id)
    }

    /// Every version as `ListPolicyVersions` reports them, oldest-first — the order
    /// the records are kept in, so a reader does not sort.
    pub fn version_summaries(&self) -> Vec<ModeledPolicyVersion> {
        self.versions
            .iter()
            .map(|(version_id, version)| ModeledPolicyVersion {
                version_id: Some(version_id.to_string()),
                is_default_version: *version_id == self.default_version_id,
                create_date: Some(::aws_smithy_types::DateTime::from(version.created_at)),
            })
            .collect()
    }
}
