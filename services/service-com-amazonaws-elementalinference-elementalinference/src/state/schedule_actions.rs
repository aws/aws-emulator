// SPDX-License-Identifier: CC0-1.0

//! The SCHEDULE ACTIONS on a feed: configuration changes the feed applies to itself at a
//! time the caller named.
//!
//! "A schedule action applies configuration changes to the feed at a time that you
//! specify, in the same way as an UpdateFeed request" (`CreateScheduleAction`), so an
//! action lives ON the feed record ([`Feed::schedule_actions`]) rather than in a store of
//! its own: it is bounded by the feeds that exist, it leaves the store with the feed it
//! belongs to ("When you delete or archive a feed, Elemental Inference deletes all the
//! schedule actions on that feed", `DeleteScheduleAction`), and the change it makes is
//! written into the very outputs `GetFeed` reports.
//!
//! An action's status is the CLOCK's: it is `PENDING` "waiting for its scheduled time",
//! and when that time has passed the service applies its operations and the action
//! reports `COMPLETED`, or `FAILED` when it could not apply them. Nothing sleeps for it —
//! the due instant is stored on the action and materialised by the next call's settle
//! pass, exactly as a feed's own transitions are.

use super::*;

use crate::ops::types::{ScheduleAction, ScheduleActionError};

/// Value equality for a schedule action's start settings and its operations, which the
/// stored action is compared by: `CreateScheduleAction` is `@idempotent` and carries no
/// `@idempotencyToken`, so the request's own members are its key and a repeat of the
/// action a feed already holds has to be recognised as that same request once more. The
/// shapes are rendered from the model, which derives only `Debug`, `Clone` and `Default`
/// on them, so the comparison lives here beside the record it serves.
impl PartialEq for ScheduleActionStartConfiguration {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::fixedModeScheduleActionStartSettings(ours),
                Self::fixedModeScheduleActionStartSettings(theirs),
            ) => ours.scheduled_at == theirs.scheduled_at,
        }
    }
}

impl Eq for ScheduleActionStartConfiguration {}

impl PartialEq for ScheduleActionOperation {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::updateOutputStatus(ours), Self::updateOutputStatus(theirs)) => {
                ours.output_name == theirs.output_name && ours.status == theirs.status
            }
            (
                Self::updateClippingDataSourceConfiguration(ours),
                Self::updateClippingDataSourceConfiguration(theirs),
            ) => {
                ours.output_name == theirs.output_name
                    && ours.data_source_configuration == theirs.data_source_configuration
            }
            (
                Self::removeClippingDataSourceConfiguration(ours),
                Self::removeClippingDataSourceConfiguration(theirs),
            ) => ours.output_name == theirs.output_name,
            _ => false,
        }
    }
}

impl Eq for ScheduleActionOperation {}

/// The UTC instant, in epoch seconds, at which a start configuration runs its action.
///
/// `FixedModeScheduleActionStartSettings$scheduledAt` is "the UTC time at which the
/// action runs", and fixed mode is the only member the union declares ("Currently, only
/// fixed mode is supported", `ScheduleActionStartConfiguration`).
pub fn scheduled_at_secs(start: &ScheduleActionStartConfiguration) -> i64 {
    match start {
        ScheduleActionStartConfiguration::fixedModeScheduleActionStartSettings(settings) => {
            settings.scheduled_at.secs()
        }
    }
}

/// How long the service keeps a settled action before deleting it automatically.
///
/// DOCUMENTED: "Elemental Inference keeps an action whose status is COMPLETED or FAILED
/// for about 48 hours after its scheduled time, and then deletes it automatically"
/// (`ListScheduleActions`), so this duration is the service's own rather than chosen.
pub const SETTLED_ACTION_RETENTION: Duration = Duration::from_secs(48 * 60 * 60);

