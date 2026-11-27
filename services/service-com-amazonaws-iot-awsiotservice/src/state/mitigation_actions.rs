// SPDX-License-Identifier: CC0-1.0

//! The account's DEVICE DEFENDER MITIGATION ACTIONS: the record one holds, the ONE type
//! of change each defines, and what every reader of one reports.
//!
//! A mitigation action is "an action that can be applied to audit findings by using
//! StartAuditMitigationActionsTask" (`CreateMitigationAction`), and the name is the
//! action's identity — the model binds it as the `@httpLabel` of
//! `/mitigationactions/actions/{actionName}` on every single-action operation, and
//! `UpdateMitigationAction$actionName` documents "You cannot change the name by using
//! UpdateMitigationAction. Instead, you must delete and recreate the mitigation action
//! with the new name" — so the store is a `BTreeMap` keyed by it. `BTreeMap` (not
//! `HashMap`) because `ListMitigationActions` is `@paginated` and a page boundary is only
//! meaningful over a stable order.
//!
//! One record carries everything a reader hands back, so `DescribeMitigationAction` after
//! `CreateMitigationAction` reports the created action member for member: a reader clones
//! what the writer stored and the two cannot drift.
//!
//! What a writer of an action is held to lives here too, so the create and the update read
//! one rule: the parameters define exactly one type of change
//! ([`ONE_ACTION_PARAMETER_MESSAGE`]), and the `roleArn` they are applied with is the ARN
//! of an IAM role ([`unreadable_role_arn`]) — the model bounds that member's LENGTH alone,
//! so its FORM is read here. A create under a name the account already holds is decided by
//! what it ASKS for ([`RegisteredMitigationAction::applies_the_same_change`]): the same
//! action is the record itself, and a different one is the collision this family states
//! with the ARN of the record that exists ([`super::existing_mitigation_action_message`]).
//!
//! The action's TYPE is DERIVED from the parameters it was created with rather than
//! recorded beside them ([`RegisteredMitigationAction::action_type`]): "Each mitigation
//! action can apply only one type of change" (`CreateMitigationAction`), and
//! `MitigationActionParams` is a structure with one optional member per type
//! (`smithy_query(query="shape",shape="MitigationActionParams")`), so the member that is
//! set IS the type and a record cannot report one type while holding another's
//! parameters.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    DescribeMitigationActionOutput, MitigationActionIdentifier, MitigationActionParams,
};

/// How many mitigation actions this emulator keeps for the account.
///
/// AWS publishes a service quota on an account's mitigation actions; the exact figure is
/// not in the model, so this is the emulator's own ceiling on a store a caller can grow —
/// the same reason [`super::MAX_POLICIES`] exists — and `CreateMitigationAction` past it
/// refuses with the modelled `LimitExceededException` rather than growing without bound.
pub const MAX_MITIGATION_ACTIONS: usize = 100;

/// How many tags one mitigation action may carry: the same documented IoT tagging limit
/// [`super::MAX_POLICY_TAGS`] records, for the same reason.
pub const MAX_MITIGATION_ACTION_TAGS: usize = super::MAX_POLICY_TAGS;

/// The `MitigationActionType` of an action that changes a DEVICE certificate's status.
pub const ACTION_TYPE_UPDATE_DEVICE_CERTIFICATE: &str = "UPDATE_DEVICE_CERTIFICATE";
/// The `MitigationActionType` of an action that changes a CA certificate's status.
pub const ACTION_TYPE_UPDATE_CA_CERTIFICATE: &str = "UPDATE_CA_CERTIFICATE";
/// The `MitigationActionType` of an action that adds things to thing groups.
pub const ACTION_TYPE_ADD_THINGS_TO_THING_GROUP: &str = "ADD_THINGS_TO_THING_GROUP";
/// The `MitigationActionType` of an action that replaces a policy's default version.
pub const ACTION_TYPE_REPLACE_DEFAULT_POLICY_VERSION: &str = "REPLACE_DEFAULT_POLICY_VERSION";
/// The `MitigationActionType` of an action that enables AWS IoT logging.
pub const ACTION_TYPE_ENABLE_IOT_LOGGING: &str = "ENABLE_IOT_LOGGING";
/// The `MitigationActionType` of an action that publishes the finding to an SNS topic.
pub const ACTION_TYPE_PUBLISH_FINDING_TO_SNS: &str = "PUBLISH_FINDING_TO_SNS";

