// SPDX-License-Identifier: CC0-1.0

//! The scheduled actions an account created: the record each one is, the
//! expression its schedule is read under, the Amazon Redshift operation it
//! triggers, and the invocation times a read of it reports.
//!
//! A scheduled action "triggers some Amazon Redshift API operations on a
//! schedule" (`smithy_query(query="shape", shape="ScheduledAction")`), so the
//! schedule it carries is what every upcoming invocation of it is COMPUTED
//! from — one reading of it ([`parse_scheduled_action_schedule`]) decides both
//! whether an expression is one the service admits at all and when the schedule
//! it states fires, so no stored action carries a schedule nothing can be
//! computed from.
//!
//! The member a read reports that is NOT stored — `ScheduledAction$NextInvocations` —
//! is derived here ([`StoredScheduledAction::view`]) from the schedule, the
//! action's state and the window it is active in, so the create path, the modify
//! path and the read path cannot report a different action.

use super::*;

use super::snapshot_schedules::{expression_body, parse_cron_body, system_time, utc_datetime};
use crate::ops::types::{CreateScheduledActionOutput, ScheduledActionType};

/// The most scheduled actions this store holds, past which
/// `CreateScheduledAction` refuses with the modelled
/// `ScheduledActionQuotaExceededFault`.
///
/// The same reasoning as [`SNAPSHOT_SCHEDULE_QUOTA`]: the emulator's own bound on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::scheduled_actions`],
/// not a claim about an account's published limit. An action's identity is the
/// name its create gives it, and names are unlimited in number, so this bound is
/// what stops the collection growing without end; it is set far above any
/// workflow an emulator serves.
pub const SCHEDULED_ACTION_QUOTA: usize = 10_000;

/// `ScheduledAction$State` of an action that triggers.
///
/// Source: `smithy_query(query="shape", shape="ScheduledActionState")`, whose
/// values are `ACTIVE` and `DISABLED`, with
/// `CreateScheduledActionMessage$Enable` — "If true, the schedule is enabled. If
/// false, the scheduled action does not trigger" — deciding which of them an
/// action carries.
pub const SCHEDULED_ACTION_STATE_ACTIVE: &str = "ACTIVE";

/// `ScheduledAction$State` of an action that does not trigger. See
/// [`SCHEDULED_ACTION_STATE_ACTIVE`].
pub const SCHEDULED_ACTION_STATE_DISABLED: &str = "DISABLED";

/// Whether an action whose request named no `Enable` triggers.
///
/// Source: `smithy_query(query="doc", shape="CreateScheduledActionMessage",
/// member="Enable")` states no default. An action created to run is the one a
/// caller that said nothing about enabling asked for — the schedule is the
/// reason the call was made — so an action is active unless the request turned
/// it off.
pub const SCHEDULED_ACTION_ENABLED_BY_DEFAULT: bool = true;

/// The `at(...)` expression type: a one-time schedule.
///
/// Source: `smithy_query(query="doc", shape="ScheduledAction",
/// member="Schedule")` — "The schedule for a one-time (at format) or recurring
/// (cron format) scheduled action", "Format of at expressions is
/// `at(yyyy-mm-ddThh:mm:ss)`".
const AT_EXPRESSION: &str = "at";

/// The date-and-time format the body of an `at(...)` expression is written in.
///
/// Source: the same doc as [`AT_EXPRESSION`] — "`at(2016-03-04T17:27:00)`",
/// which carries no zone, because the member states the instant "in UTC".
const AT_INSTANT_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

/// How many upcoming invocations a scheduled action reports.
///
/// `ScheduledAction$NextInvocations` is "a list of times when the scheduled
/// action will run" and no member of any request names how many of them to
/// report, so the number is this service's own — the next time the action fires,
/// which is what a caller polling an action reads. The same choice, for the same
/// reason, as [`DEFAULT_REPORTED_NEXT_INVOCATIONS`].
pub const REPORTED_SCHEDULED_ACTION_INVOCATIONS: usize = 1;

/// The schedule a scheduled action runs on, read as the instants it fires at.
///
/// The two expression types a schedule may be written in (see
/// [`parse_scheduled_action_schedule`]), each carrying what the invocation times
/// are computed from: one instant for an `at` expression and a parsed calendar
/// schedule for a `cron` one.
#[derive(Debug, Clone)]
pub enum ScheduledActionSchedule {
    /// `at(yyyy-mm-ddThh:mm:ss)`: the action runs once, at that instant.
    At(SystemTime),
    /// `cron(...)`: the action runs at the calendar instants the expression
    /// names. Boxed, because a parsed schedule is much larger than an instant.
    Cron(Box<::cron::Schedule>),
}