/// The furthest ahead this store holds a schedule action's due instant.
///
/// `FixedModeScheduleActionStartSettings$scheduledAt` states no range of its own ("for
/// the range of times that you can specify, see the AWS Elemental Inference User Guide")
/// and the member is a `Timestamp`, so a caller can name an instant arbitrarily far
/// ahead — further than a monotonic instant can represent. The wait is therefore clamped
/// to this span: an action named beyond it is still `PENDING` at every instant this
/// process can observe, which is what naming it that far ahead asks for, and no request
/// can make the due instant overflow.
const MAX_SCHEDULE_WAIT: Duration = Duration::from_secs(10 * 365 * 24 * 60 * 60);

/// The lifecycle statuses a schedule action record holds, as the modelled
/// `ScheduleActionStatus` enum declares them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleActionLifecycle {
    /// "The action is waiting for its scheduled time." (`ScheduleAction$status`)
    Pending,
    /// "Elemental Inference applied all the action's operations to the feed."
    Completed,
    /// "Elemental Inference couldn't apply the action."
    Failed,
}

impl ScheduleActionLifecycle {
    /// This status as the modelled `ScheduleActionStatus` enum spells it.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Completed => "COMPLETED",
            Self::Failed => "FAILED",
        }
    }
}

/// One schedule action on a feed.
#[derive(Debug, Clone)]
pub struct FeedScheduleAction {
    /// "The name of the schedule action. The name is unique within the feed."
    /// (`ScheduleAction$actionName`)
    pub action_name: String,
    pub start_configuration: ScheduleActionStartConfiguration,
    pub operations: Vec<ScheduleActionOperation>,
    pub status: ScheduleActionLifecycle,
    /// Why the action failed, reported "only when the status is FAILED"
    /// (`ScheduleAction$error`).
    pub error: Option<ScheduleActionError>,
    /// The instant the action's scheduled time falls due on this store's clock. The time
    /// the caller NAMED is a wall-clock instant, so the span to wait is the difference
    /// between that instant and the moment the action was created — a time already past
    /// is due at once.
    pub due_at: Instant,
    /// When a SETTLED action leaves the feed ([`SETTLED_ACTION_RETENTION`] after its
    /// scheduled time), and `None` while the action is still pending.
    pub retire_at: Option<Instant>,
}

impl FeedScheduleAction {
    /// The `ScheduleAction` projection every schedule-action response reports.
    pub fn to_wire(&self) -> ScheduleAction {
        ScheduleAction {
            action_name: self.action_name.clone(),
            start_configuration: self.start_configuration.clone(),
            operations: self.operations.clone(),
            status: self.status.as_wire().to_string(),
            error: self.error.clone(),
        }
    }

    /// The name of the output each of this action's operations targets, in order — what
    /// the service validates "the targets of the action" against when the action is
    /// created, and resolves again when the action runs.
    pub fn target_output_names(&self) -> Vec<&str> {
        self.operations
            .iter()
            .map(schedule_action_target_output)
            .collect()
    }
}

/// The output name one schedule action operation targets.
///
/// Every member of the `ScheduleActionOperation` union names one: `updateOutputStatus`
/// enables or disables it, `updateClippingDataSourceConfiguration` attaches a game to it
/// and `removeClippingDataSourceConfiguration` removes the game from it
/// (`smithy_query(query="shape",shape="ScheduleActionOperation")`).
pub fn schedule_action_target_output(operation: &ScheduleActionOperation) -> &str {
    match operation {
        ScheduleActionOperation::updateOutputStatus(update) => update.output_name.as_str(),
        ScheduleActionOperation::updateClippingDataSourceConfiguration(update) => {
            update.output_name.as_str()
        }
        ScheduleActionOperation::removeClippingDataSourceConfiguration(remove) => {
            remove.output_name.as_str()
        }
    }
}