/// The ARN of a mitigation action:
/// `arn:aws:iot:<region>:<account>:mitigationaction/<actionName>`.
///
/// `mitigationaction/` is the resource type AWS IoT's mitigation-action ARNs use — it is
/// what `CreateMitigationAction$actionArn` / `DescribeMitigationAction$actionArn` report
/// and what an `iot:DescribeMitigationAction` policy statement names an action by; the
/// namespace and account segments are the ones [`super::thing_arn`] documents.
pub fn mitigation_action_arn(region: &str, action_name: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("mitigationaction/{action_name}"))
}

/// How this service states that an `actionName` names no mitigation action this account
/// holds — for a reader of one and for a writer of one.
///
/// This family states the miss by naming the MEMBER the request carried the name at: the
/// fixed text is the family's and the variable part is the name asked about, so a
/// describe and an update of the same absent action word the miss one way.
pub fn missing_mitigation_action_message(action_name: &str) -> String {
    format!("No action with name {action_name} is found")
}

/// How `UpdateMitigationAction` states that same miss.
///
/// The WRITE path words it as the RESOURCE it could not resolve rather than as the action
/// it could not read: the Device Defender configuration families' sentence
/// ([`super::unresolved_resource_message`]), naming the member the update resolves the
/// action by, `findingActionName`, with the name the request carried as its variable part.
/// Two sentences for one miss because AWS states them differently on the two paths, and a
/// caller reading either reads the one AWS sends.
pub fn unresolved_mitigation_action_message(action_name: &str) -> String {
    super::unresolved_resource_message(FINDING_ACTION_NAME_MEMBER, action_name)
}

/// The input member `UpdateMitigationAction` resolves a mitigation action by, as its
/// not-found sentence names it.
const FINDING_ACTION_NAME_MEMBER: &str = "findingActionName";

/// The `MitigationActionType`s `params` defines — one per member it sets, in the order the
/// model declares them.
///
/// A well-formed `MitigationActionParams` defines exactly ONE: "Each mitigation action can
/// apply only one type of change", so the writers read this and refuse anything else
/// ([`ONE_ACTION_PARAMETER_MESSAGE`]), and a reader of a stored record takes the single
/// entry as the record's type.
pub fn action_types_defined(params: &MitigationActionParams) -> Vec<&'static str> {
    let defined = [
        (
            params.update_device_certificate_params.is_some(),
            ACTION_TYPE_UPDATE_DEVICE_CERTIFICATE,
        ),
        (
            params.update_ca_certificate_params.is_some(),
            ACTION_TYPE_UPDATE_CA_CERTIFICATE,
        ),
        (
            params.add_things_to_thing_group_params.is_some(),
            ACTION_TYPE_ADD_THINGS_TO_THING_GROUP,
        ),
        (
            params.replace_default_policy_version_params.is_some(),
            ACTION_TYPE_REPLACE_DEFAULT_POLICY_VERSION,
        ),
        (
            params.enable_io_t_logging_params.is_some(),
            ACTION_TYPE_ENABLE_IOT_LOGGING,
        ),
        (
            params.publish_finding_to_sns_params.is_some(),
            ACTION_TYPE_PUBLISH_FINDING_TO_SNS,
        ),
    ];
    defined
        .into_iter()
        .filter_map(|(is_set, action_type)| is_set.then_some(action_type))
        .collect()
}

