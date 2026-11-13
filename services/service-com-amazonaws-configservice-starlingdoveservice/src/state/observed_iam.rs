// SPDX-License-Identifier: CC0-1.0

//! What a recording pass OBSERVES in IAM: the account's IAM roles, the account's
//! customer managed policies, and the account's own password policy.
//!
//! IAM is a GLOBAL service, so what is read here is read through IAM's own API over the
//! in-process callback, exactly as the Amazon S3 and Amazon SQS observations beside it are
//! (`super::recording`): this crate names no other service crate, and a resource or a
//! setting IAM does not report is not recorded at all rather than recorded as something
//! else.
//!
//! Three readers live here because each puts its question to ONE service:
//!
//! * [`observed_iam_roles`] is the recorder's observation of a resource TYPE, answering
//!   the configuration items a pass records for this account's roles;
//! * [`observed_iam_policies`] is the same for the account's customer managed policies,
//!   each with the entities it is attached to as the item's RELATIONSHIPS;
//! * [`observed_account`] is the ACCOUNT's own configuration, which an account-wide
//!   managed rule is evaluated against (`super::account_rule`) rather than recorded as a
//!   resource — Config evaluates such a rule against the account pseudo-resource, and the
//!   account is not a resource any recorder discovers.

use std::collections::BTreeMap;
use std::time::SystemTime;

use super::{DiscoveredResourceEntry, RecordedRelationship};

/// The resource type Config records an IAM role under, a
/// `com.amazonaws.configservice#ResourceType` value.
pub const IAM_ROLE_RESOURCE_TYPE: &str = "AWS::IAM::Role";

/// The resource type Config records an IAM customer managed policy under, a
/// `com.amazonaws.configservice#ResourceType` value.
pub const IAM_POLICY_RESOURCE_TYPE: &str = "AWS::IAM::Policy";

/// The resource types this emulator can observe that belong to a GLOBAL service — the
/// ones a recording group covers only when it asks for the global resource types
/// ([`records_global_resource_types`]).
///
/// Source: the `AWS::IAM::*` and other non-regional entries of
/// `com.amazonaws.configservice#ResourceType`, of which this emulator observes the roles
/// and the customer managed policies today; a global type a later pass learns to observe
/// belongs here, so the gate below stays one rule.
pub const GLOBAL_RESOURCE_TYPES: [&str; 2] = [IAM_ROLE_RESOURCE_TYPE, IAM_POLICY_RESOURCE_TYPE];

/// Whether `resource_type` belongs to a global service, so that recording it is governed
/// by `RecordingGroup$includeGlobalResourceTypes` ("Specifies whether Config records
/// configuration changes for all supported global resources").
pub fn is_global_resource_type(resource_type: &str) -> bool {
    GLOBAL_RESOURCE_TYPES.contains(&resource_type)
}

/// Whether `group` covers `resource_type` as far as its being GLOBAL decides.
///
/// A group that records every supported type covers the global ones only when it asks for
/// them: "Default is to record all supported resource types, excluding the global IAM
/// resource types" (`PutConfigurationRecorder`), and
/// `RecordingGroup$includeGlobalResourceTypes` is the member that asks
/// ("`includeGlobalResourceTypes` and the resource exclusions in `recordingGroup` are not
/// compatible", so the member governs the all-supported strategy alone). A group that
/// ENUMERATES the types it records, or that records every type bar the ones it excludes,
/// has already said what it covers — [`super::group_records_resource_type`] is what reads
/// that — so being global changes nothing for it.
pub fn group_records_global_resource_type(
    group: &crate::ops::types::RecordingGroup,
    resource_type: &str,
) -> bool {
    if !is_global_resource_type(resource_type) {
        return true;
    }
    if super::recording_strategy_of(group) != super::RECORDING_STRATEGY_ALL_SUPPORTED {
        return true;
    }
    group.include_global_resource_types
}

/// How many pages of ONE paged IAM listing a pass reads before it stops asking for more —
/// the roles it records, the managed policies it records, and the identities one of those
/// policies is attached to.
///
/// A HOST bound: the inventory a pass writes into is bounded
/// ([`super::DISCOVERED_RESOURCES_KEPT`]), so one pass reads at most the pages that can
/// fill it rather than following a marker without end. ONE bound for every listing here,
/// so no reader follows a marker further than another.
const MAX_IAM_PAGES: usize = 20;

