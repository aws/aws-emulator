// SPDX-License-Identifier: CC0-1.0

//! The RESTORE TESTING side of this service's state: the plan
//! `CreateRestoreTestingPlan` stores, and what every call that names a
//! `RestoreTestingPlanName` resolves against.
//!
//! # The rules this module holds
//!
//! * a restore testing plan is addressed by NAME — the name "cannot be changed
//!   after creation" (model doc for `RestoreTestingPlanName`) — and that name is
//!   unique to the account: the store is keyed by it, so the first create of a
//!   name stores the one plan that name has and a create naming one the account
//!   already holds is refused as already existing
//!   ([`super::restore_testing_plan_already_exists`]);
//! * a name the account holds no plan under is the not-found condition
//!   ([`super::restore_testing_plan_not_found`]), which every read of a plan
//!   answers;
//! * the plan body a create carries is stored as submitted, except for the three
//!   members the model gives a default for, which the service fills where the
//!   request states none ([`DEFAULT_START_WINDOW_HOURS`],
//!   [`DEFAULT_SELECTION_WINDOW_DAYS`] and an empty `ExcludeVaults`), and is
//!   reported back through the ONE read projection below, so the plan a create
//!   stored and the plan a read reports can never differ.

use std::collections::BTreeMap;
use std::time::SystemTime;

use super::ComAmazonawsBackupCryocontrollerusermanagerState;
use crate::ops::types::{
    InvalidParameterValueException, RestoreTestingPlanForGet,
    RestoreTestingRecoveryPointSelection,
};

/// The most restore testing plans this emulator keeps.
///
/// An EMULATOR-CHOSEN ceiling, not an AWS quota, for the reason given on
/// [`super::MAX_VAULTS`]: the store is in memory and every request can add to
/// it. AWS caps the restore testing plans an account may hold too, and answers
/// the same modelled `LimitExceededException` for a create past it.
pub const MAX_RESTORE_TESTING_PLANS: usize = 500;

/// The start window a plan runs with when its create states none: "Defaults to
/// 24 hours" (model doc for `RestoreTestingPlanForCreate$StartWindowHours`).
pub const DEFAULT_START_WINDOW_HOURS: i32 = 24;

/// The least hours a stated start window may be, and the most: "If this value
/// is included, this parameter has a maximum value of 168 hours (one week)"
/// (model doc for `RestoreTestingPlanForCreate$StartWindowHours`), which a
/// window of hours is counted from one.
pub const MIN_START_WINDOW_HOURS: i32 = 1;
pub const MAX_START_WINDOW_HOURS: i32 = 168;

/// The selection window a plan picks recovery points within when its create
/// states none: "If not included, the value defaults to 30" (model doc for
/// `RestoreTestingRecoveryPointSelection$SelectionWindowDays`).
pub const DEFAULT_SELECTION_WINDOW_DAYS: i32 = 30;

/// The least and the most days a stated selection window may be: "Accepted
/// values are integers from 1 to 365" (model doc for the member).
pub const MIN_SELECTION_WINDOW_DAYS: i32 = 1;
pub const MAX_SELECTION_WINDOW_DAYS: i32 = 365;

/// The most characters a `RestoreTestingPlanName` may carry: "Maximum length is
/// 50" (model doc for `CreateRestoreTestingPlanOutput$RestoreTestingPlanName`).
pub const MAX_RESTORE_TESTING_PLAN_NAME_LENGTH: usize = 50;

/// A restore testing plan: what `CreateRestoreTestingPlan` stores and
/// `GetRestoreTestingPlan` reports.
///
/// The body is kept as the create submitted it, so a read reports what was
/// stored rather than a value recomputed per call; the two instants only this
/// service can know — when the plan was stored, and when an update last rewrote
/// it — are its own members.
#[derive(Debug, Clone)]
pub struct RestoreTestingPlanRecord {
    pub name: String,
    pub arn: String,
    /// Which recovery points a run of this plan restores, stored as submitted
    /// with the selection window filled in.
    pub recovery_point_selection: RestoreTestingRecoveryPointSelection,
    /// The cron expression a run of this plan is initiated by.
    pub schedule_expression: String,
    /// The zone that expression is read in; `None` leaves it in UTC, which is
    /// what the model documents for an absent one.
    pub schedule_expression_timezone: Option<String>,
    pub start_window_hours: i32,
    pub creator_request_id: Option<String>,
    pub creation_time: SystemTime,
    /// When an update last rewrote this plan; `None` until one has.
    pub last_update_time: Option<SystemTime>,
    /// When a restore test last ran with this plan; `None` until one has.
    pub last_execution_time: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
}

impl RestoreTestingPlanRecord {
    /// The plan as a READ reports it — the ONE projection of a stored plan, so a
    /// read can report nothing the create did not store.
    pub fn for_get(&self) -> RestoreTestingPlanForGet {
        RestoreTestingPlanForGet {
            restore_testing_plan_name: self.name.clone(),
            restore_testing_plan_arn: self.arn.clone(),
            recovery_point_selection: self.recovery_point_selection.clone(),
            schedule_expression: self.schedule_expression.clone(),
            schedule_expression_timezone: self.schedule_expression_timezone.clone(),
            start_window_hours: self.start_window_hours,
            creator_request_id: self.creator_request_id.clone(),
            creation_time: ::aws_smithy_types::DateTime::from(self.creation_time),
            last_update_time: self.last_update_time.map(::aws_smithy_types::DateTime::from),
            last_execution_time: self
                .last_execution_time
                .map(::aws_smithy_types::DateTime::from),
        }
    }
}