/// Whether one schedule action operation may only target an event CLIPPING output.
///
/// The two data-source operations may: each is "a type of ScheduleActionOperation, and
/// the output must be an event clipping output"
/// (`smithy_query(query="shape",shape="UpdateClippingDataSourceConfiguration")` and
/// `…="RemoveClippingDataSourceConfiguration"`). Enabling or disabling an output is not
/// one of them: any output of a feed can be enabled or disabled.
pub fn targets_clipping_output_only(operation: &ScheduleActionOperation) -> bool {
    matches!(
        operation,
        ScheduleActionOperation::updateClippingDataSourceConfiguration(_)
            | ScheduleActionOperation::removeClippingDataSourceConfiguration(_)
    )
}

/// What this service answers a schedule action whose operation names an output the feed
/// does not hold: "The output must exist in the feed when you create the action"
/// (`UpdateOutputStatus$outputName`), which is the validation
/// `CreateScheduleAction` does ("Elemental Inference validates the targets of the action
/// when you create it"). Built from the name the request supplied, like every other
/// refusal of a reference that resolves to nothing.
pub fn schedule_action_output_not_found_message(output_name: &str) -> String {
    format!("Can't find output {output_name} in the feed. Use a different outputName and try again.")
}

/// What this service answers a data-source operation whose target is not an event
/// clipping output.
pub fn schedule_action_output_not_clipping_message(output_name: &str) -> String {
    format!(
        "Output {output_name} is not an event clipping output. Use the name of an event clipping \
         output and try again."
    )
}

/// What this service answers a `CreateScheduleAction` whose `actionName` the feed already
/// holds under a DIFFERENT action: the name "is unique within the feed"
/// (`ScheduleAction$actionName`), so a second, different action cannot take it.
pub fn schedule_action_name_in_use_message(action_name: &str) -> String {
    format!(
        "This feed already has a schedule action named {action_name}. Use a different actionName, \
         or delete that action, and try again."
    )
}

/// What this service answers once a feed holds the most schedule actions it may hold.
pub fn too_many_schedule_actions_message() -> String {
    format!(
        "This feed already holds the maximum of {MAX_SCHEDULE_ACTIONS_PER_FEED} schedule \
         actions. Delete a schedule action you no longer need and try again."
    )
}

/// What this service answers a delete of an action that has already run: "You can delete
/// only an action whose status is PENDING. Deleting an action doesn't reverse changes
/// that the action already made to the feed." (`DeleteScheduleAction`). The sentence
/// names the status the action is really in, which the store decided the refusal from.
pub fn schedule_action_not_pending_message(action_name: &str, status: &str) -> String {
    format!(
        "Schedule action {action_name} has the status {status}. You can delete only an action \
         whose status is PENDING."
    )
}

/// The failure a schedule action records when a target it named no longer resolves at the
/// scheduled time: "The action referred to something that didn't exist or wasn't valid at
/// the scheduled time" (`ScheduleActionError$errorCode`'s `INVALID_ACTION_TARGET`).
pub fn invalid_action_target_error(output_name: &str) -> ScheduleActionError {
    ScheduleActionError {
        error_code: "INVALID_ACTION_TARGET".to_string(),
        error_message: format!(
            "Can't find output {output_name} in the feed. The action referred to an output that \
             no longer exists."
        ),
    }
}

impl Feed {
    /// The schedule action of this feed named `action_name`, or `None` when it holds
    /// none.
    pub fn schedule_action_named(&self, action_name: &str) -> Option<&FeedScheduleAction> {
        self.schedule_actions
            .iter()
            .find(|action| action.action_name == action_name)
    }