/// The configuration items the recorder observes for this account's IAM roles, or `None`
/// when IAM could not be read at all — which is not the same as an account with no roles.
///
/// Every member of the recorded configuration document comes from an IAM read named on
/// it; nothing else is recorded, because nothing else is observed. A role is identified by
/// the `RoleId` IAM minted for it — the id that does not change when the role is renamed —
/// and its custom name is the role name, which is how Config identifies a role in its
/// inventory.
pub async fn observed_iam_roles(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    now: SystemTime,
) -> Option<Vec<DiscoveredResourceEntry>> {
    let client = ::service_common::iam_client::iam_client_through(callback, region);

    // "IAM resource-listing operations return a subset of the available attributes"
    // (`ListRoles`), and the listing is paged: a pass follows the marker so an account
    // holding more roles than one page is observed in full.
    let mut listed = Vec::new();
    let mut marker: Option<String> = None;
    for _ in 0..MAX_IAM_PAGES {
        let page = match client
            .list_roles()
            .set_marker(marker.clone())
            .send()
            .await
        {
            Ok(page) => page,
            Err(error) => {
                ::tracing::warn!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the configuration recorder could not read this account's IAM roles"
                );
                return None;
            }
        };
        listed.extend(page.roles.clone());
        marker = page.marker.clone();
        if !page.is_truncated || marker.is_none() {
            break;
        }
    }

    let mut observed = Vec::new();
    for role in listed {
        // `ListRoleTags`' `Tags`: the tags a rule's scope can narrow by, which the
        // listing above withholds ("This operation does not return the following
        // attributes ... Tags").
        let tags: BTreeMap<String, String> = client
            .list_role_tags()
            .role_name(&role.role_name)
            .send()
            .await
            .ok()
            .map(|answer| {
                answer
                    .tags
                    .into_iter()
                    .map(|tag| (tag.key, tag.value))
                    .collect()
            })
            .unwrap_or_default();

        // The configuration item Config records for a role: the members IAM's own read
        // reports for it. IAM reports a role's whole configuration through one call, so
        // there is no further block to supplement it with.
        let configuration = ::serde_json::json!({
            // "The path to the role" (`Role$Path`).
            "path": role.path,
            // "The friendly name that identifies the role" (`Role$RoleName`).
            "roleName": role.role_name,
            // "The stable and unique string identifying the role" (`Role$RoleId`).
            "roleId": role.role_id,
            "arn": role.arn,
            // "The date and time, in ISO 8601 date-time format, when the role was
            // created" (`Role$CreateDate`).
            "createDate": role
                .create_date
                .fmt(::aws_smithy_types::date_time::Format::DateTime)
                .ok(),
            // "The policy that grants an entity permission to assume the role"
            // (`Role$AssumeRolePolicyDocument`).
            "assumeRolePolicyDocument": role.assume_role_policy_document,
            // "A description of the role that you provide" (`Role$Description`).
            "description": role.description,
            // "The maximum session duration (in seconds) for the specified role"
            // (`Role$MaxSessionDuration`).
            "maxSessionDuration": role.max_session_duration,
        });

        observed.push(DiscoveredResourceEntry {
            resource_type: IAM_ROLE_RESOURCE_TYPE.to_string(),
            resource_id: role.role_id.clone(),
            resource_name: Some(role.role_name.clone()),
            // The ARN IAM minted for the role, which is the ARN its configuration item
            // carries: it names the role by its path and name, neither of which the
            // recorded id is.
            arn: Some(role.arn.clone()),
            deleted_at: None,
            // The schema version of a configuration item Config itself recorded is not
            // one a caller supplied, and this emulator has no catalogue of them, so none
            // is reported rather than an invented one.
            schema_version_id: None,
            configuration: Some(configuration.to_string()),
            supplementary_configuration: BTreeMap::new(),
            tags,
            first_recorded_at: Some(now),
            last_recorded_at: Some(now),
            // The links IAM reports FROM a role are not observed here: what this pass
            // reads of a role is the role itself, and the relationship between a managed
            // policy and the identities it is attached to is recorded on the POLICY's own
            // configuration item ([`observed_iam_policies`]).
            relationships: Vec::new(),
            // The states this role has already left are kept by the store's own write
            // path, which carries them over from the item this one replaces.
            history: Vec::new(),
        });
    }
    Some(observed)
}