impl ScheduledActionSchedule {
    /// The first `count` instants this schedule fires at, strictly after
    /// `after`.
    ///
    /// Every conversion is fallible on purpose: a schedule came from a caller's
    /// request, so an instant outside the range this service computes in drops
    /// out of the report instead of being reported as some other instant.
    #[must_use]
    pub fn upcoming(&self, after: SystemTime, count: usize) -> Vec<SystemTime> {
        if count == 0 {
            return Vec::new();
        }
        match self {
            // A one-time schedule has exactly one invocation, and only while it
            // is still ahead: an action whose instant has passed will not run
            // again.
            Self::At(at) => {
                if *at > after {
                    vec![*at]
                } else {
                    Vec::new()
                }
            }
            Self::Cron(schedule) => {
                let Some(after) = utc_datetime(after) else {
                    return Vec::new();
                };
                schedule
                    .after(&after)
                    .take(count)
                    .filter_map(system_time)
                    .collect()
            }
        }
    }
}

/// One `Schedule` of a scheduled action, read as the schedule it states, or why
/// it is not a schedule this service stores an action with.
///
/// The schedule is "for a one-time (at format) or recurring (cron format)
/// scheduled action" (`smithy_query(query="doc", shape="ScheduledAction",
/// member="Schedule")`), so those two expression types are the ones a schedule is
/// read from and any other is [`ScheduleDefinitionRefusal::Unsupported`] — a
/// parameter value this service cannot honour at all, which is what makes it a
/// different refusal from one written in an offered type that states no schedule.
/// An `at` expression names one UTC instant in the documented format; a `cron`
/// expression names the calendar instants of the documented AWS cron format,
/// read in UTC through the same reading the snapshot schedules use
/// ([`parse_cron_body`]).
pub fn parse_scheduled_action_schedule(
    schedule: &str,
) -> Result<ScheduledActionSchedule, ScheduleDefinitionRefusal> {
    let expression = schedule.trim();
    if let Some(body) = expression_body(expression, AT_EXPRESSION) {
        let invalid = || ScheduleDefinitionRefusal::Invalid(invalid_schedule_message(expression));
        let naive =
            ::chrono::NaiveDateTime::parse_from_str(body.trim(), AT_INSTANT_FORMAT)
                .map_err(|_| invalid())?;
        let at = system_time(naive.and_utc()).ok_or_else(invalid)?;
        return Ok(ScheduledActionSchedule::At(at));
    }
    if let Some(body) = expression_body(expression, CRON_EXPRESSION) {
        let parsed = parse_cron_body(body).ok_or_else(|| {
            ScheduleDefinitionRefusal::Invalid(invalid_schedule_message(expression))
        })?;
        return Ok(ScheduledActionSchedule::Cron(Box::new(parsed)));
    }
    Err(ScheduleDefinitionRefusal::Unsupported(
        unsupported_schedule_expression_message(expression),
    ))
}

/// Why an expression written in neither schedule type is refused, naming the
/// expression the request sent and both formats the member documents.
///
/// Amazon Redshift answers such a request with the `InvalidParameterValue` code
/// — the value of `Schedule` is one it cannot honour, decided on the request
/// alone before anything is looked up — so the sentence is worded the way this
/// service words every parameter it cannot honour (see
/// [`next_invocations_out_of_range_message`]): the member, the value, and what
/// the caller has to send instead.
pub fn unsupported_schedule_expression_message(expression: &str) -> String {
    format!(
        "Invalid value for Schedule: '{expression}'. Schedule must be an at(yyyy-mm-ddThh:mm:ss) or cron(Minutes Hours Day-of-month Month Day-of-week Year) expression."
    )
}

/// The `ScheduledActionFilterName` that filters on the cluster an action's
/// target action names.
///
/// Source: `smithy_query(query="shape", shape="ScheduledActionFilterName")`,
/// whose values are `cluster-identifier` and `iam-role` — "the type of element
/// to filter".
pub const SCHEDULED_ACTION_FILTER_CLUSTER_IDENTIFIER: &str = "cluster-identifier";

/// The `ScheduledActionFilterName` that filters on the role an action assumes.
/// See [`SCHEDULED_ACTION_FILTER_CLUSTER_IDENTIFIER`].
pub const SCHEDULED_ACTION_FILTER_IAM_ROLE: &str = "iam-role";