    /// Apply every schedule action of this feed whose scheduled time has passed at `now`,
    /// and drop every settled action whose retention has run out.
    ///
    /// The one writer of a due action's outcome, called from the store's settle pass: the
    /// action's operations were owed at this instant whether or not anybody called, so a
    /// reporting operation materialising them changes nothing a later caller would not
    /// have seen anyway. An operation whose target no longer resolves fails the action
    /// ([`invalid_action_target_error`]) and the remaining operations of that action are
    /// not applied, because the action either "applied all the action's operations to the
    /// feed" or "couldn't apply the action" (`ScheduleAction$status`).
    ///
    /// Walks at most [`MAX_SCHEDULE_ACTIONS_PER_FEED`] actions, each with at most ten
    /// operations (the modelled bound on `ScheduleActionOperations`).
    pub fn settle_due_schedule_actions(&mut self, now: Instant) {
        if self.schedule_actions.is_empty() {
            return;
        }
        // The actions are taken OUT of the record for the pass, so applying an operation
        // can write the feed's own outputs while they are walked, and put back when the
        // pass is done — the feed holds exactly the actions it held before, minus the
        // ones whose retention ran out.
        let mut actions = std::mem::take(&mut self.schedule_actions);
        for action in &mut actions {
            if action.status != ScheduleActionLifecycle::Pending || now < action.due_at {
                continue;
            }
            let mut failure = None;
            for operation in &action.operations {
                if let Err(error) = apply_schedule_action_operation(&mut self.outputs, operation) {
                    failure = Some(error);
                    break;
                }
            }
            match failure {
                Some(error) => {
                    action.status = ScheduleActionLifecycle::Failed;
                    action.error = Some(error);
                }
                None => action.status = ScheduleActionLifecycle::Completed,
            }
            action.retire_at = Some(
                action
                    .due_at
                    .checked_add(SETTLED_ACTION_RETENTION)
                    .unwrap_or(action.due_at),
            );
        }
        actions.retain(|action| action.retire_at.is_none_or(|retire| now < retire));
        self.schedule_actions = actions;
    }

    /// The instant at which a schedule action scheduled for `scheduled_at_secs` (a UTC
    /// wall-clock instant, in epoch seconds) falls due on this store's clock.
    ///
    /// The caller named the time, so the span to wait is the one the request asked for:
    /// the difference between that instant and `now_secs`, the wall-clock reading taken
    /// at the same moment as `now`. A time already past is due at once, which is what
    /// makes an action scheduled for the past run at the next call rather than never, and
    /// a time beyond [`MAX_SCHEDULE_WAIT`] is clamped so no request can overflow the due
    /// instant.
    pub fn schedule_action_due_at(
        now: Instant,
        now_secs: i64,
        scheduled_at_secs: i64,
    ) -> Instant {
        let wait = scheduled_at_secs.saturating_sub(now_secs).max(0);
        let wait = Duration::from_secs(wait.unsigned_abs()).min(MAX_SCHEDULE_WAIT);
        now.checked_add(wait).unwrap_or(now)
    }
}

/// Apply ONE schedule action operation to `outputs`, the outputs of the feed the action
/// is on, or report the failure it earns.
///
/// The change is written into the feed's own outputs — the same records `UpdateFeed`
/// writes and `GetFeed` reports — so an action that has run is visible as the
/// configuration in force rather than as a note beside it. A free function rather than a
/// method, so the pass above can walk the actions it took off the record while this
/// writes the outputs still on it.
fn apply_schedule_action_operation(
    outputs: &mut [Output],
    operation: &ScheduleActionOperation,
) -> Result<(), ScheduleActionError> {
    let target = schedule_action_target_output(operation);
    let Some(output) = outputs.iter_mut().find(|output| output.name == target) else {
        return Err(invalid_action_target_error(target));
    };
    match operation {
        ScheduleActionOperation::updateOutputStatus(update) => {
            output.status = update.status.clone();
        }
        ScheduleActionOperation::updateClippingDataSourceConfiguration(update) => {
            let OutputConfig::Clipping(clipping) = &mut output.output_config else {
                return Err(invalid_action_target_error(target));
            };
            clipping.data_source_configuration = Some(update.data_source_configuration.clone());
        }
        ScheduleActionOperation::removeClippingDataSourceConfiguration(_) => {
            let OutputConfig::Clipping(clipping) = &mut output.output_config else {
                return Err(invalid_action_target_error(target));
            };
            clipping.data_source_configuration = None;
        }
    }
    Ok(())
}