/// The `Scope` that lists the account's OWN managed policies and nothing else: "to list
/// only the customer managed policies in your Amazon Web Services account, set `Scope` to
/// `Local`" (`ListPolicies`). A policy Amazon Web Services owns belongs to no account, so
/// no account's recorder records it.
const CUSTOMER_MANAGED_SCOPE: ::aws_sdk_iam::types::PolicyScopeType =
    ::aws_sdk_iam::types::PolicyScopeType::Local;

/// The relationship wording Config reports a managed policy's ATTACHMENT to each kind of
/// identity by, one per entity kind `ListEntitiesForPolicy` reports
/// (`ListEntitiesForPolicyResponse$PolicyRoles`, `$PolicyUsers`, `$PolicyGroups`).
///
/// `relationship_name` is `Relationship$relationshipName` as Config publishes it for an
/// `AWS::IAM::Policy` configuration item, and `resource_type` is the
/// `com.amazonaws.configservice#ResourceType` entry that entity kind is recorded under.
struct AttachedIdentityKind {
    /// See [`AttachedIdentityKind`].
    resource_type: &'static str,
    /// See [`AttachedIdentityKind`].
    relationship_name: &'static str,
}

/// The kind of identity an attachment is TO, one per list `ListEntitiesForPolicy` reports.
const ATTACHED_TO_ROLE: AttachedIdentityKind = AttachedIdentityKind {
    resource_type: IAM_ROLE_RESOURCE_TYPE,
    relationship_name: "Is attached to Role",
};
/// See [`ATTACHED_TO_ROLE`].
const ATTACHED_TO_USER: AttachedIdentityKind = AttachedIdentityKind {
    resource_type: "AWS::IAM::User",
    relationship_name: "Is attached to User",
};
/// See [`ATTACHED_TO_ROLE`].
const ATTACHED_TO_GROUP: AttachedIdentityKind = AttachedIdentityKind {
    resource_type: "AWS::IAM::Group",
    relationship_name: "Is attached to Group",
};