/// The `ScheduledActionTypeValues` the three operations the scheduler supports
/// are named by, in the order the `ScheduledActionType` shape declares its
/// members.
///
/// Source: `smithy_query(query="shape", shape="ScheduledActionTypeValues")` —
/// `ResizeCluster`, `PauseCluster`, `ResumeCluster` — which is the same set
/// `ScheduledActionType` carries one member per.
pub const SCHEDULED_ACTION_TARGETS: [&str; 3] = ["ResizeCluster", "PauseCluster", "ResumeCluster"];

/// Which operation a `TargetAction` triggers and which cluster it names, or
/// `None` when it does not name exactly one operation.
///
/// "The action type that specifies an Amazon Redshift API operation that is
/// supported by the Amazon Redshift scheduler" (`smithy_query(query="shape",
/// shape="ScheduledActionType")`): the shape carries one member per supported
/// operation and an action triggers ONE of them, so a `TargetAction` that names
/// none, or more than one, states no action to run.
#[must_use]
pub fn scheduled_action_target(target: &ScheduledActionType) -> Option<(&'static str, &str)> {
    let named: Vec<(&'static str, &str)> = [
        (
            SCHEDULED_ACTION_TARGETS[0],
            target
                .resize_cluster
                .as_ref()
                .map(|action| action.cluster_identifier.as_str()),
        ),
        (
            SCHEDULED_ACTION_TARGETS[1],
            target
                .pause_cluster
                .as_ref()
                .map(|action| action.cluster_identifier.as_str()),
        ),
        (
            SCHEDULED_ACTION_TARGETS[2],
            target
                .resume_cluster
                .as_ref()
                .map(|action| action.cluster_identifier.as_str()),
        ),
    ]
    .into_iter()
    .filter_map(|(name, cluster)| cluster.map(|cluster| (name, cluster)))
    .collect();
    match named.as_slice() {
        [one] => Some(*one),
        _ => None,
    }
}

/// Why a `TargetAction` that does not name exactly one supported operation is
/// refused.
///
/// The fixed text is what the model states the fault is
/// (`smithy_query(query="shape", shape="InvalidScheduledActionFault")` — "The
/// scheduled action is not valid"), with what the caller has to send named after
/// it: the member and the operations it may carry one of.
pub fn invalid_scheduled_action_message() -> String {
    format!(
        "The scheduled action is not valid: TargetAction must name exactly one of {}.",
        SCHEDULED_ACTION_TARGETS.join(", ")
    )
}

/// Why a create under a name the account already holds a scheduled action under
/// is refused, naming the action the request asked for.
///
/// The NAME "must be unique within an account"
/// (`smithy_query(query="doc", shape="CreateScheduledActionMessage",
/// member="ScheduledActionName")`), so the refusal names it — single-quoted, as
/// this family words an identifier it reports back inside a sentence (see
/// [`snapshot_schedule_already_exists_message`]).
pub fn scheduled_action_already_exists_message(name: &str) -> String {
    format!(
        "Cannot create the scheduled action because an action with the name '{name}' already exists."
    )
}

/// Why a create past [`SCHEDULED_ACTION_QUOTA`] is refused.
pub fn scheduled_action_quota_message() -> String {
    format!(
        "The request would exceed the allowed number of scheduled actions ({SCHEDULED_ACTION_QUOTA})."
    )
}

/// Why an operation naming a scheduled action this account does not hold is
/// refused, naming the action the request asked about.
///
/// The sentence follows the family Amazon Redshift words a missing NAMED
/// resource an operation ACTS on with — the shape's own name, then
/// `not found: `, then the name the request sent — exactly as a missing cluster
/// and a missing cluster snapshot are worded
/// ([`ClusterNotFoundWording::NotFoundThenIdentifier`],
/// [`ClusterSnapshotNotFoundWording::NotFoundThenIdentifier`]), and says what the
/// model states the fault is (`smithy_query(query="shape",
/// shape="ScheduledActionNotFoundFault")` — "The scheduled action cannot be
/// found") about the name the caller sent.
pub fn scheduled_action_not_found_message(name: &str) -> String {
    format!("ScheduledAction not found: {name}")
}

