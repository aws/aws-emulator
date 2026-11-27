// SPDX-License-Identifier: CC0-1.0

//! The restore testing SELECTIONS of a restore testing plan: what
//! `CreateRestoreTestingSelection` stores inside the plan it names, and what
//! every call that addresses a selection by name resolves against.
//!
//! # The rules this module holds
//!
//! * a selection belongs to ONE restore testing plan and is addressed by name
//!   within it — the name "cannot be changed after creation" (model doc for
//!   `RestoreTestingSelectionForList$RestoreTestingSelectionName`) — so the
//!   store is keyed by the (plan name, selection name) pair and a create naming
//!   a selection that plan already holds is refused as already existing
//!   ([`restore_testing_selection_already_exists`]);
//! * a create naming a plan the account does not hold resolves nothing to store
//!   the selection in, which is the plan's own not-found condition
//!   ([`super::restore_testing_plan_not_found`]), and a selection name the named
//!   plan does not hold is this resource's
//!   ([`restore_testing_selection_not_found`]);
//! * a selection is "RestoreTestingSelectionName, ProtectedResourceType, and one
//!   of the following: ProtectedResourceArns ProtectedResourceConditions …
//!   Cannot select by both protected resource types AND specific ARNs. Request
//!   will fail if both are included" (the operation's own model doc), which is
//!   the reading [`rejected_restore_testing_selection_body`] applies before
//!   anything is stored;
//! * the body is stored as submitted, except for the three members every
//!   selection HAS rather than optionally states — the resources it is scoped
//!   to, the conditions it filters those resources by and the restore metadata
//!   keys it overrides, which are completed where the request states them
//!   emptily or not at all ([`stored_protected_resource_arns`],
//!   [`stored_protected_resource_conditions`],
//!   [`stored_restore_metadata_overrides`]) and so are reported by every read as
//!   the collections the selection holds, empty ones included, rather than
//!   omitted;
//! * an update rewrites the members its body states and leaves every other
//!   member of the stored selection alone
//!   ([`RestoreTestingSelectionRecord::apply_update`]), so the selection it
//!   merged is what every later read reports.

use std::collections::HashMap;
use std::time::SystemTime;

use super::ComAmazonawsBackupCryocontrollerusermanagerState;
use crate::ops::types::{
    AlreadyExistsException, InvalidParameterValueException, LimitExceededException,
    MissingParameterValueException, ProtectedResourceConditions, ResourceNotFoundException,
    RestoreTestingSelectionForGet, RestoreTestingSelectionForList,
    RestoreTestingSelectionForUpdate,
};

/// The most restore testing selections this emulator keeps for one plan.
///
/// An EMULATOR-CHOSEN ceiling, not an AWS quota, for the reason given on
/// [`super::MAX_RESTORE_TESTING_PLANS`]: the store is in memory and every
/// request can add to it. AWS caps the selections one restore testing plan may
/// hold too, and answers the same modelled `LimitExceededException` for a create
/// past it.
pub const MAX_RESTORE_TESTING_SELECTIONS_PER_PLAN: usize = 500;

/// The most specific protected resource ARNs one selection may name: "you can
/// include up to 30 specific protected resource ARNs in ProtectedResourceArns"
/// (model doc for `CreateRestoreTestingSelection`).
pub const MAX_PROTECTED_RESOURCE_ARNS: usize = 30;

/// The `ProtectedResourceArns` entry that selects EVERY protected resource of
/// the selection's type rather than one named resource: "you can include a
/// wildcard: `ProtectedResourceArns: ["*"]`, but not both" (model doc for the
/// member).
pub const PROTECTED_RESOURCE_ARNS_WILDCARD: &str = "*";

/// The least and the most hours a stated validation window may be: "This is
/// amount of hours (0 to 168) available to run a validation script on the data"
/// (model doc for `RestoreTestingSelectionForCreate$ValidationWindowHours`).
/// The member carries a modelled default of `0`, so a request that states
/// nothing for it states the floor of that range.
pub const MIN_VALIDATION_WINDOW_HOURS: i32 = 0;
pub const MAX_VALIDATION_WINDOW_HOURS: i32 = 168;