/// The configuration items the recorder observes for this account's CUSTOMER MANAGED
/// policies, or `None` when IAM could not be read in full — which is not the same as an
/// account with no managed policies of its own.
///
/// Every member of the recorded configuration document, and every relationship the item
/// carries, comes from an IAM read named on it; nothing else is recorded, because nothing
/// else is observed. A policy is identified by the `PolicyId` IAM minted for it — the id
/// that does not change when the policy's document does — and its custom name is the
/// policy name.
///
/// A read that FAILED says nothing about the policy it was about: since an item recording
/// part of a policy would record a state the policy was never in, such a pass observes
/// nothing at all rather than a policy with members missing.
pub async fn observed_iam_policies(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    now: SystemTime,
) -> Option<Vec<DiscoveredResourceEntry>> {
    let client = ::service_common::iam_client::iam_client_through(callback, region);

    // The listing is paged, so a pass follows the marker and an account holding more
    // policies than one page is observed in full.
    let mut listed = Vec::new();
    let mut marker: Option<String> = None;
    for _ in 0..MAX_IAM_PAGES {
        let page = match client
            .list_policies()
            .scope(CUSTOMER_MANAGED_SCOPE.clone())
            .set_marker(marker.clone())
            .send()
            .await
        {
            Ok(page) => page,
            Err(error) => {
                ::tracing::warn!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the configuration recorder could not read this account's managed policies"
                );
                return None;
            }
        };
        listed.extend(page.policies.unwrap_or_default());
        marker = page.marker;
        if !page.is_truncated || marker.is_none() {
            break;
        }
    }

    let mut observed = Vec::new();
    for policy in listed {
        // A policy is recorded under the id IAM minted for it, and the ARN IAM minted is
        // the one its configuration item carries; neither is derived here.
        let (Some(policy_id), Some(arn)) = (policy.policy_id.clone(), policy.arn.clone()) else {
            continue;
        };

        // `GetPolicy` reports the members a listing withholds — "IAM resource-listing
        // operations return a subset of the available attributes" — which for a policy are
        // its description and its tags.
        let detailed = match client.get_policy().policy_arn(&arn).send().await {
            Ok(answer) => answer.policy,
            Err(error) => {
                ::tracing::warn!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the configuration recorder could not read a managed policy"
                );
                return None;
            }
        };
        // A read that answered WITHOUT the policy itself leaves the listing's own record of
        // it, which carries every member bar the ones the listing withholds.
        let detailed = detailed.unwrap_or(policy);
        let tags: BTreeMap<String, String> = detailed
            .tags
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|tag| (tag.key, tag.value))
            .collect();

        let versions = observed_policy_versions(&client, &arn).await?;
        let relationships = observed_policy_attachments(&client, &arn).await?;

        // The configuration item Config records for a customer managed policy: the
        // members IAM's own reads report for it, each named on the read it comes from.
        let configuration = ::serde_json::json!({
            // "The friendly name (not ARN) identifying the policy" (`Policy$PolicyName`).
            "policyName": detailed.policy_name,
            // "The stable and unique string identifying the policy" (`Policy$PolicyId`).
            "policyId": policy_id,
            "arn": arn,
            // "The path to the policy" (`Policy$Path`).
            "path": detailed.path,
            // "The identifier for the version of the policy that is set as the default
            // version" (`Policy$DefaultVersionId`).
            "defaultVersionId": detailed.default_version_id,
            // "The number of entities (users, groups, and roles) that the policy is
            // attached to" (`Policy$AttachmentCount`), which IAM derives from the
            // attachments that exist now.
            "attachmentCount": detailed.attachment_count,
            // "The number of entities (users and roles) for which the policy is used to
            // set the permissions boundary" (`Policy$PermissionsBoundaryUsageCount`).
            "permissionsBoundaryUsageCount": detailed.permissions_boundary_usage_count,
            // "Specifies whether the policy can be attached to an IAM user, group, or
            // role" (`Policy$IsAttachable`).
            "isAttachable": detailed.is_attachable,
            // "A friendly description of the policy" (`Policy$Description`).
            "description": detailed.description,
            // "The date and time, in ISO 8601 date-time format, when the policy was
            // created" and "when the policy was last updated" (`Policy$CreateDate`,
            // `Policy$UpdateDate`).
            "createDate": detailed
                .create_date
                .and_then(|at| at.fmt(::aws_smithy_types::date_time::Format::DateTime).ok()),
            "updateDate": detailed
                .update_date
                .and_then(|at| at.fmt(::aws_smithy_types::date_time::Format::DateTime).ok()),
            // Every version the policy carries, each with the document it grants
            // ([`observed_policy_versions`]).
            "policyVersionList": versions,
        });

        observed.push(DiscoveredResourceEntry {
            resource_type: IAM_POLICY_RESOURCE_TYPE.to_string(),
            resource_id: policy_id,
            resource_name: detailed.policy_name.clone(),
            // The ARN IAM minted for the policy: it names the policy by its path and
            // name, neither of which the recorded id is.
            arn: Some(arn),
            deleted_at: None,
            // The schema version of a configuration item Config itself recorded is not
            // one a caller supplied, and this emulator has no catalogue of them, so none
            // is reported rather than an invented one.
            schema_version_id: None,
            configuration: Some(configuration.to_string()),
            // IAM reports a policy's whole configuration through the reads above, so
            // there is no further block to supplement it with.
            supplementary_configuration: BTreeMap::new(),
            tags,
            first_recorded_at: Some(now),
            last_recorded_at: Some(now),
            relationships,
            // The states this policy has already left are kept by the store's own write
            // path, which carries them over from the item this one replaces.
            history: Vec::new(),
        });
    }
    Some(observed)
}