/// Whether two `MitigationActionParams` define the SAME change: the same type of action,
/// with the same parameters for it.
///
/// Read member by member off the modelled shape
/// (`smithy_query(query="shape",shape="MitigationActionParams")`) rather than through a
/// derived comparison, because the shape and the per-type parameter structures it holds are
/// rendered from the upstream model and carry only the derives the generator writes: a
/// comparison of the VALUES a caller sent belongs to this module, where the rule that uses
/// it lives, and keeps holding across a model re-render.
pub fn defines_the_same_action(
    left: &MitigationActionParams,
    right: &MitigationActionParams,
) -> bool {
    same_params(
        &left.update_device_certificate_params,
        &right.update_device_certificate_params,
        |left, right| left.action == right.action,
    ) && same_params(
        &left.update_ca_certificate_params,
        &right.update_ca_certificate_params,
        |left, right| left.action == right.action,
    ) && same_params(
        &left.add_things_to_thing_group_params,
        &right.add_things_to_thing_group_params,
        |left, right| {
            left.thing_group_names == right.thing_group_names
                && left.override_dynamic_groups == right.override_dynamic_groups
        },
    ) && same_params(
        &left.replace_default_policy_version_params,
        &right.replace_default_policy_version_params,
        |left, right| left.template_name == right.template_name,
    ) && same_params(
        &left.enable_io_t_logging_params,
        &right.enable_io_t_logging_params,
        |left, right| {
            left.role_arn_for_logging == right.role_arn_for_logging
                && left.log_level == right.log_level
        },
    ) && same_params(
        &left.publish_finding_to_sns_params,
        &right.publish_finding_to_sns_params,
        |left, right| left.topic_arn == right.topic_arn,
    )
}

/// One optional per-type parameter member of two `MitigationActionParams` compared: both
/// absent is the same change, both present is whatever `same` makes of the two values, and
/// one of each is a different change — the type itself differs.
fn same_params<T>(
    left: &Option<T>,
    right: &Option<T>,
    same: impl FnOnce(&T, &T) -> bool,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => same(left, right),
        _ => false,
    }
}

/// How this service refuses `actionParams` that do not define exactly one type of change —
/// parameters defining NONE and parameters defining more than one alike.
///
/// `MitigationActionParams` documents "You can specify only one type of parameter (in other
/// words, you can apply only one action for each defined mitigation action)"
/// (`smithy_query(query="doc",shape="MitigationActionParams")`), and the refusal states
/// that one rule as ENTIRELY fixed text: it names neither the member nor the types the
/// request's parameters defined, so one sentence states both sides of the rule and the
/// create and the update word it one way.
pub const ONE_ACTION_PARAMETER_MESSAGE: &str = "Expected only one action parameter to be set";

/// The refusal a mitigation action's `roleArn` is answered with when it is not the ARN of
/// an IAM role at all, or `None` when it names one.
///
/// `roleArn` is "The ARN of the IAM role that is used to apply the mitigation action" and
/// the model constrains it by LENGTH alone — `RoleArn`, 20..2048, with no `@pattern`
/// (`smithy_query(query="operation",operation="CreateMitigationAction")`) — so a value long
/// enough to satisfy that bound and yet no role ARN passes the request boundary and has to
/// be read here: an action stored under it names a role nothing could ever be applied
/// with. The form required is the one every service reads an IAM role ARN with
/// ([`::service_common::arn::IAM_ROLE`]), so no two planes disagree about what the ARN of a
/// role is, and the refusal is the modelled `InvalidRequestException` (HTTP 400) carrying
/// the sentence [`invalid_role_arn_message`] states.
pub fn unreadable_role_arn(role_arn: &str) -> Option<String> {
    (!::service_common::arn::IAM_ROLE.admits(role_arn)).then(|| invalid_role_arn_message(role_arn))
}

/// How this service states that a `roleArn` holds text that is not the ARN of an IAM role.
///
/// The fixed text is `Invalid Role ARN:: ` — the clause's own colon followed by the colon
/// that opens the value — and the single variable part is the value the request carried,
/// echoed exactly as it was sent, so a caller reads back what it submitted and nothing of
/// the account's state.
pub fn invalid_role_arn_message(role_arn: &str) -> String {
    format!("Invalid Role ARN:: {role_arn}")
}