/// A scheduled action as this service stores it: what the create and the modifies
/// gave it, and nothing a read derives.
///
/// The modelled `ScheduledAction` shape is NOT the whole record and is not stored
/// as one: its `NextInvocations` is computed when the action is reported, from
/// the schedule below and the window the action is active in
/// ([`StoredScheduledAction::view`]). Storing it would let an action report
/// invocation times that have already passed.
#[derive(Debug, Clone)]
pub struct StoredScheduledAction {
    /// The name the create gave the action, which is what an account's actions
    /// are unique on.
    pub scheduled_action_name: String,
    /// The Amazon Redshift operation the action triggers, with the input it
    /// triggers it with.
    pub target_action: ScheduledActionType,
    /// The schedule expression the action runs on, exactly as the request wrote
    /// it: `Schedule` is reported back verbatim, so the characters the caller
    /// sent are kept rather than a re-rendering of the schedule they state.
    pub schedule: String,
    /// The role the action assumes to run its target operation.
    pub iam_role: String,
    /// The description the last write gave the action, and `None` when none gave
    /// one.
    pub scheduled_action_description: Option<String>,
    /// Whether the action triggers ([`SCHEDULED_ACTION_STATE_ACTIVE`]) or not
    /// ([`SCHEDULED_ACTION_STATE_DISABLED`]).
    pub state: String,
    /// When the schedule becomes active, and `None` when it is active from the
    /// moment it was created.
    pub start_time: Option<::aws_smithy_types::DateTime>,
    /// When the schedule stops being active, and `None` when it never does.
    pub end_time: Option<::aws_smithy_types::DateTime>,
}

impl StoredScheduledAction {
    /// This action as a response reports it at `now`: the stored record plus the
    /// invocations still ahead of it.
    ///
    /// ONE rule for the create, the modify and the read, so no two of them can
    /// report the same action differently.
    #[must_use]
    pub fn view(&self, now: SystemTime) -> CreateScheduledActionOutput {
        CreateScheduledActionOutput {
            scheduled_action_name: Some(self.scheduled_action_name.clone()),
            target_action: Some(self.target_action.clone()),
            schedule: Some(self.schedule.clone()),
            iam_role: Some(self.iam_role.clone()),
            scheduled_action_description: self.scheduled_action_description.clone(),
            state: Some(self.state.clone()),
            next_invocations: Some(self.next_invocations(now)),
            start_time: self.start_time,
            end_time: self.end_time,
        }
    }

    /// When this action will run next, as of `now`.
    ///
    /// Three rules of the model decide it, and all three are the action's own:
    /// "if false, the scheduled action does not trigger"
    /// (`ScheduledAction$State`), "before this time, the scheduled action does
    /// not trigger" (`$StartTime`) and "after this time, the scheduled action
    /// does not trigger" (`$EndTime`). So a disabled action reports no upcoming
    /// invocation at all, and an enabled one reports only instants inside its
    /// window.
    #[must_use]
    pub fn next_invocations(&self, now: SystemTime) -> Vec<::aws_smithy_types::DateTime> {
        if self.state != SCHEDULED_ACTION_STATE_ACTIVE {
            return Vec::new();
        }
        let Ok(schedule) = parse_scheduled_action_schedule(&self.schedule) else {
            // Unreachable for a stored action: every write parsed its schedule
            // before storing it, so this is an action no caller could have
            // created, and it reports no invocation rather than failing a read.
            return Vec::new();
        };
        // The window opens at the later of now and the action's own start, so an
        // action that is not active yet reports the first instant after it
        // becomes active rather than one before.
        let after = self
            .start_time
            .and_then(|start| system_time(::chrono::DateTime::from_timestamp(start.secs(), 0)?))
            .map_or(now, |start| start.max(now));
        schedule
            .upcoming(after, REPORTED_SCHEDULED_ACTION_INVOCATIONS)
            .into_iter()
            .map(::aws_smithy_types::DateTime::from)
            .filter(|at| {
                self.end_time
                    .is_none_or(|end| at.secs() <= end.secs())
            })
            .collect()
    }
}

impl ComAmazonawsRedshiftRedshiftserviceversion20121201State {
    /// Where in [`Self::scheduled_actions`] the action named `name` sits, and
    /// `None` when this account holds none under that name.
    ///
    /// The name is matched EXACTLY: the model documents no folding, so an action
    /// asked about in another case is an action this account does not hold.
    #[must_use]
    pub fn scheduled_action_position(&self, name: &str) -> Option<usize> {
        self.scheduled_actions
            .iter()
            .position(|action| action.scheduled_action_name == name)
    }

    /// The scheduled action named `name`, and `None` when this account holds
    /// none under that name.
    #[must_use]
    pub fn scheduled_action(&self, name: &str) -> Option<&StoredScheduledAction> {
        self.scheduled_action_position(name)
            .map(|position| &self.scheduled_actions[position])
    }
}