/// Every version of the managed policy `arn`, newest first, as the recorded
/// `policyVersionList` carries them — or `None` when IAM could not be read.
///
/// `ListPolicyVersions` reports the versions a policy holds but not what each grants
/// ("the policy document … is not returned in the response to the `ListPolicyVersions`
/// operation"), so the document of each version comes from that version's own
/// `GetPolicyVersion` read.
async fn observed_policy_versions(
    client: &::aws_sdk_iam::Client,
    arn: &str,
) -> Option<Vec<::serde_json::Value>> {
    let listed = match client.list_policy_versions().policy_arn(arn).send().await {
        Ok(answer) => answer.versions.unwrap_or_default(),
        Err(error) => {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the configuration recorder could not read a managed policy's versions"
            );
            return None;
        }
    };

    let mut versions = Vec::new();
    for version in listed {
        let Some(version_id) = version.version_id.clone() else {
            continue;
        };
        let document = match client
            .get_policy_version()
            .policy_arn(arn)
            .version_id(&version_id)
            .send()
            .await
        {
            Ok(answer) => answer.policy_version.and_then(|held| held.document),
            Err(error) => {
                ::tracing::warn!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the configuration recorder could not read a managed policy version"
                );
                return None;
            }
        };
        versions.push(::serde_json::json!({
            // "The document that defines the permissions" (`PolicyVersion$Document`).
            "document": document,
            // "The identifier for the policy version" (`PolicyVersion$VersionId`).
            "versionId": version_id,
            // "Specifies whether the policy version is set as the policy's default
            // version" (`PolicyVersion$IsDefaultVersion`).
            "isDefaultVersion": version.is_default_version,
            // "The date and time, in ISO 8601 date-time format, when the policy version
            // was created" (`PolicyVersion$CreateDate`).
            "createDate": version
                .create_date
                .and_then(|at| at.fmt(::aws_smithy_types::date_time::Format::DateTime).ok()),
        }));
    }
    Some(versions)
}

/// The relationships the managed policy `arn`'s configuration item carries: one per
/// identity the policy is ATTACHED to, in the wording Config reports that attachment by —
/// or `None` when IAM could not be read.
///
/// The identities come from `ListEntitiesForPolicy`, "the list of IAM users, groups, and
/// roles that the specified managed policy is attached to", narrowed to the policies put
/// to use as a PERMISSIONS POLICY ("to list only the policies used to set permissions
/// boundaries, set the value to `PermissionsBoundary`"): a boundary is not an attachment,
/// and it is the attachments that `Policy$AttachmentCount` counts. The listing is paged,
/// so a pass follows the marker and a policy attached to more identities than one page
/// holds is observed in full.
///
/// The result is ORDERED, so two observations of an unchanged policy carry the same
/// relationships in the same order rather than the order one listing happened to walk its
/// stores in.
async fn observed_policy_attachments(
    client: &::aws_sdk_iam::Client,
    arn: &str,
) -> Option<Vec<RecordedRelationship>> {
    let mut relationships: Vec<RecordedRelationship> = Vec::new();
    let mut marker: Option<String> = None;
    for _ in 0..MAX_IAM_PAGES {
        let page = match client
            .list_entities_for_policy()
            .policy_arn(arn)
            .policy_usage_filter(::aws_sdk_iam::types::PolicyUsageType::PermissionsPolicy)
            .set_marker(marker.clone())
            .send()
            .await
        {
            Ok(page) => page,
            Err(error) => {
                ::tracing::warn!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the configuration recorder could not read what a managed policy is \
                     attached to"
                );
                return None;
            }
        };
        for role in page.policy_roles.unwrap_or_default() {
            relationships.extend(attachment(&ATTACHED_TO_ROLE, role.role_id, role.role_name));
        }
        for user in page.policy_users.unwrap_or_default() {
            relationships.extend(attachment(&ATTACHED_TO_USER, user.user_id, user.user_name));
        }
        for group in page.policy_groups.unwrap_or_default() {
            relationships.extend(attachment(
                &ATTACHED_TO_GROUP,
                group.group_id,
                group.group_name,
            ));
        }
        marker = page.marker;
        if !page.is_truncated || marker.is_none() {
            break;
        }
    }
    relationships.sort();
    relationships.dedup();
    Some(relationships)
}