/// One mitigation action in the store, as this service persists it.
#[derive(Debug, Clone)]
pub struct RegisteredMitigationAction {
    /// The name the action was created under; also its key in the store.
    pub action_name: String,
    /// `CreateMitigationAction$actionId` — "A unique identifier for the new mitigation
    /// action", minted on create and kept by every later update of the record.
    pub action_id: String,
    /// `arn:aws:iot:<region>:<account>:mitigationaction/<actionName>`, minted on create.
    pub action_arn: String,
    /// `CreateMitigationAction$roleArn` — "The ARN of the IAM role that is used to apply
    /// the mitigation action".
    pub role_arn: String,
    /// `CreateMitigationAction$actionParams` — "Defines the type of action and the
    /// parameters for that action". Exactly one member of it is set, which is what makes
    /// [`RegisteredMitigationAction::action_type`] a reading of the record rather than a
    /// second copy of it.
    pub action_params: MitigationActionParams,
    /// `Tag$Key` -> `Tag$Value` for the tags the create request carried.
    pub tags: BTreeMap<String, Option<String>>,
    /// When the action was created — `DescribeMitigationAction$creationDate`.
    pub creation_date: SystemTime,
    /// When the action last changed — `DescribeMitigationAction$lastModifiedDate`. Equal
    /// to [`RegisteredMitigationAction::creation_date`] until an update moves it.
    pub last_modified_date: SystemTime,
}

impl RegisteredMitigationAction {
    /// A newly created mitigation action: the minted identity plus the requested role,
    /// parameters and tags.
    pub fn new(
        action_name: String,
        action_id: String,
        action_arn: String,
        role_arn: String,
        action_params: MitigationActionParams,
        tags: BTreeMap<String, Option<String>>,
        created_at: SystemTime,
    ) -> Self {
        Self {
            action_name,
            action_id,
            action_arn,
            role_arn,
            action_params,
            tags,
            creation_date: created_at,
            last_modified_date: created_at,
        }
    }