/// Whether `value` is a name a restore testing plan can be addressed by: "it
/// must consist of only alphanumeric characters and underscores" and "maximum
/// length is 50" (model doc for `RestoreTestingPlanForCreate$RestoreTestingPlanName`).
///
/// The member targets a plain `String`, so the framework refuses nothing for it
/// before a handler runs: the rule is applied here instead, so no name the
/// service could never report is ever stored.
pub fn is_modelled_restore_testing_plan_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_RESTORE_TESTING_PLAN_NAME_LENGTH
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Why the body a create or an update carries is not one a restore testing plan
/// can be stored from, or `None` when it is one.
///
/// The ONE reading of a plan body, so every operation that stores one applies
/// the same documented rules and names the same member in its refusal: the name
/// is alphanumeric and underscores up to 50 characters, the schedule is a cron
/// expression this service can compute a run from
/// ([`super::schedule::parse_schedule_expression`]), a stated start window is
/// within [`MIN_START_WINDOW_HOURS`] to [`MAX_START_WINDOW_HOURS`] and a stated
/// selection window is within [`MIN_SELECTION_WINDOW_DAYS`] to
/// [`MAX_SELECTION_WINDOW_DAYS`].
///
/// A window the request states NOTHING for is not a stated one: it reaches this
/// service as the modelled default of its integer member (`0`), which is the
/// value [`stored_start_window_hours`] and
/// [`stored_restore_testing_recovery_point_selection`] replace with the
/// documented default.
pub fn rejected_restore_testing_plan_body(
    name: Option<&str>,
    schedule_expression: Option<&str>,
    start_window_hours: i32,
    selection_window_days: i32,
) -> Option<InvalidParameterValueException> {
    if name.is_some_and(|name| !is_modelled_restore_testing_plan_name(name)) {
        return Some(super::rejected_parameter("RestoreTestingPlanName"));
    }
    if schedule_expression
        .is_some_and(|expression| super::schedule::parse_schedule_expression(expression).is_none())
    {
        return Some(super::rejected_parameter("ScheduleExpression"));
    }
    if start_window_hours != 0
        && !(MIN_START_WINDOW_HOURS..=MAX_START_WINDOW_HOURS).contains(&start_window_hours)
    {
        return Some(super::rejected_parameter("StartWindowHours"));
    }
    if selection_window_days != 0
        && !(MIN_SELECTION_WINDOW_DAYS..=MAX_SELECTION_WINDOW_DAYS)
            .contains(&selection_window_days)
    {
        return Some(super::rejected_parameter("SelectionWindowDays"));
    }
    None
}

/// The two members of a stored plan the model gives a DEFAULT for, filled where
/// the request stated neither: the selection the plan picks recovery points
/// with, as submitted otherwise.
///
/// * `SelectionWindowDays` reaches this service as the modelled default of its
///   integer member (`0`) when the request states nothing for it, and `0` is
///   outside the member's documented range, so it is exactly the "not included"
///   case [`DEFAULT_SELECTION_WINDOW_DAYS`] is documented for;
/// * `ExcludeVaults` "defaults to empty list if not listed" (model doc for the
///   member), so a plan stored from a request that listed none excludes no
///   vault, and a read of it reports that empty list rather than nothing.
pub fn stored_restore_testing_recovery_point_selection(
    selection: RestoreTestingRecoveryPointSelection,
) -> RestoreTestingRecoveryPointSelection {
    let selection_window_days = if selection.selection_window_days == 0 {
        DEFAULT_SELECTION_WINDOW_DAYS
    } else {
        selection.selection_window_days
    };
    RestoreTestingRecoveryPointSelection {
        selection_window_days,
        exclude_vaults: Some(selection.exclude_vaults.unwrap_or_default()),
        ..selection
    }
}

/// The start window a stored plan carries: the stated value, or
/// [`DEFAULT_START_WINDOW_HOURS`] where the request stated none — which reaches
/// this service as the modelled default of the member (`0`), for the reason
/// given above.
pub fn stored_start_window_hours(start_window_hours: i32) -> i32 {
    if start_window_hours == 0 {
        DEFAULT_START_WINDOW_HOURS
    } else {
        start_window_hours
    }
}

impl ComAmazonawsBackupCryocontrollerusermanagerState {
    /// Resolve the restore testing plan a `RestoreTestingPlanName` addresses, or
    /// the modelled refusal for one this account does not hold.
    ///
    /// The ONE resolution step every call that names a plan shares, so none of
    /// them can answer a different class or a different sentence for the same
    /// condition.
    pub fn stored_restore_testing_plan(
        &self,
        name: &str,
    ) -> Result<&RestoreTestingPlanRecord, crate::ops::types::ResourceNotFoundException> {
        self.restore_testing_plans
            .get(name)
            .ok_or_else(super::restore_testing_plan_not_found)
    }

    /// The plan a repeated `CreatorRequestId` replays: "This identifies the
    /// request and allows failed requests to be retried without the risk of
    /// running the operation twice. If the request includes a
    /// `CreatorRequestId` that matches an existing backup plan, that plan is
    /// returned" (model doc for the member), so a retry carrying the token of a
    /// create that already stored a plan answers with that plan rather than
    /// storing a second one.
    pub fn restore_testing_plan_by_creator_request_id(
        &self,
        creator_request_id: &str,
    ) -> Option<&RestoreTestingPlanRecord> {
        self.restore_testing_plans
            .values()
            .find(|plan| plan.creator_request_id.as_deref() == Some(creator_request_id))
    }
}