/// One attachment of a managed policy to the identity `(resource_id, resource_name)`, as
/// the `kind` of identity it is decides the relationship's type and wording
/// ([`ATTACHED_TO_ROLE`]).
///
/// An identity IAM reported without the id that identifies it is not related to anything
/// that could be read back, so it contributes no relationship rather than one naming
/// nothing.
fn attachment(
    kind: &AttachedIdentityKind,
    resource_id: Option<String>,
    resource_name: Option<String>,
) -> Option<RecordedRelationship> {
    Some(RecordedRelationship {
        resource_type: kind.resource_type.to_string(),
        resource_id: resource_id?,
        resource_name,
        relationship_name: kind.relationship_name.to_string(),
    })
}

/// The account's own configuration, as an account-wide rule is evaluated against it.
#[derive(Debug, Clone, Default)]
pub struct ObservedAccount {
    /// The account's password policy, or `None` for an account that has NONE — which is
    /// a fact about the account ("The request was rejected because it referenced a
    /// resource entity that does not exist", which is how IAM answers a read of a policy
    /// the account never set), not a failure to read it.
    pub password_policy: Option<ObservedPasswordPolicy>,
}

/// The account password policy IAM reports, as the members an account-wide rule reads it
/// by (`com.amazonaws.iam#PasswordPolicy`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObservedPasswordPolicy {
    /// "Minimum length to require for IAM user passwords"
    /// (`PasswordPolicy$MinimumPasswordLength`).
    pub minimum_password_length: Option<i32>,
    /// Whether a password must carry one of the symbols the member lists
    /// (`PasswordPolicy$RequireSymbols`).
    pub require_symbols: bool,
    /// Whether a password must carry a numeric character
    /// (`PasswordPolicy$RequireNumbers`).
    pub require_numbers: bool,
    /// Whether a password must carry an uppercase character
    /// (`PasswordPolicy$RequireUppercaseCharacters`).
    pub require_uppercase_characters: bool,
    /// Whether a password must carry a lowercase character
    /// (`PasswordPolicy$RequireLowercaseCharacters`).
    pub require_lowercase_characters: bool,
    /// "The number of days that an IAM user password is valid"
    /// (`PasswordPolicy$MaxPasswordAge`), absent where passwords never expire.
    pub max_password_age: Option<i32>,
    /// "The number of previous passwords that IAM users are prevented from reusing"
    /// (`PasswordPolicy$PasswordReusePrevention`), absent where reuse is not prevented.
    pub password_reuse_prevention: Option<i32>,
}

/// The account's own configuration as IAM reports it, or `None` when IAM could not be
/// read at all — which is not the same as an account that has set nothing.
///
/// `GetAccountPasswordPolicy` answers the policy in force, and answers `NoSuchEntity` for
/// an account that has never set one; both are facts the account's rules are evaluated
/// against, so they are told apart here and only a read that FAILED leaves the account
/// unobserved.
pub async fn observed_account(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> Option<ObservedAccount> {
    let client = ::service_common::iam_client::iam_client_through(callback, region);
    match client.get_account_password_policy().send().await {
        Ok(answer) => {
            let policy = answer.password_policy?;
            Some(ObservedAccount {
                password_policy: Some(ObservedPasswordPolicy {
                    minimum_password_length: policy.minimum_password_length,
                    require_symbols: policy.require_symbols,
                    require_numbers: policy.require_numbers,
                    require_uppercase_characters: policy.require_uppercase_characters,
                    require_lowercase_characters: policy.require_lowercase_characters,
                    max_password_age: policy.max_password_age,
                    password_reuse_prevention: policy.password_reuse_prevention,
                }),
            })
        }
        Err(error) => {
            // The account holds no password policy: IAM says so with the entity-not-found
            // refusal, which is an OBSERVATION of the account rather than a failed read.
            if error
                .as_service_error()
                .is_some_and(|service| service.is_no_such_entity_exception())
            {
                return Some(ObservedAccount {
                    password_policy: None,
                });
            }
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "Config could not read this account's password policy"
            );
            None
        }
    }
}