    /// The ONE `MitigationActionType` this record's parameters define, read out of them
    /// ([`action_types_defined`]) so the type a reader reports is the type the stored
    /// parameters are for.
    pub fn action_type(&self) -> Option<&'static str> {
        action_types_defined(&self.action_params).first().copied()
    }

    /// Whether this record IS the action a create under its name asks for: the role the
    /// action is applied with and the parameters it applies — everything a create states
    /// ABOUT the action rather than naming it.
    ///
    /// This is what decides a repeat create, the way it decides one for every other family
    /// whose collision is stated as an existing resource with DIFFERENT attributes
    /// ([`super::existing_mitigation_action_message`], and
    /// [`super::ThingConfiguration`] for the registry's): a second create asking for
    /// exactly what the record holds is the same action, and one asking for anything else
    /// is the conflict.
    ///
    /// Tags are not part of it, for the reason the other comparing families leave them out:
    /// "Metadata that can be used to manage the mitigation action" is not what the action
    /// does, so a repeat create keeps the tags the record already carries, whatever the
    /// request sent — changing them is what `TagResource`/`UntagResource` are for.
    pub fn applies_the_same_change(
        &self,
        role_arn: &str,
        action_params: &MitigationActionParams,
    ) -> bool {
        self.role_arn == role_arn && defines_the_same_action(&self.action_params, action_params)
    }

    /// Apply an `UpdateMitigationAction`: the members the request carried replace what the
    /// record holds, the ones it left absent keep their stored values, and the record's
    /// last-modified instant moves to `updated_at`.
    ///
    /// The name, the id and the ARN are untouched: the name cannot be changed by this
    /// operation, and the other two are the identity minted for it.
    pub fn apply_update(
        &mut self,
        role_arn: Option<String>,
        action_params: Option<MitigationActionParams>,
        updated_at: SystemTime,
    ) {
        if let Some(role_arn) = role_arn {
            self.role_arn = role_arn;
        }
        if let Some(action_params) = action_params {
            self.action_params = action_params;
        }
        self.last_modified_date = updated_at;
    }

    /// The record as `DescribeMitigationAction` reports it — every member of the stored
    /// record, so the read and the write cannot drift.
    pub fn to_description(&self) -> DescribeMitigationActionOutput {
        DescribeMitigationActionOutput {
            action_name: Some(self.action_name.clone()),
            action_type: self.action_type().map(str::to_string),
            action_arn: Some(self.action_arn.clone()),
            action_id: Some(self.action_id.clone()),
            role_arn: Some(self.role_arn.clone()),
            action_params: Some(self.action_params.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            last_modified_date: Some(::aws_smithy_types::DateTime::from(self.last_modified_date)),
        }
    }

    /// The record as `ListMitigationActions` reports it: "Information that identifies a
    /// mitigation action", projected from the same record the reader above reports.
    pub fn to_identifier(&self) -> MitigationActionIdentifier {
        MitigationActionIdentifier {
            action_name: Some(self.action_name.clone()),
            action_arn: Some(self.action_arn.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::{
        EnableIoTLoggingParams, PublishFindingToSnsParams, UpdateCACertificateParams,
        MITIGATION_ACTION_TYPE_VALUES,
    };

    /// Every type this module names is a value the modelled `MitigationActionType` enum
    /// accepts, read through the generated constant so the two cannot drift.
    #[test]
    fn each_named_action_type_is_a_modeled_one() {
        for action_type in [
            ACTION_TYPE_UPDATE_DEVICE_CERTIFICATE,
            ACTION_TYPE_UPDATE_CA_CERTIFICATE,
            ACTION_TYPE_ADD_THINGS_TO_THING_GROUP,
            ACTION_TYPE_REPLACE_DEFAULT_POLICY_VERSION,
            ACTION_TYPE_ENABLE_IOT_LOGGING,
            ACTION_TYPE_PUBLISH_FINDING_TO_SNS,
        ] {
            assert!(
                MITIGATION_ACTION_TYPE_VALUES.contains(&action_type),
                "case {action_type:?}: expected one of the modelled values \
                 {MITIGATION_ACTION_TYPE_VALUES:?}, got a name the model does not carry"
            );
        }
    }

    /// The parameters a request carries decide the types it defines: one member set
    /// defines that member's type, none defines nothing, and two define both.
    #[test]
    fn the_defined_types_are_the_members_that_are_set() {
        let none = MitigationActionParams::default();
        assert!(
            action_types_defined(&none).is_empty(),
            "case no member set: expected no type, got {:?}",
            action_types_defined(&none)
        );

        let one = MitigationActionParams {
            publish_finding_to_sns_params: Some(PublishFindingToSnsParams {
                topic_arn: "arn:aws:sns:us-east-1:000000000000:defender".to_string(),
            }),
            ..MitigationActionParams::default()
        };
        assert_eq!(
            action_types_defined(&one),
            vec![ACTION_TYPE_PUBLISH_FINDING_TO_SNS],
            "case one member set: expected the type that member defines"
        );

        let two = MitigationActionParams {
            update_ca_certificate_params: Some(UpdateCACertificateParams {
                action: "DEACTIVATE".to_string(),
            }),
            enable_io_t_logging_params: Some(EnableIoTLoggingParams {
                role_arn_for_logging: "arn:aws:iam::000000000000:role/logging".to_string(),
                log_level: "INFO".to_string(),
            }),
            ..MitigationActionParams::default()
        };
        assert_eq!(
            action_types_defined(&two),
            vec![
                ACTION_TYPE_UPDATE_CA_CERTIFICATE,
                ACTION_TYPE_ENABLE_IOT_LOGGING
            ],
            "case two members set: expected both types, in the order the model declares \
             the members"
        );
    }
}