/// The most characters a `RestoreTestingSelectionName` may carry: "The name
/// consists of only alphanumeric characters and underscores. Maximum length is
/// 50" (model doc for
/// `RestoreTestingSelectionForCreate$RestoreTestingSelectionName`).
pub const MAX_RESTORE_TESTING_SELECTION_NAME_LENGTH: usize = 50;

/// The sentence AWS states for "the restore testing selection asked about is not
/// one the named plan holds" — fixed, and worded as this service words the same
/// condition for the plan that holds it
/// ([`super::MESSAGE_RESTORE_TESTING_PLAN_NOT_FOUND`]): it names no part of the
/// request, so the name asked about does not appear in it. Its token is
/// [`ERROR_CODE_RESTORE_TESTING_SELECTION_NOT_FOUND`].
pub const MESSAGE_RESTORE_TESTING_SELECTION_NOT_FOUND: &str =
    "Restore testing selection does not exist.";

/// `Code` for "the restore testing selection asked about is not one the named
/// plan holds" — one token for the condition, stated the same way by every call
/// that addresses a selection by name, and distinct from the token the PLAN's
/// own not-found condition states
/// ([`super::ERROR_CODE_RESTORE_TESTING_PLAN_NOT_FOUND`]), so a client that
/// branches on it tells "no such selection of this plan" from "no such plan".
pub const ERROR_CODE_RESTORE_TESTING_SELECTION_NOT_FOUND: &str = "ERROR_RT_5001";

/// The FIXED text AWS states for "a restore testing selection of this name
/// already exists in this plan", up to and between its three variable parts: the
/// plan the create addressed, the account it was made in and the selection name
/// it asked for. Composed by
/// [`restore_testing_selection_already_exists_message`], the one place the
/// sentence is built — so the collision can be attributed to a plan, an account
/// and a name rather than only to a class.
pub const MESSAGE_RESTORE_TESTING_SELECTION_ALREADY_EXISTS_PREFIX: &str = "Restore testing plan [";
/// The text between the plan name and the account id in that sentence.
pub const MESSAGE_RESTORE_TESTING_SELECTION_ALREADY_EXISTS_IN_ACCOUNT: &str = "] in account [";
/// The text between the account id and the selection name in that sentence.
pub const MESSAGE_RESTORE_TESTING_SELECTION_ALREADY_EXISTS_CONTAINS: &str =
    "] already contains selection [";
/// The text that closes that sentence, after the selection name.
pub const MESSAGE_RESTORE_TESTING_SELECTION_ALREADY_EXISTS_SUFFIX: &str = "].";

/// `Code` for "a restore testing selection of this name already exists in this
/// plan" — the create conflict's own token, as the vault's create conflict has
/// its own ([`super::ERROR_CODE_VAULT_ALREADY_EXISTS`]). A client branching on
/// it tells a duplicate SELECTION from a duplicate PLAN, which share the
/// modelled class and status.
pub const ERROR_CODE_RESTORE_TESTING_SELECTION_ALREADY_EXISTS: &str = "ERROR_RT_3001";

/// The sentence AWS states for "a restore testing selection of this name already
/// exists in this plan": the fixed text above around its three variable parts —
/// the plan name and the selection name the request carried, and the account
/// this emulator presents ([`engine::emulator_account_id`]), as the account-naming
/// refusals of this service state it ([`super::framework_not_found_message`]).
pub fn restore_testing_selection_already_exists_message(
    plan_name: &str,
    selection_name: &str,
) -> String {
    format!(
        "{MESSAGE_RESTORE_TESTING_SELECTION_ALREADY_EXISTS_PREFIX}{plan_name}\
         {MESSAGE_RESTORE_TESTING_SELECTION_ALREADY_EXISTS_IN_ACCOUNT}{}\
         {MESSAGE_RESTORE_TESTING_SELECTION_ALREADY_EXISTS_CONTAINS}{selection_name}\
         {MESSAGE_RESTORE_TESTING_SELECTION_ALREADY_EXISTS_SUFFIX}",
        ::engine::emulator_account_id()
    )
}

/// The resource SCOPE a stored selection carries: the specific protected
/// resource ARNs the request named, and the documented wildcard where it named
/// none.
///
/// A selection selects "one of the following: ProtectedResourceArns
/// ProtectedResourceConditions" and "can include a wildcard value (\"*\") for
/// ProtectedResourceArns along with ProtectedResourceConditions" (model doc for
/// the operation), so a selection that names no specific ARN is scoped to every
/// protected resource of its type — filtered by its conditions where it states
/// any — which is what that wildcard form states
/// ([`PROTECTED_RESOURCE_ARNS_WILDCARD`]). The scope is therefore stored as the
/// wildcard and read back as it, rather than as the absent or empty list the
/// request happened to spell it with; a body that names neither ARNs nor
/// conditions selects nothing and is refused before anything is stored
/// ([`rejected_restore_testing_selection_body`]), so no selection is scoped to
/// the wildcard by this completion without having asked for every resource of
/// its type.
pub fn stored_protected_resource_arns(arns: Option<Vec<String>>) -> Vec<String> {
    let arns = arns.unwrap_or_default();
    if arns.is_empty() {
        return vec![PROTECTED_RESOURCE_ARNS_WILDCARD.to_string()];
    }
    arns
}

/// The conditions filter a STORED selection carries: the structure submitted
/// with both of its lists materialised, and the filter that matches on nothing
/// where the request stated none.
///
/// Every selection HAS a conditions filter — AWS reports the structure on every
/// read of a selection, as the empty-but-present form for one stored without
/// conditions — so what this service keeps is the filter the selection applies
/// rather than whether the request mentioned the member: a selection that
/// states no condition filters by no tag, which is exactly the filter whose two
/// lists are empty. The same completion the plan's create applies to the
/// members of a body the service fills in
/// ([`super::stored_restore_testing_recovery_point_selection`]).
pub fn stored_protected_resource_conditions(
    conditions: Option<ProtectedResourceConditions>,
) -> ProtectedResourceConditions {
    let conditions = conditions.unwrap_or_default();
    ProtectedResourceConditions {
        string_equals: Some(conditions.string_equals.unwrap_or_default()),
        string_not_equals: Some(conditions.string_not_equals.unwrap_or_default()),
    }
}

/// The restore metadata overrides a STORED selection carries: the map
/// submitted, and the empty map where the request stated none — for the reason
/// given on [`stored_protected_resource_conditions`]: a selection overrides a
/// set of restore metadata keys, and one that states no override overrides none,
/// which is the empty map AWS reports for it.
pub fn stored_restore_metadata_overrides(
    overrides: Option<HashMap<String, String>>,
) -> HashMap<String, String> {
    overrides.unwrap_or_default()
}

/// A restore testing selection: what `CreateRestoreTestingSelection` stores,
/// `UpdateRestoreTestingSelection` rewrites and `GetRestoreTestingSelection`
/// reports.
///
/// The body is kept as it was submitted, except for the three members every
/// selection has rather than optionally states, which are completed where the
/// request states them emptily or not at all (above) — so a read reports what
/// was stored rather than a value recomputed per call; the instants only this
/// service can know, when the selection was stored and when it was last
/// written, are its own members.
#[derive(Debug, Clone)]
pub struct RestoreTestingSelectionRecord {
    /// The plan this selection belongs to, the other half of the key it is
    /// stored under: a selection's name identifies it within ONE restore testing
    /// plan, so the same name under another plan is another selection.
    pub plan_name: String,
    pub name: String,
    /// The role a restore test started from this selection restores as, stored
    /// as submitted.
    pub iam_role_arn: String,
    pub protected_resource_type: String,
    /// The resources this selection restores: the specific ARNs the request
    /// named, or the wildcard where it named none
    /// ([`stored_protected_resource_arns`]) — so a selection always carries the
    /// scope it really has and a read always reports it.
    pub protected_resource_arns: Vec<String>,
    /// The tag conditions this selection filters protected resources by, with
    /// both of its lists materialised — empty where the selection filters by no
    /// tag.
    pub protected_resource_conditions: ProtectedResourceConditions,
    /// The restore metadata keys this selection overrides — empty where it
    /// overrides none.
    pub restore_metadata_overrides: HashMap<String, String>,
    /// The hours a restore test started from this selection keeps its data for
    /// validation, as the request stated it.
    ///
    /// The member is a plain integer carrying a modelled default of `0`, and
    /// `0` is also the floor of its documented range, so a request that states
    /// nothing for it and one that states that floor reach this service alike:
    /// an update carrying `0` states no new window and the stored one stands
    /// ([`Self::apply_update`]), which is the one window an update cannot move
    /// a stored selection back to.
    pub validation_window_hours: i32,
    pub creator_request_id: Option<String>,
    pub creation_time: SystemTime,
    /// When this selection was last written: the instant the create stored it
    /// until an update rewrites it.
    pub last_update_time: SystemTime,
}

impl RestoreTestingSelectionRecord {
    /// The selection as a READ reports it — the ONE projection of a stored
    /// selection, so a read can report nothing the store does not hold.
    ///
    /// The conditions filter, the overrides map and the resource scope are
    /// members of every stored selection, so this projection always reports
    /// them: a client that reads
    /// `ProtectedResourceConditions.StringEquals`, iterates
    /// `RestoreMetadataOverrides` or asks what a selection is scoped to after a
    /// successful read finds the collection the selection holds rather than
    /// nothing at all.
    pub fn for_get(&self) -> RestoreTestingSelectionForGet {
        RestoreTestingSelectionForGet {
            restore_testing_plan_name: self.plan_name.clone(),
            restore_testing_selection_name: self.name.clone(),
            iam_role_arn: self.iam_role_arn.clone(),
            protected_resource_type: self.protected_resource_type.clone(),
            protected_resource_arns: Some(self.protected_resource_arns.clone()),
            protected_resource_conditions: Some(self.protected_resource_conditions.clone()),
            restore_metadata_overrides: Some(self.restore_metadata_overrides.clone()),
            validation_window_hours: self.validation_window_hours,
            creator_request_id: self.creator_request_id.clone(),
            creation_time: super::modelled_timestamp(self.creation_time),
        }
    }

    /// The selection as a LISTING reports it — the metadata members
    /// `RestoreTestingSelectionForList` declares, each taken from the same
    /// stored record [`Self::for_get`] projects, so a listing and a read can
    /// never report a different plan, role, resource type, validation window or
    /// creation instant for one selection.
    ///
    /// The list projection carries NO collection member: the shape declares
    /// only the metadata ones (`smithy_query` shape
    /// `RestoreTestingSelectionForList`), so the resources a selection selects
    /// are reported by the read alone.
    pub fn for_list(&self) -> RestoreTestingSelectionForList {
        RestoreTestingSelectionForList {
            restore_testing_plan_name: self.plan_name.clone(),
            restore_testing_selection_name: self.name.clone(),
            iam_role_arn: self.iam_role_arn.clone(),
            protected_resource_type: self.protected_resource_type.clone(),
            validation_window_hours: self.validation_window_hours,
            creation_time: super::modelled_timestamp(self.creation_time),
        }
    }

    /// Rewrite the members an update may change, and record the instant it
    /// changed them.
    ///
    /// "Most elements except the RestoreTestingSelectionName can be updated with
    /// this request" (the update's model doc), so the name, the plan it belongs
    /// to, the protected resource type it was created for and the creation
    /// instant survive the rewrite, and the five members
    /// `RestoreTestingSelectionForUpdate` declares are what this applies. A
    /// member the body states NOTHING for keeps the value the selection already
    /// held; a stated collection REPLACES the stored one, completed exactly as a
    /// create completes it ([`stored_protected_resource_conditions`],
    /// [`stored_restore_metadata_overrides`]), so a selection an update rewrote
    /// and a selection a create stored read back the same way.
    pub fn apply_update(&mut self, body: RestoreTestingSelectionForUpdate, now: SystemTime) {
        if let Some(iam_role_arn) = body.iam_role_arn {
            self.iam_role_arn = iam_role_arn;
        }
        if let Some(arns) = body.protected_resource_arns {
            self.protected_resource_arns = stored_protected_resource_arns(Some(arns));
        }
        if let Some(conditions) = body.protected_resource_conditions {
            self.protected_resource_conditions =
                stored_protected_resource_conditions(Some(conditions));
        }
        if let Some(overrides) = body.restore_metadata_overrides {
            self.restore_metadata_overrides = stored_restore_metadata_overrides(Some(overrides));
        }
        // A validation window the update states nothing for reaches this
        // service as the modelled default of its integer member (`0`), which is
        // indistinguishable from a stated zero: it states no new window, so the
        // stored one stands — the reading the plan's update applies to its own
        // windows ([`super::RestoreTestingPlanRecord::apply_update`]).
        if body.validation_window_hours != 0 {
            self.validation_window_hours = body.validation_window_hours;
        }
        self.last_update_time = now;
    }
}

/// The modelled refusal for a `RestoreTestingSelectionName` the named restore
/// testing plan holds no selection under — see
/// [`MESSAGE_RESTORE_TESTING_SELECTION_NOT_FOUND`].
///
/// Only that condition: a `RestoreTestingPlanName` that names no plan of this
/// account is a DIFFERENT condition, which the plan's own refusal states
/// ([`super::restore_testing_plan_not_found`]), so a client can tell "no such
/// selection of this plan" from "no such plan".
pub fn restore_testing_selection_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        code: Some(ERROR_CODE_RESTORE_TESTING_SELECTION_NOT_FOUND.to_string()),
        message: Some(MESSAGE_RESTORE_TESTING_SELECTION_NOT_FOUND.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a create naming a restore testing selection the
/// named plan already holds, with the condition's own token and the sentence
/// built from the plan and selection the create named.
///
/// `Arn` and `CreatorRequestId` are left UNSET for the reason given on
/// [`super::vault_already_exists`].
pub fn restore_testing_selection_already_exists(
    plan_name: &str,
    selection_name: &str,
) -> AlreadyExistsException {
    AlreadyExistsException {
        code: Some(ERROR_CODE_RESTORE_TESTING_SELECTION_ALREADY_EXISTS.to_string()),
        message: Some(restore_testing_selection_already_exists_message(
            plan_name,
            selection_name,
        )),
        ..Default::default()
    }
}

/// The modelled refusal for a selection create that states neither of the two
/// members a selection selects its resources with.
///
/// A selection "consists of RestoreTestingSelectionName, ProtectedResourceType,
/// and one of the following: ProtectedResourceArns ProtectedResourceConditions"
/// (model doc for `CreateRestoreTestingSelection`), and neither member carries
/// the `@required` trait, so the boundary validator passes a request stating
/// neither while a selection that states neither selects no resource at all. The
/// sentence is the one this service states for the same class of condition — a
/// create whose selection selects nothing
/// ([`super::MESSAGE_MISSING_RESOURCE_SELECTION`]).
pub fn missing_protected_resource_selection() -> MissingParameterValueException {
    MissingParameterValueException {
        message: Some(super::MESSAGE_MISSING_RESOURCE_SELECTION.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a selection naming more specific protected resource
/// ARNs than one may: "you can include up to 30 specific protected resource ARNs
/// in ProtectedResourceArns" (model doc for the operation), which is "a maximum
/// number of items allowed in a request" — the condition the modelled
/// `LimitExceededException` is documented for.
pub fn too_many_protected_resource_arns() -> LimitExceededException {
    LimitExceededException {
        message: Some(format!(
            "ProtectedResourceArns cannot contain more than \
             {MAX_PROTECTED_RESOURCE_ARNS} protected resource ARNs"
        )),
        ..Default::default()
    }
}

/// Whether `value` is a name a restore testing selection can be addressed by:
/// "The name consists of only alphanumeric characters and underscores. Maximum
/// length is 50" (model doc for
/// `RestoreTestingSelectionForCreate$RestoreTestingSelectionName`).
///
/// The member targets a plain `String`, so the framework refuses nothing for it
/// before a handler runs: the rule is applied here instead, so no name the
/// service could never report is ever stored.
pub fn is_modelled_restore_testing_selection_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_RESTORE_TESTING_SELECTION_NAME_LENGTH
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Whether the `ProtectedResourceArns` a selection states names its resources by
/// WILDCARD — "a restore testing selection can include a wildcard value (\"*\")
/// for ProtectedResourceArns along with ProtectedResourceConditions" (model doc
/// for the operation), which is what makes conditions admissible beside it.
fn selects_by_wildcard(arns: &[String]) -> bool {
    arns.iter()
        .any(|arn| arn == PROTECTED_RESOURCE_ARNS_WILDCARD)
}

/// Why a restore testing selection body is not one AWS stores, as
/// [`rejected_restore_testing_selection_body`] decides it.
///
/// The three refusals are kept apart because they are three different modelled
/// errors: one states a value AWS does not accept, one that a member the
/// selection cannot do without was not stated, and one that the body is past a
/// documented limit.
#[derive(Debug)]
pub enum RejectedRestoreTestingSelectionBody {
    /// A member of the body carries a value AWS does not accept, as this
    /// modelled refusal states.
    Value(InvalidParameterValueException),
    /// The body selects no protected resource at all — see
    /// [`missing_protected_resource_selection`].
    SelectsNothing(MissingParameterValueException),
    /// The body names more specific protected resource ARNs than one selection
    /// may — see [`too_many_protected_resource_arns`].
    TooManyProtectedResourceArns(LimitExceededException),
}

/// Why the body a selection create carries is not one a restore testing
/// selection can be stored from, or `None` when it is one.
///
/// The ONE reading of a selection body, so every operation that stores one
/// applies the same documented rules and names the same member in its refusal:
///
/// * the name is alphanumeric and underscores up to 50 characters, and the
///   protected resource type and the role the restore runs as are stated — the
///   role as an ARN, which the member is typed as with no pattern, so the
///   framework refuses no other value for it;
/// * the resources are selected ONE way: "a restore testing selection can
///   include a wildcard value (\"*\") for ProtectedResourceArns along with
///   ProtectedResourceConditions. Alternatively, you can include up to 30
///   specific protected resource ARNs in ProtectedResourceArns. Cannot select by
///   both protected resource types AND specific ARNs. Request will fail if both
///   are included" (model doc for the operation), so the wildcard mixed into a
///   list of ARNs, and specific ARNs stated beside conditions, are both refused,
///   and more than [`MAX_PROTECTED_RESOURCE_ARNS`] specific ARNs is past the
///   documented limit;
/// * a selection selects its resources by one of those two means, so a body
///   that selects by neither selects nothing;
/// * a stated validation window is within [`MIN_VALIDATION_WINDOW_HOURS`] to
///   [`MAX_VALIDATION_WINDOW_HOURS`].
///
/// A collection the request states as EMPTY is a stated one — it is stored and
/// reported as the empty collection it is — but it selects no resource, which is
/// why a body whose only selection is empty is the missing-selection condition
/// rather than an invalid value.
pub fn rejected_restore_testing_selection_body(
    name: &str,
    protected_resource_type: &str,
    iam_role_arn: &str,
    protected_resource_arns: Option<&Vec<String>>,
    protected_resource_conditions: Option<&ProtectedResourceConditions>,
    validation_window_hours: i32,
) -> Option<RejectedRestoreTestingSelectionBody> {
    use RejectedRestoreTestingSelectionBody::{
        SelectsNothing, TooManyProtectedResourceArns, Value,
    };

    if !is_modelled_restore_testing_selection_name(name) {
        return Some(Value(super::rejected_parameter(
            "RestoreTestingSelectionName",
        )));
    }
    if protected_resource_type.is_empty() {
        return Some(Value(super::rejected_parameter("ProtectedResourceType")));
    }
    if let Some(refusal) = super::rejected_arn("IamRoleArn", iam_role_arn) {
        return Some(Value(refusal));
    }
    let arns: &[String] = protected_resource_arns.map_or(&[], Vec::as_slice);
    let wildcard = selects_by_wildcard(arns);
    let specific_arns = arns
        .iter()
        .filter(|arn| arn.as_str() != PROTECTED_RESOURCE_ARNS_WILDCARD)
        .count();
    let conditions = states_a_condition(protected_resource_conditions);
    // The wildcard and specific ARNs are two ways of naming the same member's
    // resources, and specific ARNs beside conditions are two ways of selecting
    // the selection's: "but not both" in either case.
    if wildcard && specific_arns > 0 {
        return Some(Value(super::rejected_parameter("ProtectedResourceArns")));
    }
    if specific_arns > 0 && conditions {
        return Some(Value(super::rejected_parameter("ProtectedResourceArns")));
    }
    if specific_arns > MAX_PROTECTED_RESOURCE_ARNS {
        return Some(TooManyProtectedResourceArns(
            too_many_protected_resource_arns(),
        ));
    }
    if !wildcard && specific_arns == 0 && !conditions {
        return Some(SelectsNothing(missing_protected_resource_selection()));
    }
    if !(MIN_VALIDATION_WINDOW_HOURS..=MAX_VALIDATION_WINDOW_HOURS)
        .contains(&validation_window_hours)
    {
        return Some(Value(super::rejected_parameter("ValidationWindowHours")));
    }
    None
}

/// Whether the `ProtectedResourceConditions` a selection states filters
/// protected resources at all: a conditions structure whose two lists are empty
/// or unstated matches on nothing, so it selects no resource and is not the
/// "along with ProtectedResourceConditions" the wildcard rule speaks of.
pub fn states_a_condition(conditions: Option<&ProtectedResourceConditions>) -> bool {
    conditions.is_some_and(|conditions| {
        conditions
            .string_equals
            .as_ref()
            .is_some_and(|pairs| !pairs.is_empty())
            || conditions
                .string_not_equals
                .as_ref()
                .is_some_and(|pairs| !pairs.is_empty())
    })
}

/// Why the body an UPDATE carries is not one the stored selection can be
/// rewritten from, or `None` when it is one.
///
/// An update states only the members it changes, so the rules are read over the
/// selection the update would leave behind — each member as the body states it,
/// else as the selection already holds it — through the SAME reading that
/// admitted the create's body
/// ([`rejected_restore_testing_selection_body`]), so a selection an update
/// rewrote can be no selection a create could not have stored. That is what
/// states the documented rule for this operation: "you can use either protected
/// resource ARNs or conditions, but not both. That is, if your selection has
/// ProtectedResourceArns, requesting an update with the parameter
/// ProtectedResourceConditions will be unsuccessful" (model doc for
/// `UpdateRestoreTestingSelection`) — the merged selection names specific ARNs
/// beside conditions, which the reading refuses.
///
/// The name is not read: it addresses the selection rather than being rewritten
/// ("most elements except the RestoreTestingSelectionName can be updated", the
/// same doc), and it is the stored one, which was admitted when it was stored.
pub fn rejected_restore_testing_selection_update(
    stored: &RestoreTestingSelectionRecord,
    body: &RestoreTestingSelectionForUpdate,
) -> Option<RejectedRestoreTestingSelectionBody> {
    let iam_role_arn = body
        .iam_role_arn
        .clone()
        .unwrap_or_else(|| stored.iam_role_arn.clone());
    let protected_resource_arns = body
        .protected_resource_arns
        .clone()
        .unwrap_or_else(|| stored.protected_resource_arns.clone());
    let protected_resource_conditions = stored_protected_resource_conditions(Some(
        body.protected_resource_conditions
            .clone()
            .unwrap_or_else(|| stored.protected_resource_conditions.clone()),
    ));
    let validation_window_hours = if body.validation_window_hours == 0 {
        stored.validation_window_hours
    } else {
        body.validation_window_hours
    };
    rejected_restore_testing_selection_body(
        &stored.name,
        &stored.protected_resource_type,
        &iam_role_arn,
        Some(&protected_resource_arns),
        Some(&protected_resource_conditions),
        validation_window_hours,
    )
}

impl ComAmazonawsBackupCryocontrollerusermanagerState {
    /// Resolve the restore testing selection a (`RestoreTestingPlanName`,
    /// `RestoreTestingSelectionName`) pair addresses, or the modelled refusal
    /// for one this account does not hold.
    ///
    /// The ONE resolution step every call that names a selection shares, so none
    /// of them can answer a different class or a different sentence for the same
    /// condition. The plan is resolved FIRST, so a name no plan of this account
    /// carries answers the plan's not-found condition rather than the
    /// selection's.
    pub fn stored_restore_testing_selection(
        &self,
        plan_name: &str,
        selection_name: &str,
    ) -> Result<&RestoreTestingSelectionRecord, ResourceNotFoundException> {
        self.stored_restore_testing_plan(plan_name)?;
        self.restore_testing_selections
            .get(&(plan_name.to_string(), selection_name.to_string()))
            .ok_or_else(restore_testing_selection_not_found)
    }

    /// Resolve the restore testing selection a (`RestoreTestingPlanName`,
    /// `RestoreTestingSelectionName`) pair addresses FOR WRITING, or the
    /// modelled refusal for one this account does not hold — the same
    /// resolution, and so the same refusals in the same order, every read of a
    /// selection answers.
    pub fn stored_restore_testing_selection_mut(
        &mut self,
        plan_name: &str,
        selection_name: &str,
    ) -> Result<&mut RestoreTestingSelectionRecord, ResourceNotFoundException> {
        self.stored_restore_testing_plan(plan_name)?;
        self.restore_testing_selections
            .get_mut(&restore_testing_selection_key(plan_name, selection_name))
            .ok_or_else(restore_testing_selection_not_found)
    }

    /// Take the restore testing selection a (`RestoreTestingPlanName`,
    /// `RestoreTestingSelectionName`) pair addresses OUT of the store, or answer
    /// the modelled refusal for a plan this account does not hold.
    ///
    /// The ONE removal step, so the resource a delete reports gone is gone from
    /// the store every later read and mutation of that pair resolves against:
    /// after it, the pair answers the selection's not-found condition
    /// ([`restore_testing_selection_not_found`]) and the name is free for a
    /// create again. The plan is resolved FIRST, for the same reason
    /// [`Self::stored_restore_testing_selection`] resolves it first.
    ///
    /// Once the plan is resolved the removal is IDEMPOTENT: `None` says the
    /// named plan held no such selection, which is the state the caller asked
    /// for, so a repeat of the call is not a different answer.
    pub fn remove_restore_testing_selection(
        &mut self,
        plan_name: &str,
        selection_name: &str,
    ) -> Result<Option<RestoreTestingSelectionRecord>, ResourceNotFoundException> {
        self.stored_restore_testing_plan(plan_name)?;
        Ok(self
            .restore_testing_selections
            .remove(&restore_testing_selection_key(plan_name, selection_name)))
    }

    /// Every selection the named plan holds, in name order — the one traversal
    /// of this plan's share of the store, which is a contiguous range of it
    /// because the map is keyed plan-first.
    pub fn restore_testing_selections_of_plan<'a>(
        &'a self,
        plan_name: &str,
    ) -> impl Iterator<Item = &'a RestoreTestingSelectionRecord> + 'a {
        let plan = plan_name.to_string();
        let first = (plan.clone(), String::new());
        self.restore_testing_selections
            .range((std::ops::Bound::Included(first), std::ops::Bound::Unbounded))
            .take_while(move |((held, _), _)| *held == plan)
            .map(|(_, selection)| selection)
    }

    /// The selection a repeated `CreatorRequestId` replays: the token
    /// "identifies the request and allows failed requests to be retried without
    /// the risk of running the operation twice. If the request includes a
    /// CreatorRequestId that matches an existing [resource], that [resource] is
    /// returned" (model doc for
    /// `RestoreTestingSelectionForGet$CreatorRequestId`), so a retry carrying
    /// the token of a create that already stored a selection answers with that
    /// selection rather than storing a second one.
    ///
    /// Scoped to the plan the create addresses: the create is bound to one
    /// `RestoreTestingPlanName` (the model binds it to that path), so the call a
    /// token identifies is a call on that plan.
    pub fn restore_testing_selection_by_creator_request_id(
        &self,
        plan_name: &str,
        creator_request_id: &str,
    ) -> Option<&RestoreTestingSelectionRecord> {
        self.restore_testing_selections_of_plan(plan_name)
            .find(|selection| selection.creator_request_id.as_deref() == Some(creator_request_id))
    }

    /// How many selections the named plan already holds, which the per-plan
    /// ceiling is measured against.
    pub fn restore_testing_selection_count(&self, plan_name: &str) -> usize {
        self.restore_testing_selections_of_plan(plan_name).count()
    }
}

/// The key a selection is stored under: the plan it belongs to, then its own
/// name, so one plan's selections are contiguous in the map and a read of them
/// is a range of this one store rather than a second index beside it.
pub fn restore_testing_selection_key(plan_name: &str, selection_name: &str) -> (String, String) {
    (plan_name.to_string(), selection_name.to_string())
}
