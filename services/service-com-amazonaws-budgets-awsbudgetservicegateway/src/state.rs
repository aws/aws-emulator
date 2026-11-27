// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-budgets-awsbudgetservicegateway service.
//!
//! The resource this service is built around is the **Budget**. AWS gives a budget
//! its identity from the account that owns it plus its name — `Budget$BudgetName`
//! is documented as "The name must be unique within an account" — so the store is
//! keyed by that pair. A budget OWNS the notifications, their subscribers and the
//! resource tags created with it ("After the end date, Amazon Web Services deletes
//! the budget and all the associated notifications and subscribers"), so they are
//! held inside the stored budget and go away with it. The budget ACTIONS created on
//! it are held the same way: an action carries the name of its budget
//! (`Action$BudgetName`) and every action operation names that budget beside the
//! `ActionId`, so an action cannot outlive the budget it acts on.
//!
//! That same sentence also bounds a budget's LIFE: a budget's `TimePeriod$End` is the
//! instant after which AWS holds it no longer, so the store is reached through
//! [`store_at`], which materialises every deletion the clock has already made due
//! ([`expire_due_budgets`]) before any operation sees it.
//!
//! The helpers below (ARN rendering, the opaque pagination token, the cross-member
//! rules `CreateBudget`/`UpdateBudget` document) are shared by several operation
//! handlers, so they live here rather than being copied into each `src/ops` file —
//! a new file under `src/ops/` would be picked up by `build.rs` as an operation.

use crate::ops::types::{
    Action, ActionHistory, ActionHistoryDetails, ActionThreshold, Budget,
    BudgetedAndActualAmounts, CalculatedSpend, CostTypes, Definition, HealthStatus, Notification,
    NotificationWithSubscribers, ResourceTag, Spend, Subscriber, TimePeriod,
};
use std::collections::HashMap;
use std::time::{Duration, SystemTime};

/// The alert a notification in alarm delivers to the subscribers held with it — the
/// only part of this service that calls another service, so it is kept apart from the
/// store itself.
pub mod alerts;

/// How this service words the refusals several operations share — one sentence per
/// CONDITION rather than per operation, so a client that recognises a condition by its
/// message recognises it whichever operation answered.
mod refusals;
pub use refusals::*;

/// The execution role a budget action is written with, resolved against the account's IAM
/// records — the other part of this service that calls another service, kept apart from
/// the store for the same reason.
pub mod execution_role;

/// The work an executed action's `Definition` describes, performed in the service that owns
/// the resources it names — the third part of this service that calls another service, kept
/// apart from the store for the same reason.
pub mod applied_definition;

/// The Region every in-process hop this service makes is addressed in.
///
/// Each service that holds what a budget action's members name is PARTITION-GLOBAL: IAM holds
/// one set of identities and policies per account, and AWS Organizations one organization,
/// whichever Region either is asked in. AWS Budgets is itself a global service whose requests
/// are signed in `us-east-1`, so that is where those hops are addressed — one constant rather
/// than anything read off a request. The one hop addressed elsewhere is the Systems Manager
/// one, whose Region the definition itself states (`SsmActionDefinition$Region`, "the Region
/// to run the SSM document").
pub const PARTITION_GLOBAL_REGION: &str = "us-east-1";

/// Per-service state for com-amazonaws-budgets-awsbudgetservicegateway. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBudgetsAwsbudgetservicegatewayState {
    /// Every budget this emulator holds, keyed by `(AccountId, BudgetName)` — the
    /// identity AWS gives a budget. The account half is always the one account this
    /// emulator answers for, since a request naming another is refused
    /// ([`foreign_account_refusal`]).
    pub budgets: HashMap<(String, String), StoredBudget>,
}

/// One persisted budget: the modelled shape as the caller last supplied it, with the
/// members the service owns stamped over it ([`stamp_service_owned`]), plus the
/// notifications and tags created alongside it.
///
/// No ARN is stored: nothing in this slice reports one. The operations that name a
/// budget by ARN (`TagResource`, `UntagResource` and `ListTagsForResource`) resolve it
/// the other way round — [`parse_resource_arn`] reads the account and
/// the budget name out of the ARN and looks the budget up under the key it already has,
/// so no second copy of a budget's identity is persisted.
#[derive(Debug, Clone)]
pub struct StoredBudget {
    /// The budget as `DescribeBudget` / `DescribeBudgets` read it back.
    pub budget: Budget,
    /// The notifications created with this budget, in the order they were supplied.
    pub notifications: Vec<StoredNotification>,
    /// The budget actions created on this budget, in the order they were created. An
    /// action "is a budget action resource" of the budget it names
    /// (`Action$BudgetName`), so it is held inside that budget and goes away with it.
    pub actions: Vec<StoredAction>,
    /// `ResourceTags` supplied at creation.
    pub tags: Vec<ResourceTag>,
}

/// A notification plus the subscribers attached to it. A notification cannot exist
/// without at least one subscriber (`Subscribers` is `@length(min: 1)`), so the two
/// are stored together.
///
/// The notification holds only what the CALLER owns: its `NotificationState` is dropped on
/// the write path ([`stamp_notification_owned`]) and computed on every read
/// ([`project_notification`]), the way a budget's `CalculatedSpend` is.
#[derive(Debug, Clone)]
pub struct StoredNotification {
    pub notification: Notification,
    pub subscribers: Vec<Subscriber>,
}

/// One persisted budget action: the modelled `Action` a read reports, the tags created
/// with it, and the settle the clock owes while the action is mid-flight.
///
/// The action's `Definition` is not merely recorded here: executing the action performs the
/// work that definition describes in the service that owns it and reverting it takes that
/// work back off ([`applied_definition`]), so the `Action$Status` state machine
/// `ExecuteBudgetAction` drives reports what that service did with it.
#[derive(Debug, Clone)]
pub struct StoredAction {
    /// The action as `DescribeBudgetAction` reads it back.
    pub action: Action,
    /// `ResourceTags` supplied at creation.
    pub tags: Vec<ResourceTag>,
    /// Every event recorded against this action, OLDEST FIRST — the order
    /// `DescribeBudgetActionHistories` reports and its pagination token indexes into.
    /// Append-only ([`record_action_event`]) and bounded by
    /// [`MAX_ACTION_HISTORY_RECORDS`].
    pub history: Vec<ActionHistory>,
    /// The settle owed while `action.status` is one of the `*_IN_PROGRESS` statuses,
    /// or `None` for an action whose status is settled.
    pub pending: Option<PendingSettle>,
    /// Whether the work the running execution started is still outstanding — set by the call
    /// that starts work in the service that owns the definition ([`applied_definition`]) and
    /// cleared by that same call once that service has answered.
    ///
    /// A settle is not due while it is set, however long the clock says: the status an
    /// execution settles to is the one its WORK leaves behind, so an action whose work has
    /// not answered yet is still executing and keeps its `*_IN_PROGRESS` status — which is
    /// also what holds a second execution off it ([`action_is_in_progress`]).
    pub work_outstanding: bool,
}

/// The status an in-flight action moves to, and the instant the clock owes that move.
#[derive(Debug, Clone)]
pub struct PendingSettle {
    /// When the move falls due — set by the call that started the work, never slept on.
    pub settles_at: SystemTime,
    /// The `ActionStatus` the action carries once the move is made.
    pub settles_to: String,
}

impl StoredAction {
    /// A newly created action: the status this service's reading of its trigger threshold
    /// gives it ([`created_action_status`]) is already stamped on `action`, and it carries
    /// nothing owed and an empty history — the call that creates it records its own
    /// `CREATE_ACTION` event.
    pub fn new(action: Action, tags: Vec<ResourceTag>) -> Self {
        Self {
            action,
            tags,
            history: Vec::new(),
            pending: None,
            work_outstanding: false,
        }
    }

    /// Materialise the settle the clock has already made due.
    ///
    /// A reporting operation may call this: the move was owed at
    /// [`PendingSettle::settles_at`] whether or not anybody called, so making it here
    /// reports the status the action already had rather than changing it.
    ///
    /// The move is a status change this SERVICE makes on its own, so — like every other
    /// status change of the action — it is recorded in the action's history, with
    /// [`EVENT_TYPE_SYSTEM`] as the event type that "distinguishes between whether the
    /// events are triggered by the user or are generated by the system"
    /// (`ActionHistory$EventType`). The record is timestamped at the instant the move
    /// fell due, not at the instant someone noticed, and carries the settled status —
    /// so the newest history record always agrees with what a read of the action itself
    /// reports.
    ///
    /// Nothing is due while the execution's own work is still outstanding
    /// ([`Self::work_outstanding`]): the status a settle makes is the one that work leaves
    /// behind, so settling before it answers would report an outcome nothing had reached.
    /// That hold is itself bounded ([`WORK_ANSWER_BACKSTOP`]) — work that never answers at
    /// all, because the request performing it went away, did not complete either, so the
    /// settle is made at the failure status rather than leaving the action executing for
    /// ever.
    pub fn settle_due(&mut self, now: SystemTime) {
        let Some(pending) = self.pending.clone() else {
            return;
        };
        if self.work_outstanding {
            // The backstop instant, built with `checked_add` for the reason
            // [`Self::start`] gives: an unrepresentable one leaves the hold at the settle's
            // own instant, which is the nearest this clock can name.
            let backstop = pending
                .settles_at
                .checked_add(WORK_ANSWER_BACKSTOP)
                .unwrap_or(pending.settles_at);
            if now < backstop {
                return;
            }
            self.work_answered(true);
        }
        let Some(pending) = self.pending.clone() else {
            return;
        };
        if now < pending.settles_at {
            return;
        }
        self.action.status = pending.settles_to.clone();
        self.pending = None;
        record_action_event(self, EVENT_TYPE_SYSTEM, pending.settles_at);
    }

    /// The work this action's execution started has ANSWERED, `refused` when the service
    /// that owns it answered that it did not do the work ([`applied_definition`]).
    ///
    /// Work that was refused leaves the definition out of place and the execution
    /// incomplete, so what the clock owes becomes the failure counterpart of the status the
    /// execution was started with ([`failed_settle`]). Either way the settle this execution
    /// owes is no longer held back ([`Self::work_outstanding`]), so it falls due at the
    /// instant the execution set — or at once, where that instant has already passed while
    /// the work ran.
    ///
    /// The action keeps the `*_IN_PROGRESS` status its execution gave it: only the status the
    /// clock owes is decided here, so a caller reading while the work runs still sees that
    /// status and the refusals it carries. An action whose settle is no longer owed — one
    /// nothing started — has nothing to answer for.
    pub fn work_answered(&mut self, refused: bool) {
        self.work_outstanding = false;
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        if refused {
            if let Some(failed) = failed_settle(&pending.settles_to) {
                pending.settles_to = failed.to_string();
            }
        }
    }

    /// Start `execution`'s work: the action takes its `*_IN_PROGRESS` status now, and
    /// the clock owes the settled status [`ACTION_SETTLES_AFTER`] later.
    ///
    /// `performs_work` says whether this execution has work for the service that owns the
    /// action's definition ([`applied_definition::work_of`]). While that work is outstanding
    /// no settle is due, and the call that started it answers for it
    /// ([`Self::work_answered`]) on every path.
    pub fn start(&mut self, execution: ActionExecution, now: SystemTime, performs_work: bool) {
        self.action.status = execution.in_progress.to_string();
        self.work_outstanding = performs_work;
        self.pending = Some(PendingSettle {
            // `checked_add` rather than `SystemTime`'s own `Add`, which is an expect on
            // overflow — the same reason [`instant_at`] uses it. An unrepresentable
            // instant leaves the settle owed from `now`, which is the nearest instant
            // this clock can name.
            settles_at: now.checked_add(ACTION_SETTLES_AFTER).unwrap_or(now),
            settles_to: execution.settles_to.to_string(),
        });
    }
}

/// `ActionStatus$STANDBY` — the status a created action carries and the one a reset action
/// returns to: the action is armed and waiting for the threshold that triggers it, the
/// definition it applies not being in place.
pub const ACTION_STATUS_STANDBY: &str = "STANDBY";

/// `ActionStatus$PENDING` — the status of an action whose trigger threshold the spend it
/// watches has reached while the manual approval its `ApprovalModel` requires has not been
/// given: the action is no longer merely waiting, and nothing has applied its definition
/// either.
pub const ACTION_STATUS_PENDING: &str = "PENDING";

/// `ApprovalModel$MANUAL` — the action whose execution has to be approved before it runs,
/// which is what `ExecuteBudgetAction`'s `APPROVE_BUDGET_ACTION` does.
pub const APPROVAL_MODEL_MANUAL: &str = "MANUAL";

/// The `ActionStatus` a CREATED action carries: the service's own reading of the action's
/// trigger threshold against the spend the budget tracks ([`action_threshold_passed`]),
/// under the approval its `ApprovalModel` asks for ("this specifies if the action needs
/// manual or automatic approval").
///
/// A created action is ARMED and waiting for its threshold, which is
/// [`ACTION_STATUS_STANDBY`] — there is nothing to do for it until the spend the budget
/// tracks reaches the threshold that triggers it. Where that spend has ALREADY reached the
/// threshold and the action's execution needs manual approval, that approval is what the
/// action is now waiting for, which is [`ACTION_STATUS_PENDING`]: the approval is given by
/// an `ExecuteBudgetAction` and by nothing else, so the action stands there until one
/// arrives.
///
/// An action that is approved automatically is left ARMED even where its trigger is
/// already met: AWS would run such an action, and nothing in this emulator TRIGGERS an
/// action's execution on its own — an execution is performed by an `ExecuteBudgetAction`
/// call and by nothing else ([`applied_definition`]) — so reporting an execution status for
/// it would be a status for work nothing started. That is a known divergence of this
/// emulator rather than a reading of the model.
///
/// Where the budget's figures state no comparison at all — a `FORECASTED` action of a
/// budget no forecast is reported for, a percentage of a budget that reports no limit —
/// nothing has reached the threshold, so the action is armed: the same reading
/// [`notification_state`] takes of a threshold it cannot compare.
///
/// The ONE place the created status is decided, so `CreateBudgetAction` and every read of
/// a fresh action agree on it.
pub fn created_action_status(
    budget: &Budget,
    notification_type: &str,
    action_threshold: &ActionThreshold,
    approval_model: &str,
) -> &'static str {
    let triggered =
        action_threshold_passed(budget, notification_type, action_threshold).unwrap_or(false);
    if triggered && approval_model == APPROVAL_MODEL_MANUAL {
        ACTION_STATUS_PENDING
    } else {
        ACTION_STATUS_STANDBY
    }
}

/// Whether the spend an action watches has already reached the threshold that triggers it,
/// or `None` when this budget's figures do not state the comparison at all.
///
/// Which spend the action watches is its `NotificationType` ("the type of a notification.
/// It must be `ACTUAL` or `FORECASTED`"), read off the same `CalculatedSpend` a read of the
/// budget reports ([`spend_watched_by`]), and the amount it is measured against is
/// `ActionThreshold$ActionThresholdValue` read through
/// `ActionThreshold$ActionThresholdType` ([`threshold_in_budget_unit`]) — the same two
/// readings a notification's threshold is made of. An `ActionThreshold` carries no
/// comparison operator of its own, so the one comparison it states is the trigger: the
/// action triggers once the watched spend REACHES the threshold.
///
/// `None` wherever there is nothing to compare, the same cases [`notification_state`]
/// answers `OK` for: a figure the budget reports none of, and a threshold type or
/// notification type this service does not know.
pub fn action_threshold_passed(
    budget: &Budget,
    notification_type: &str,
    action_threshold: &ActionThreshold,
) -> Option<bool> {
    let spent = spend_watched_by(budget, notification_type)?;
    let threshold = threshold_in_budget_unit(
        budget,
        action_threshold.action_threshold_value,
        &action_threshold.action_threshold_type,
    )?;
    Some(spent >= threshold)
}

/// `ActionStatus$EXECUTION_IN_PROGRESS` — the action's execution is under way.
pub const ACTION_STATUS_EXECUTION_IN_PROGRESS: &str = "EXECUTION_IN_PROGRESS";

/// `ActionStatus$EXECUTION_SUCCESS` — the action's definition is in place.
pub const ACTION_STATUS_EXECUTION_SUCCESS: &str = "EXECUTION_SUCCESS";

/// `ActionStatus$EXECUTION_FAILURE` — an execution that did not complete.
pub const ACTION_STATUS_EXECUTION_FAILURE: &str = "EXECUTION_FAILURE";

/// `ActionStatus$REVERSE_IN_PROGRESS` — the reversal of an executed action is under way.
pub const ACTION_STATUS_REVERSE_IN_PROGRESS: &str = "REVERSE_IN_PROGRESS";

/// `ActionStatus$REVERSE_SUCCESS` — the action's definition has been taken back off.
pub const ACTION_STATUS_REVERSE_SUCCESS: &str = "REVERSE_SUCCESS";

/// `ActionStatus$REVERSE_FAILURE` — a reversal that did not complete.
pub const ACTION_STATUS_REVERSE_FAILURE: &str = "REVERSE_FAILURE";

/// `ActionStatus$RESET_IN_PROGRESS` — the re-arming of a settled action is under way.
pub const ACTION_STATUS_RESET_IN_PROGRESS: &str = "RESET_IN_PROGRESS";

/// `ActionStatus$RESET_FAILURE` — a reset that did not complete.
pub const ACTION_STATUS_RESET_FAILURE: &str = "RESET_FAILURE";

/// `ExecutionType$APPROVE_BUDGET_ACTION` — execute the action.
pub const EXECUTION_TYPE_APPROVE: &str = "APPROVE_BUDGET_ACTION";

/// `ExecutionType$RETRY_BUDGET_ACTION` — run the work that failed again.
pub const EXECUTION_TYPE_RETRY: &str = "RETRY_BUDGET_ACTION";

/// `ExecutionType$REVERSE_BUDGET_ACTION` — take an executed action back off.
pub const EXECUTION_TYPE_REVERSE: &str = "REVERSE_BUDGET_ACTION";

/// `ExecutionType$RESET_BUDGET_ACTION` — re-arm a settled action.
pub const EXECUTION_TYPE_RESET: &str = "RESET_BUDGET_ACTION";

/// How long the settle an execution owes is held back for work that has not answered yet
/// ([`StoredAction::work_outstanding`]).
///
/// Emulator backstop, NOT an AWS figure: the call that starts the work answers for it on
/// every path it returns on, so this is reached only where that request went away without
/// answering — and without it such an action would be left executing, and locked against
/// every later execution, for good. Generous rather than tight, because the work is as slow
/// as what the owning service does for it: a definition applied across a hundred identities
/// on a loaded host is still work in progress, and only a request that is gone fails to
/// answer inside this.
pub const WORK_ANSWER_BACKSTOP: Duration = Duration::from_secs(60);

/// How long an action's work stays in flight before the clock settles it.
///
/// `ExecuteBudgetAction` answers while the work is still running — that is what the
/// `*_IN_PROGRESS` statuses are for — so the settled status is owed a short span later.
/// AWS documents no duration for it, so this is the emulator's own: long enough that a
/// caller reading straight after the call still sees `*_IN_PROGRESS` and the refusals
/// that status carries, short enough not to make a test wait.
pub const ACTION_SETTLES_AFTER: Duration = Duration::from_millis(500);

/// The status move one `ExecuteBudgetAction` call makes: the `*_IN_PROGRESS` status the
/// action takes immediately, and the status the clock settles it to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionExecution {
    /// The `ActionStatus` the action carries while the work runs.
    pub in_progress: &'static str,
    /// The `ActionStatus` it carries once [`ACTION_SETTLES_AFTER`] has passed.
    pub settles_to: &'static str,
}

/// Whether `status` is one of the `*_IN_PROGRESS` statuses — work this service has
/// already started and has not settled yet.
///
/// An action in one of them is the locked resource `ResourceLockedException` ("the
/// server rejected that particular method for the requested resource") names: while the
/// work runs, none of the methods that mutate the action — a second execution, an update
/// of its members, its deletion — is accepted.
pub fn action_is_in_progress(status: &str) -> bool {
    matches!(
        status,
        ACTION_STATUS_EXECUTION_IN_PROGRESS
            | ACTION_STATUS_REVERSE_IN_PROGRESS
            | ACTION_STATUS_RESET_IN_PROGRESS
    )
}

/// The move `execution_type` makes on an action whose settled status is `status`, or
/// `None` when that execution type has nothing to do from that status.
///
/// Each execution type names the work it performs, and the statuses it is accepted from
/// are the ones in which that work is left to do:
///
/// * `APPROVE_BUDGET_ACTION` gives the approval the action's triggered execution is
///   waiting for, so it is accepted only from the status in which that approval is what
///   is left to do ([`ACTION_STATUS_PENDING`]). An ARMED action
///   ([`ACTION_STATUS_STANDBY`]) has nothing to approve — the threshold that triggers it
///   has not been reached — so this execution type has no work there either.
/// * `REVERSE_BUDGET_ACTION` takes an applied action back off, so it is accepted once
///   the definition is in place ([`ACTION_STATUS_EXECUTION_SUCCESS`]) or a previous
///   reversal left it there ([`ACTION_STATUS_REVERSE_FAILURE`]).
/// * `RESET_BUDGET_ACTION` re-arms the action, so it is accepted from every settled
///   status in which it has already run — and not from `STANDBY`, which is already
///   armed, or `PENDING`, which has not run. `ActionStatus` has no `RESET_SUCCESS`,
///   and `STANDBY` is the status an armed action carries, so that is what a completed
///   reset leaves behind.
/// * `RETRY_BUDGET_ACTION` runs the work that failed again, so it is accepted from each
///   `*_FAILURE` status and repeats the operation that produced it.
///
/// Nothing in this emulator ever fails an action's work BY ITSELF: a `*_FAILURE` status is
/// minted only where the service that owns the definition's work refused it
/// ([`applied_definition`]), which is what [`failed_settle`] names, so those rows are the
/// rule stated over the whole `ActionStatus` enum rather than a status this service invents.
pub fn action_execution(execution_type: &str, status: &str) -> Option<ActionExecution> {
    let execute = ActionExecution {
        in_progress: ACTION_STATUS_EXECUTION_IN_PROGRESS,
        settles_to: ACTION_STATUS_EXECUTION_SUCCESS,
    };
    let reverse = ActionExecution {
        in_progress: ACTION_STATUS_REVERSE_IN_PROGRESS,
        settles_to: ACTION_STATUS_REVERSE_SUCCESS,
    };
    let reset = ActionExecution {
        in_progress: ACTION_STATUS_RESET_IN_PROGRESS,
        settles_to: ACTION_STATUS_STANDBY,
    };
    match (execution_type, status) {
        (EXECUTION_TYPE_APPROVE, ACTION_STATUS_PENDING) => Some(execute),
        (
            EXECUTION_TYPE_REVERSE,
            ACTION_STATUS_EXECUTION_SUCCESS | ACTION_STATUS_REVERSE_FAILURE,
        ) => Some(reverse),
        (
            EXECUTION_TYPE_RESET,
            ACTION_STATUS_EXECUTION_SUCCESS
            | ACTION_STATUS_EXECUTION_FAILURE
            | ACTION_STATUS_REVERSE_SUCCESS
            | ACTION_STATUS_REVERSE_FAILURE
            | ACTION_STATUS_RESET_FAILURE,
        ) => Some(reset),
        (EXECUTION_TYPE_RETRY, ACTION_STATUS_EXECUTION_FAILURE) => Some(execute),
        (EXECUTION_TYPE_RETRY, ACTION_STATUS_REVERSE_FAILURE) => Some(reverse),
        (EXECUTION_TYPE_RETRY, ACTION_STATUS_RESET_FAILURE) => Some(reset),
        _ => None,
    }
}

/// The `ActionStatus` an execution that was aimed at `settles_to` settles to instead when
/// the work it performs did not complete, or `None` when no failure status corresponds to
/// it.
///
/// Each settled status that reports completed work has one `*_FAILURE` counterpart in the
/// modelled `ActionStatus` catalog, naming the operation that did not complete: an execution
/// that was to leave the definition in place, a reversal that was to take it off, and the
/// re-arming a reset performs — whose completed status is [`ACTION_STATUS_STANDBY`], the
/// status an armed action carries, since `ActionStatus` has no `RESET_SUCCESS`.
pub fn failed_settle(settles_to: &str) -> Option<&'static str> {
    match settles_to {
        ACTION_STATUS_EXECUTION_SUCCESS => Some(ACTION_STATUS_EXECUTION_FAILURE),
        ACTION_STATUS_REVERSE_SUCCESS => Some(ACTION_STATUS_REVERSE_FAILURE),
        ACTION_STATUS_STANDBY => Some(ACTION_STATUS_RESET_FAILURE),
        _ => None,
    }
}

/// The action of `stored` that `action_id` names, with any settle the clock has already
/// made due materialised ([`StoredAction::settle_due`]), or `None` when the budget
/// holds no such action — which the caller answers with `NotFoundException`.
pub fn find_action_mut<'a>(
    stored: &'a mut StoredBudget,
    action_id: &str,
    now: SystemTime,
) -> Option<&'a mut StoredAction> {
    let found = find_action_unsettled_mut(stored, action_id)?;
    found.settle_due(now);
    Some(found)
}

/// The action of `stored` that `action_id` names, with the settle the clock owes it left
/// OWED, or `None` when the budget holds no such action.
///
/// For the one caller that has to re-aim that owed settle rather than report it
/// ([`StoredAction::work_answered`]): a settle already due is still the one this execution
/// owes, so materialising it first would record a status the work never reached, while
/// leaving it owed lets the next observation report the status the work actually left —
/// timestamped at the instant it fell due, as every settle is. Every OTHER reader goes
/// through [`find_action_mut`], which materialises it.
pub fn find_action_unsettled_mut<'a>(
    stored: &'a mut StoredBudget,
    action_id: &str,
) -> Option<&'a mut StoredAction> {
    stored
        .actions
        .iter_mut()
        .find(|held| held.action.action_id == action_id)
}

/// Every action `stored` holds, as a read reports it at `now`, in the order they were
/// created — the order a paginated read's token indexes into, so it does not depend on
/// anything a later call can reshuffle.
///
/// Each action's owed settle is materialised first ([`StoredAction::settle_due`]), for
/// the same reason [`find_action_mut`] does it: the move fell due whether or not anybody
/// called, so a list reports the status the action already carried rather than a stale
/// one — and agrees with what `DescribeBudgetAction` reports for the same action.
pub fn actions_for_budget(stored: &mut StoredBudget, now: SystemTime) -> Vec<Action> {
    stored
        .actions
        .iter_mut()
        .map(|held| {
            held.settle_due(now);
            held.action.clone()
        })
        .collect()
}

/// The number of actions one budget will hold.
///
/// Not an AWS quota reproduced here: it is this emulator's own ceiling, so a caller
/// cannot grow the process without limit by creating actions on one budget (the number
/// of BUDGETS is already bounded by [`MAX_BUDGETS_PER_ACCOUNT`] /
/// [`MAX_BUDGETS_TOTAL`]). `CreateBudgetAction` answers the modelled
/// `ServiceQuotaExceededException` ("You've reached a Service Quota limit on this
/// resource") once a budget holds this many.
pub const MAX_ACTIONS_PER_BUDGET: usize = 10;

/// The `Definition` member an `ActionType` is written with: "The type of action. This
/// defines the type of tasks that can be carried out by this action. This field also
/// determines the format for definition" (`Action$ActionType`), over a `Definition`
/// whose three members — `IamActionDefinition`, `ScpActionDefinition`,
/// `SsmActionDefinition` — are each optional in the model, so no constraint trait can
/// tie them to the action type.
///
/// `None` for an `ActionType` this emulator does not know, which the modelled
/// `ActionType` enum already refuses at the boundary.
fn definition_member_for(action_type: &str) -> Option<&'static str> {
    match action_type {
        ACTION_TYPE_APPLY_IAM_POLICY => Some("IamActionDefinition"),
        ACTION_TYPE_APPLY_SCP_POLICY => Some("ScpActionDefinition"),
        ACTION_TYPE_RUN_SSM_DOCUMENTS => Some("SsmActionDefinition"),
        _ => None,
    }
}

/// `ActionType$APPLY_IAM_POLICY` — the action whose definition is a managed policy and the
/// IAM identities it goes on, so the work it performs belongs to IAM.
pub const ACTION_TYPE_APPLY_IAM_POLICY: &str = "APPLY_IAM_POLICY";

/// `ActionType$APPLY_SCP_POLICY` — the action whose definition is a service control policy
/// and the targets it goes on, so the work it performs belongs to AWS Organizations.
pub const ACTION_TYPE_APPLY_SCP_POLICY: &str = "APPLY_SCP_POLICY";

/// `ActionType$RUN_SSM_DOCUMENTS` — the action whose definition is a document to run and the
/// instances to run it against, so the work it performs belongs to Systems Manager.
pub const ACTION_TYPE_RUN_SSM_DOCUMENTS: &str = "RUN_SSM_DOCUMENTS";

/// The cross-member rule `Action$ActionType` documents, checked over the `Definition`
/// supplied with an action: the action type determines which member of `Definition`
/// describes the work, so exactly that one must be present and the other two must not.
///
/// `Err` carries the sentence that goes in `InvalidParameterException$Message` ("An
/// error on the client occurred. Typically, the cause is an invalid input value.").
pub fn validate_action_definition(action_type: &str, definition: &Definition) -> Result<(), String> {
    let Some(wanted) = definition_member_for(action_type) else {
        return Ok(());
    };
    let supplied: Vec<&'static str> = [
        ("IamActionDefinition", definition.iam_action_definition.is_some()),
        ("ScpActionDefinition", definition.scp_action_definition.is_some()),
        ("SsmActionDefinition", definition.ssm_action_definition.is_some()),
    ]
    .into_iter()
    .filter(|(_, present)| *present)
    .map(|(member, _)| member)
    .collect();
    if supplied == [wanted] {
        return Ok(());
    }
    Err(format!(
        "Unable to create/update budget action: an {action_type} action is defined by \
         Definition.{wanted} alone."
    ))
}

/// The account field of an ARN — the fifth of the six colon-separated fields an ARN is
/// written in (`arn:<partition>:<service>:<region>:<account>:<resource>`), answered only
/// when it is the twelve-digit id an AWS account is named by (`AccountId`: len 12..12,
/// `^\d{12}$`).
///
/// The ARN itself is read with the one shared parse ([`::service_common::arn::parse`])
/// rather than split again here. `None` for anything else, which a member carrying a
/// modelled ARN `@pattern` whose account field is `\d{12}` cannot be once request
/// validation has run.
fn arn_account(arn: &str) -> Option<&str> {
    let account = ::service_common::arn::parse(arn)?.account;
    (account.len() == ACCOUNT_ID_LEN && account.chars().all(|c| c.is_ascii_digit()))
        .then_some(account)
}

/// The width of an account id: "it's a 12-digit number" (`AccountId`, `@length(12, 12)`
/// and `^\d{12}$`).
const ACCOUNT_ID_LEN: usize = 12;

/// The cross-member rule `Action$ExecutionRoleArn` documents, checked over the role an
/// action is written with: "the role passed for action execution and reversion. Roles and
/// actions must be in the same account" — so the account field of the role's ARN has to
/// be the account the action lives in (`AccountId`). The modelled `@pattern` fixes the
/// ARN's SHAPE only (`arn:aws…:iam::\d{12}:role/…`); no constraint trait can tie those
/// twelve digits to another member of the request, so this is the handler's to enforce on
/// every path that writes the member.
///
/// Fails CLOSED: a string whose account field is not an account id at all names no
/// account to be in the same one as, so it is refused here too rather than passed
/// through. Such a value cannot reach a handler through the modelled member (request
/// validation refuses it first), so the refusal is what this rule means when the helper
/// is reached by any other caller.
///
/// `Err` carries the sentence that goes in `InvalidParameterException$Message` ("An error
/// on the client occurred. Typically, the cause is an invalid input value."), built from
/// the request's own two values.
pub fn validate_execution_role_account(
    account_id: &str,
    execution_role_arn: &str,
) -> Result<(), String> {
    let Some(role_account) = arn_account(execution_role_arn) else {
        return Err(format!(
            "Unable to create/update budget action: ExecutionRoleArn must be the ARN of an IAM \
             role in account {account_id}, and {execution_role_arn} names no account."
        ));
    };
    if role_account == account_id {
        return Ok(());
    }
    Err(format!(
        "Unable to create/update budget action: ExecutionRoleArn names a role in account \
         {role_account}, and the role of an action in account {account_id} must be in that \
         same account."
    ))
}

/// `HealthStatusValue$HEALTHY` — the only state a budget in this emulator can be in:
/// every budget here derives from a billing view this emulator always serves, so
/// nothing can make it `UNHEALTHY` (which is why no `StatusReason` is reported either:
/// `HealthStatus$StatusReason` is "the reason for the current status", and a healthy
/// resource has none).
pub const HEALTH_STATUS_HEALTHY: &str = "HEALTHY";

/// `NotificationState$OK` — a notification that has not passed its threshold.
/// "Specifies whether this notification is in alarm. If a budget notification is in the
/// `ALARM` state, you passed the set threshold for the budget."
pub const NOTIFICATION_STATE_OK: &str = "OK";

/// `NotificationState$ALARM` — a notification whose threshold has been passed, which is
/// the state that owes its subscribers an alert ([`alerts`]).
pub const NOTIFICATION_STATE_ALARM: &str = "ALARM";

/// Seconds since the Unix epoch at `at`, clamped at the epoch for an earlier instant.
fn unix_secs(at: SystemTime) -> i64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// The wire form a budget timestamp is rendered in and read back from, so the calendar
/// is the one the runtime's own `DateTime` implements rather than arithmetic written
/// here: `YYYY-MM-DDThh:mm:ssZ`.
const CIVIL_FORMAT: ::aws_smithy_types::date_time::Format =
    ::aws_smithy_types::date_time::Format::DateTime;

/// The instant `civil` names, which is `YYYY-MM-DDT00:00:00Z` for every caller here.
///
/// `None` for a string that is not a date-time, which none of the callers below can
/// produce (each builds it from a rendered date).
fn midnight_at(civil: &str) -> Option<::aws_smithy_types::DateTime> {
    ::aws_smithy_types::DateTime::from_str(civil, CIVIL_FORMAT).ok()
}

/// The year, month and day of `at` in UTC, read off the runtime's own rendering of it
/// (`YYYY-MM-DD...`), so no date arithmetic is written here.
fn civil_date(at: SystemTime) -> Option<(i64, u32, u32)> {
    let rendered = ::aws_smithy_types::DateTime::from_secs(unix_secs(at))
        .fmt(CIVIL_FORMAT)
        .ok()?;
    let date = rendered.split('T').next()?;
    let mut parts = date.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    Some((year, month, day))
}

/// The end date AWS gives a budget whose caller supplied none: "If you didn't specify
/// an end date, Amazon Web Services set your end date to `06/15/87 00:00 UTC`"
/// (`TimePeriod$End`). Written as the date it is rather than as an epoch literal.
pub fn default_time_period_end() -> ::aws_smithy_types::DateTime {
    midnight_at("2087-06-15T00:00:00Z").expect("06/15/87 00:00 UTC is a date-time")
}

/// The start date AWS gives a budget whose caller supplied none: "Amazon Web Services
/// defaults to the start of your chosen time period (DAILY, MONTHLY, QUARTERLY,
/// ANNUALLY, or CUSTOM) ... if you created your budget on January 24, 2018, chose
/// `DAILY`, and didn't set a start date, Amazon Web Services set your start date to
/// `01/24/18 00:00 UTC`. If you chose `MONTHLY`, Amazon Web Services set your start date
/// to `01/01/18 00:00 UTC`" (`TimePeriod$Start`) — the midnight-UTC boundary of the
/// period `time_unit` names that contains `at`.
///
/// `CUSTOM` names no period length, so it takes the same boundary as `DAILY`: the day
/// the budget was written. A `TimeUnit` this emulator does not know yet takes that
/// boundary too, which is the narrowest period the enum has.
///
/// The boundary is chosen on the rendered date and turned back into an instant by the
/// runtime's own date-time parser, so this holds no calendar arithmetic of its own.
pub fn default_time_period_start(time_unit: &str, at: SystemTime) -> ::aws_smithy_types::DateTime {
    let Some((year, month, day)) = civil_date(at) else {
        // Unreachable for any instant the runtime can render; the instant itself is
        // still a sound answer for "the period containing it".
        return ::aws_smithy_types::DateTime::from_secs(unix_secs(at));
    };
    let (month, day) = match time_unit {
        "MONTHLY" => (month, 1),
        // Calendar quarters: the first month of the quarter `month` falls in.
        "QUARTERLY" => (((month.saturating_sub(1)) / 3) * 3 + 1, 1),
        "ANNUALLY" => (1, 1),
        _ => (month, day),
    };
    midnight_at(&format!("{year:04}-{month:02}-{day:02}T00:00:00Z"))
        .unwrap_or_else(|| ::aws_smithy_types::DateTime::from_secs(unix_secs(at)))
}

/// The per-member defaults `CostTypes` documents: "The default value is `true`" on every
/// `Include*` member, and `false` on `UseBlended` and `UseAmortized`. A member the caller
/// set keeps its value.
fn apply_cost_type_defaults(cost_types: &mut CostTypes) {
    for include in [
        &mut cost_types.include_tax,
        &mut cost_types.include_subscription,
        &mut cost_types.include_refund,
        &mut cost_types.include_credit,
        &mut cost_types.include_upfront,
        &mut cost_types.include_recurring,
        &mut cost_types.include_other_subscription,
        &mut cost_types.include_support,
        &mut cost_types.include_discount,
    ] {
        if include.is_none() {
            *include = Some(true);
        }
    }
    for rate in [&mut cost_types.use_blended, &mut cost_types.use_amortized] {
        if rate.is_none() {
            *rate = Some(false);
        }
    }
}

/// A collection member of a `Budget` that the caller may send EMPTY: a map or a list
/// whose entries are the selection it makes.
///
/// `Budget$Metrics` is `@length(min: 0, max: 1)`, and no constraint bounds
/// `PlannedBudgetLimits` or `CostFilters` below zero entries either, so "present" and
/// "selects something" are different questions for these members — see [`selection`].
trait Collected {
    /// Whether this collection carries any entry at all.
    fn carries_entry(&self) -> bool;
}

impl<K, V> Collected for HashMap<K, V> {
    fn carries_entry(&self) -> bool {
        !self.is_empty()
    }
}

impl<T> Collected for Vec<T> {
    fn carries_entry(&self) -> bool {
        !self.is_empty()
    }
}

/// The entries `member` SELECTS, or `None` when it selects nothing.
///
/// Every rule in this file that asks "did the caller supply this limit, this filter or
/// this metric?" asks it through here: a collection member sent empty names no limit, no
/// filter and no metric, so it is the same answer as a member that was not sent at all.
/// One home for that question, so the rules that turn on it (the limit a budget must
/// carry, the two mutually exclusive filter syntaxes, the budget types and granularities
/// a planned map is available for, and which budgets hold cost types) cannot come to
/// disagree about one request.
fn selection<C: Collected>(member: &Option<C>) -> Option<&C> {
    member
        .as_ref()
        .filter(|collection| collection.carries_entry())
}

/// Whether `member` selects anything — [`selection`] as a predicate.
fn selects<C: Collected>(member: &Option<C>) -> bool {
    selection(member).is_some()
}

/// Whether this budget HAS cost types at all.
///
/// Two documented rules decide it: "`USAGE`, `RI_UTILIZATION`, `RI_COVERAGE`,
/// `SAVINGS_PLANS_UTILIZATION`, and `SAVINGS_PLANS_COVERAGE` budgets don't have
/// `CostTypes`" (`CostTypes`), so only a `COST` budget does; and "only one set of filter
/// and metric selections can be present in the syntax at one time. Either
/// `FilterExpression` and `Metrics` or `CostFilters` and `CostTypes`, not both"
/// (`CreateBudget`), so a budget written in the `FilterExpression`/`Metrics` syntax has
/// none either — defaulting them there would store exactly the combination that rule
/// forbids (and which [`validate_budget`] refuses from a caller).
///
/// A budget is written in that other syntax only when it SELECTS something in it: an
/// empty `Metrics` list chooses no metric ([`selection`]), so it leaves the budget in the
/// `CostFilters`/`CostTypes` syntax, which a `COST` budget has cost types in.
fn cost_types_apply(budget: &Budget) -> bool {
    budget.budget_type == COST_BUDGET_TYPE
        && budget.filter_expression.is_none()
        && !selects(&budget.metrics)
}

/// Store the cost-types selection the budget HAS, and nothing else.
///
/// `CostTypes` is a member of the budget only while [`cost_types_apply`] holds: it is
/// "the types of costs that are included in this `COST` budget", and a budget of any
/// other type "do[es] not have `CostTypes`" (`Budget$CostTypes`). So the write path
/// settles the member against the budget it is writing rather than only defaulting it:
///
/// * a budget that has cost types reads back with every member of them, each the value
///   the caller sent or the documented per-member default
///   ([`apply_cost_type_defaults`]), so one is stored even for a request that carried
///   none;
/// * a budget that has none reads back with NO `CostTypes` at all, whatever the request
///   carried for it — a selection the budget cannot hold is dropped rather than
///   persisted and echoed by every later read, the same treatment a member the service
///   itself owns gets ([`drop_service_derived`]).
fn settle_cost_types(budget: &mut Budget) {
    if !cost_types_apply(budget) {
        budget.cost_types = None;
        return;
    }
    apply_cost_type_defaults(budget.cost_types.get_or_insert_with(CostTypes::default));
}

/// The `BudgetType` that has cost types.
const COST_BUDGET_TYPE: &str = "COST";

/// The `BudgetType` members whose budget is required to carry a limit, and the only ones
/// a `PlannedBudgetLimits` map can be written for: "`BudgetLimit` is required for cost or
/// usage budgets, but optional for RI or Savings Plans utilization or coverage budgets"
/// (`Budget$BudgetLimit`) and "`PlannedBudgetLimits` is available for cost or usage
/// budget" (`Budget$PlannedBudgetLimits`).
const LIMIT_REQUIRED_BUDGET_TYPES: [&str; 2] = [COST_BUDGET_TYPE, "USAGE"];

/// The `BudgetType` members whose budget tracks a utilization or coverage PERCENTAGE,
/// and whose `BudgetLimit` therefore has exactly one documented value: "RI and Savings
/// Plans utilization or coverage budgets default to `100`. This is the only valid value
/// for RI or Savings Plans utilization or coverage budgets" (`Budget$BudgetLimit`).
///
/// Spelled out rather than taken as the complement of [`LIMIT_REQUIRED_BUDGET_TYPES`]:
/// these are the four members of the `BudgetType` enum that sentence names, so a member
/// the model adds later gets no limit invented for it. The list is pinned against the
/// generated `BUDGET_TYPE_VALUES` catalog by
/// `tests/all/service_owned_budget_members.rs`.
pub const PERCENTAGE_LIMIT_BUDGET_TYPES: [&str; 4] = [
    "RI_UTILIZATION",
    "RI_COVERAGE",
    "SAVINGS_PLANS_UTILIZATION",
    "SAVINGS_PLANS_COVERAGE",
];

/// "RI and Savings Plans utilization or coverage budgets default to `100`. This is the
/// only valid value for RI or Savings Plans utilization or coverage budgets"
/// (`Budget$BudgetLimit`) — the `Spend$Amount` such a budget is stored with when the
/// request carries no `BudgetLimit`, and the only one a request may name for it
/// ([`validate_percentage_limit_amount`]). `NumericValue` is a string
/// (`^([0-9]*\.)?[0-9]+$`), so the value is written as the digits AWS documents.
pub const PERCENTAGE_LIMIT_AMOUNT: &str = "100";

/// The `Spend$Unit` of the limit above — a unit this emulator CHOOSES, not one the model
/// states: `Budget$BudgetLimit` documents the defaulted amount (`100`) and nothing about
/// the unit it is measured in, and `UnitValue`'s `.*` pattern constrains nothing either.
///
/// A utilization or coverage budget's limit is a percentage (the `100` it defaults to is
/// "the only valid value", which no currency or usage unit bounds), so the unit is spelled
/// the way this model spells that unit elsewhere (`ThresholdType$PERCENTAGE`, for a
/// threshold that is "a certain percentage"). A recorded RI/SP `DescribeBudget` response
/// is what would pin it; until one is on hand, a caller that cares sends its own
/// `BudgetLimit`, which is always stored as sent.
pub const PERCENTAGE_LIMIT_UNIT: &str = "PERCENTAGE";

/// The `TimeUnit` members a `PlannedBudgetLimits` map can be written for:
/// "`PlannedBudgetLimits` ... supports both monthly and quarterly `TimeUnit`"
/// (`Budget$PlannedBudgetLimits`). The map's key is "the start of the month" or "the
/// start of the quarter", so a budget on any other granularity has no planned periods to
/// key them by.
const PLANNED_LIMIT_TIME_UNITS: [&str; 2] = ["MONTHLY", "QUARTERLY"];

/// `AutoAdjustType$HISTORICAL` — the auto-adjust type whose budget amount comes from
/// historical data, so it is the one that takes `HistoricalOptions` ("the parameters that
/// define or describe the historical data that your auto-adjusting budget is based on",
/// `AutoAdjustData$HistoricalOptions`). The other member, `FORECAST`, adjusts from
/// forecasted data and so has no historical parameters.
const AUTO_ADJUST_TYPE_HISTORICAL: &str = "HISTORICAL";

/// The maximum `BudgetAdjustmentPeriod` for each `TimeUnit` granularity, from
/// `HistoricalOptions$BudgetAdjustmentPeriod`: "The maximum value depends on the
/// `TimeUnit` granularity of the budget: For the `DAILY` granularity, the maximum value
/// is `60`. For the `MONTHLY` granularity, the maximum value is `12`. For the
/// `QUARTERLY` granularity, the maximum value is `4`. For the `ANNUALLY` granularity,
/// the maximum value is `1`."
///
/// A `TimeUnit` that sentence does not name (`CUSTOM`, or one the model adds later)
/// names no period length, so only the modelled `@range(1, 60)` bounds it — and that
/// bound is already enforced at request validation.
const MAX_BUDGET_ADJUSTMENT_PERIOD: [(&str, i32); 4] = [
    ("DAILY", 60),
    ("MONTHLY", 12),
    ("QUARTERLY", 4),
    ("ANNUALLY", 1),
];

/// The documented maximum [`MAX_BUDGET_ADJUSTMENT_PERIOD`] holds for `time_unit`, or
/// `None` for a granularity it does not name.
fn max_budget_adjustment_period(time_unit: &str) -> Option<i32> {
    MAX_BUDGET_ADJUSTMENT_PERIOD
        .iter()
        .find(|(unit, _)| *unit == time_unit)
        .map(|(_, max)| *max)
}

/// The instant `secs` seconds after the Unix epoch, or `None` when that is no instant
/// this host can represent: an epoch before the epoch, or one so far after it that the
/// platform's clock type cannot hold it. Neither names a period a budget can be keyed
/// by, and the second is reachable from a request (a `PlannedBudgetLimits` key is a
/// caller-supplied decimal), so the addition is CHECKED — `SystemTime`'s own `Add` is an
/// `expect` on overflow, which would unwind the handler instead of refusing the value.
fn instant_at(secs: i64) -> Option<SystemTime> {
    let secs = u64::try_from(secs).ok()?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(secs))
}

/// Whether `secs` is exactly the start of a period of `time_unit` — the midnight-UTC
/// boundary [`default_time_period_start`] computes for the period containing it, which
/// equals the instant itself only when the instant IS that boundary.
fn is_period_start(time_unit: &str, secs: i64) -> bool {
    let Some(at) = instant_at(secs) else {
        return false;
    };
    // An instant the calendar cannot render names no period at all.
    // [`default_time_period_start`] answers such an instant with the instant itself (the
    // only sound answer it has for "the period containing it"), which would make every
    // unrenderable epoch look like its own boundary — so renderability is required here
    // before that comparison means anything.
    if civil_date(at).is_none() {
        return false;
    }
    default_time_period_start(time_unit, at).secs() == secs
}

/// The `TimePeriod` a write of `budget` at `at` LEAVES on it: each bound the request
/// carried, and the documented default for each it did not — "If you create your budget
/// and don't specify a start date, Amazon Web Services defaults to the start of your
/// chosen time period ... If you didn't specify an end date, Amazon Web Services set your
/// end date to `06/15/87 00:00 UTC`" (`Budget$TimePeriod`, via
/// [`default_time_period_start`] / [`default_time_period_end`]).
///
/// The one answer to "what period will this budget cover?", so the write path that stores
/// it ([`stamp_service_owned`]) and the rules that judge it ([`validate_time_period`])
/// cannot come to disagree: a cross-member rule over the period is checked against the
/// period that will be STORED, not only against the bounds the request spelled out.
pub(crate) fn stamped_time_period(budget: &Budget, at: SystemTime) -> TimePeriod {
    let carried = budget.time_period.as_ref();
    TimePeriod {
        start: carried
            .and_then(|period| period.start)
            .or_else(|| Some(default_time_period_start(&budget.time_unit, at))),
        end: carried
            .and_then(|period| period.end)
            .or_else(|| Some(default_time_period_end())),
    }
}

/// Stamp the members of `budget` the SERVICE owns, on the `CreateBudget` /
/// `UpdateBudget` write path, so that every later read reports what was stored rather
/// than recomputing it:
///
/// * `LastUpdatedTime` — "the last time that you updated this budget", so a value the
///   caller sent is replaced by the time of this write.
/// * `CreatedAt` — the instant the budget CAME INTO BEING. `previous` carries the budget
///   this write replaces, if any: a write that edits a budget keeps the creation instant
///   that budget already carried, since editing a budget does not create one, and only a
///   write that establishes a budget stamps this instant. Recorded behaviour: the shipped
///   model snapshot declares no such member, so this crate declares it itself (see
///   [`crate::ops::types::Budget`]).
/// * `TimePeriod` — a budget always covers a period; an absent `Start` or `End` takes
///   its documented default (see [`stamped_time_period`]).
/// * `CostTypes` — the unset members of a `COST` budget's cost types take their
///   documented defaults, and a budget whose type has no cost types at all is stored
///   with none whatever the request carried (see [`settle_cost_types`]).
/// * `HealthStatus` — the operational state of the billing-view-derived resource, which
///   the caller does not set. The status the budget already had (out of `previous`) is
///   kept: the status did not change because somebody edited the budget, so neither does
///   its `LastUpdatedTime`.
/// * the members the service DERIVES from the cost and usage it tracks, which a request
///   cannot set at all (see [`drop_service_derived`]).
/// * `BudgetLimit` — the documented default of a utilization or coverage budget whose
///   request carried none (see [`apply_percentage_limit_default`]).
pub fn stamp_service_owned(budget: &mut Budget, at: SystemTime, previous: Option<&Budget>) {
    let stamped = ::aws_smithy_types::DateTime::from(at);
    budget.last_updated_time = Some(stamped);
    budget.created_at = previous
        .and_then(|held| held.created_at)
        .or(Some(stamped));

    drop_service_derived(budget);
    apply_percentage_limit_default(budget);

    budget.time_period = Some(stamped_time_period(budget, at));

    settle_cost_types(budget);

    budget.health_status = Some(
        previous
            .and_then(|held| held.health_status.clone())
            .unwrap_or_else(|| HealthStatus {
                status: Some(HEALTH_STATUS_HEALTHY.to_string()),
                status_reason: None,
                last_updated_time: Some(stamped),
            }),
    );
}

/// Discard every member of `budget` whose value the SERVICE derives from the cost and
/// usage data it tracks. A request cannot set any of them, so the write path drops what
/// it carried instead of storing it and echoing it back from every later read — the same
/// treatment a caller-supplied `LastUpdatedTime` already gets.
///
/// This emulator tracks no cost or usage behind a budget: it holds budgets, their
/// notifications and their tags, and nothing that spends. So each of these members is
/// stored ABSENT rather than filled with a figure nothing computed:
///
/// * `CalculatedSpend` — "the actual and forecasted cost or usage that the budget
///   tracks" (`Budget$CalculatedSpend`), and the one member `UpdateBudget` documents a
///   caller cannot change ("you can change every part of a budget except for the
///   `budgetName` and the `calculatedSpend`"). The service computes it from the usage it
///   has attributed to the budget, so the write path stores none and every read reports
///   the computed figure instead (see [`calculated_spend`]).
/// * `AutoAdjustData$LastAutoAdjustTime` — "the last time that your budget was
///   auto-adjusted". Adjusting an auto-adjusting budget's amount means averaging the
///   account's historical or forecasted cost, which this emulator holds none of, so it
///   never adjusts one: there is no such time, and stamping the write instant would
///   report an adjustment that never ran.
/// * `HistoricalOptions$LookBackAvailablePeriods` — "you can't set your own
///   `LookBackAvailablePeriods`. The value is automatically calculated from the
///   `BudgetAdjustmentPeriod` and your historical cost data", counting the periods of
///   that window that have cost data. None do here, and the member's modelled
///   `@range(min: 1)` cannot express a count of zero, so absence is the only answer the
///   model allows.
///
/// The members of the same shapes the CALLER owns are left exactly as sent:
/// `AutoAdjustData$AutoAdjustType` and `HistoricalOptions$BudgetAdjustmentPeriod` are
/// "the parameters that determine the budget amount", which is a caller's choice.
fn drop_service_derived(budget: &mut Budget) {
    budget.calculated_spend = None;
    if let Some(auto_adjust) = budget.auto_adjust_data.as_mut() {
        auto_adjust.last_auto_adjust_time = None;
        if let Some(options) = auto_adjust.historical_options.as_mut() {
            options.look_back_available_periods = None;
        }
    }
}

/// Store the documented default `BudgetLimit` of a utilization or coverage budget whose
/// request carried none: "`BudgetLimit` is required for cost or usage budgets, but
/// optional for RI or Savings Plans utilization or coverage budgets. RI and Savings Plans
/// utilization or coverage budgets default to `100`. This is the only valid value for RI
/// or Savings Plans utilization or coverage budgets" (`Budget$BudgetLimit`) — so such a
/// budget always reads back with a limit, the way a budget written without a `TimePeriod`
/// reads back with the defaulted one.
///
/// A limit the caller DID send is kept as sent — for these budget types the only amount a
/// request can have got this far with is the one value above, since
/// [`validate_percentage_limit_amount`] refuses any other before anything is stored, while
/// the unit is the caller's own. No default is invented for any other budget type:
/// `BudgetLimit` is required for a cost or usage budget
/// ([`validate_budget_limit_present`] refuses one that carries no limit at all), and an
/// auto-adjusting budget's amount is the average of cost data this emulator does not hold
/// (see [`drop_service_derived`]), so none is derived for it either.
///
/// Defaulting here cannot produce the combination `Budget$BudgetLimit` forbids
/// ("you can't use `BudgetLimit` with `PlannedBudgetLimits`"): `PlannedBudgetLimits` "is
/// available for cost or usage budget", which [`validate_planned_budget_limits`] already
/// refuses on exactly the budget types defaulted here.
fn apply_percentage_limit_default(budget: &mut Budget) {
    if budget.budget_limit.is_some()
        || !PERCENTAGE_LIMIT_BUDGET_TYPES.contains(&budget.budget_type.as_str())
    {
        return;
    }
    budget.budget_limit = Some(Spend {
        amount: PERCENTAGE_LIMIT_AMOUNT.to_string(),
        unit: PERCENTAGE_LIMIT_UNIT.to_string(),
    });
}

/// Settle the members of `notification` the SERVICE owns, on every write path that stores
/// one, so that nothing a request carried for them is persisted or echoed back by a later
/// read:
///
/// * `NotificationState` "specifies whether this notification is in alarm. If a budget
///   notification is in the `ALARM` state, you passed the set threshold for the budget"
///   (`Notification$NotificationState`): it is the service's own reading of the spend it
///   tracks against the notification's threshold, not a value a caller chooses — exactly
///   like `Budget$CalculatedSpend`, which the write path drops for the same reason
///   ([`drop_service_derived`]). Every read reports the state computed from the budget's
///   own figures instead ([`notification_state`]).
/// * `ConsecutiveBreachCount` is counted off those same figures
///   ([`consecutive_breach_count`]) and is dropped here for the same reason.
/// * `ThresholdType` is stored only when it says something the default does not:
///   "thresholds are always a percentage" (`Notification$Threshold`), so
///   [`THRESHOLD_TYPE_PERCENTAGE`] is what an absent member already means and a request
///   that spells it out names the same notification as one that leaves it out
///   ([`same_notification`]). The service keeps the one spelling, so no read reports a
///   member whose value is the default — the member set a read reports does not depend on
///   whether the create wrote the default out.
pub fn stamp_notification_owned(notification: &mut Notification) {
    notification.notification_state = None;
    notification.consecutive_breach_count = None;
    if notification
        .threshold_type
        .as_deref()
        .is_some_and(|named| named == THRESHOLD_TYPE_PERCENTAGE)
    {
        notification.threshold_type = None;
    }
}

/// `notification` as a READ reports it: the members the caller supplied, plus the two the
/// service counts off the figures behind the budget that owns it — the
/// `NotificationState` it is in ([`notification_state`]) and how many periods in a row it
/// has been in that state ([`consecutive_breach_count`]).
///
/// Both are read off `stored` as of `now`, the same figures `DescribeBudget` reports for
/// that budget, so no two reads of one notification can disagree. Nothing is written.
pub fn project_notification(
    stored: &StoredBudget,
    notification: &Notification,
    now: SystemTime,
) -> Notification {
    let budget = project_budget(stored, now);
    let mut reported = notification.clone();
    reported.notification_state = Some(notification_state(&budget, notification).to_string());
    reported.consecutive_breach_count = Some(consecutive_breach_count(stored, notification, now));
    reported
}

/// How many budget periods IN A ROW, ending with the period in progress, the spend
/// `notification` watches has passed the threshold it alerts on — the count a read reports
/// for `Notification$ConsecutiveBreachCount`.
///
/// Recorded behaviour: the SHIPPED model snapshot declares no such member, so this crate
/// declares it itself (see [`crate::ops::types::Notification`]). What it counts follows from
/// what a notification IS — one alert condition over the spend a budget tracks, evaluated
/// per period of that budget — so it is counted over the periods the budget has actually
/// been measured over ([`covered_periods`]), each judged on the figures that period ran
/// under ([`budget_as_read_at_epoch`]), newest first and stopping at the first period that
/// was not in breach.
///
/// `0` for a notification that is not in alarm now, whatever happened before it, and for a
/// budget none of whose periods has begun: nothing has been evaluated yet, so nothing has
/// breached. The count is therefore non-zero exactly while the notification's own
/// `NotificationState` is [`NOTIFICATION_STATE_ALARM`], since both read the same figures.
pub fn consecutive_breach_count(
    stored: &StoredBudget,
    notification: &Notification,
    now: SystemTime,
) -> i32 {
    let breaches = covered_periods(&stored.budget, now)
        .iter()
        .rev()
        .take_while(|period| {
            let at = period.start.as_ref().map_or(i64::MIN, |start| start.secs());
            notification_in_alarm(&budget_as_read_at_epoch(&stored.budget, at), notification)
        })
        .count();
    // The series is bounded by the periods a budget retains ([`RETAINED_PERIODS`]), so the
    // count is a small number; the saturating conversion is what keeps the member's type
    // honest rather than a cast that could wrap.
    i32::try_from(breaches).unwrap_or(i32::MAX)
}

/// The `NotificationState` the service reports for `notification` of `budget` — whether
/// the notification is in alarm, as of the figures `budget` reads back with.
///
/// A notification is one alert condition over the spend the budget tracks: which spend it
/// watches is `NotificationType` ("whether the notification is for how much you have
/// spent (`ACTUAL`) or for how much that you're forecasted to spend (`FORECASTED`)"),
/// what it is measured against is `Threshold` read through `ThresholdType`
/// ([`threshold_amount`]), and how the two are compared is `ComparisonOperator`. The
/// notification is in [`NOTIFICATION_STATE_ALARM`] exactly while that comparison holds —
/// "if a budget notification is in the `ALARM` state, you passed the set threshold for
/// the budget" — and in [`NOTIFICATION_STATE_OK`] otherwise.
///
/// `OK` is also the answer wherever there is nothing to compare, because nothing has
/// passed a threshold that was never measured: a `FORECASTED` notification of a budget no
/// forecast is reported for (every budget here — see [`calculated_spend`]), a budget whose
/// read reports no `CalculatedSpend` at all, a `PERCENTAGE` threshold of a budget that
/// reports no limit to take a percentage of, and a comparison or threshold type this
/// service does not know.
pub fn notification_state(budget: &Budget, notification: &Notification) -> &'static str {
    if notification_in_alarm(budget, notification) {
        NOTIFICATION_STATE_ALARM
    } else {
        NOTIFICATION_STATE_OK
    }
}

/// Whether the spend `notification` watches has passed the threshold it alerts on.
fn notification_in_alarm(budget: &Budget, notification: &Notification) -> bool {
    let Some(spent) = watched_spend(budget, notification) else {
        return false;
    };
    let Some(threshold) = threshold_amount(budget, notification) else {
        return false;
    };
    match notification.comparison_operator.as_str() {
        COMPARISON_OPERATOR_GREATER_THAN => spent > threshold,
        COMPARISON_OPERATOR_LESS_THAN => spent < threshold,
        COMPARISON_OPERATOR_EQUAL_TO => spent == threshold,
        // A comparison this service does not know is one it cannot make.
        _ => false,
    }
}

/// The figure `notification` watches, out of the `CalculatedSpend` `budget` reads back
/// with: the actual spend for an `ACTUAL` notification, the forecast for a `FORECASTED`
/// one. `None` when the budget reports no such figure.
fn watched_spend(budget: &Budget, notification: &Notification) -> Option<f64> {
    spend_watched_by(budget, &notification.notification_type)
}

/// The figure a `NotificationType` watches, out of the `CalculatedSpend` `budget` reads
/// back with: "whether the notification is for how much you have spent (`ACTUAL`) or for
/// how much that you're forecasted to spend (`FORECASTED`)".
///
/// `None` when the budget reports no such figure — including every `FORECASTED` reading
/// here, since no forecast is reported for a budget of this emulator (see
/// [`calculated_spend`]) — and for a notification type this service does not know.
///
/// The ONE reading of which spend a threshold is measured against, shared by a
/// notification's alarm state ([`notification_state`]) and an action's trigger
/// ([`action_threshold_passed`]), so the two cannot come to watch different figures.
pub fn spend_watched_by(budget: &Budget, notification_type: &str) -> Option<f64> {
    let spend = budget.calculated_spend.as_ref()?;
    let watched = match notification_type {
        NOTIFICATION_TYPE_ACTUAL => Some(&spend.actual_spend),
        NOTIFICATION_TYPE_FORECASTED => spend.forecasted_spend.as_ref(),
        // A notification type this service does not know watches nothing it holds.
        _ => None,
    }?;
    numeric_amount(&watched.amount)
}

/// The amount `notification`'s `Threshold` names, in the unit the watched spend is
/// measured in.
///
/// `ThresholdType$ABSOLUTE_VALUE` is that amount itself ("Amazon Web Services notifies you
/// when you go over or are forecasted to go over your total cost threshold"), while
/// `PERCENTAGE` — the type a notification that names none carries, since "thresholds are
/// always a percentage" — is that percentage OF the budgeted amount: "if you have a budget
/// for 200 dollars and you have a `PERCENTAGE` threshold of 80%, Amazon Web Services
/// notifies you when you go over 160 dollars" (`Notification$ThresholdType`). So a budget
/// whose read reports no `BudgetLimit` has no amount for a percentage to be of, and there
/// is nothing to compare.
fn threshold_amount(budget: &Budget, notification: &Notification) -> Option<f64> {
    threshold_in_budget_unit(
        budget,
        notification.threshold,
        notification
            .threshold_type
            .as_deref()
            .unwrap_or(THRESHOLD_TYPE_PERCENTAGE),
    )
}

/// `threshold` read through `threshold_type`, in the unit the watched spend is measured in.
///
/// The ONE reading of a `ThresholdType`, shared by a notification's threshold and an
/// action's ([`action_threshold_passed`]), both of which are written as a value plus that
/// type: `ABSOLUTE_VALUE` is the amount itself, `PERCENTAGE` is that percentage of the
/// budgeted amount. `None` for a percentage of a budget whose read reports no limit — there
/// is no amount for it to be a percentage of — and for a threshold type this service does
/// not know.
pub fn threshold_in_budget_unit(
    budget: &Budget,
    threshold: f64,
    threshold_type: &str,
) -> Option<f64> {
    match threshold_type {
        THRESHOLD_TYPE_ABSOLUTE_VALUE => Some(threshold),
        THRESHOLD_TYPE_PERCENTAGE => {
            let limit = numeric_amount(&budget.budget_limit.as_ref()?.amount)?;
            Some(limit * threshold / PERCENT)
        }
        // A threshold type this service does not know names no amount.
        _ => None,
    }
}

/// The number `amount` names, for a `Spend$Amount` or `BudgetLimit` figure that is about
/// to be compared. `NumericValue` is a STRING (`^([0-9]*\.)?[0-9]+$`), so the comparison
/// reads the number out of it rather than comparing the spelling; `None` for anything that
/// is not a finite number.
///
/// The comparison that follows is therefore binary-floating-point: a limit written with
/// more significant digits than a `f64` carries is compared as the nearest one it has.
fn numeric_amount(amount: &str) -> Option<f64> {
    amount
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

/// The whole of a `PERCENTAGE` threshold's base: a threshold of `100` is the budgeted
/// amount itself.
const PERCENT: f64 = 100.0;

/// `ThresholdType$ABSOLUTE_VALUE` — the threshold read as an amount in the budget's own
/// unit rather than as a percentage of the budgeted amount.
pub const THRESHOLD_TYPE_ABSOLUTE_VALUE: &str = "ABSOLUTE_VALUE";

/// `NotificationType$ACTUAL` — the notification that watches "how much you have spent".
pub const NOTIFICATION_TYPE_ACTUAL: &str = "ACTUAL";

/// `NotificationType$FORECASTED` — the notification that watches "how much that you're
/// forecasted to spend".
pub const NOTIFICATION_TYPE_FORECASTED: &str = "FORECASTED";

/// `ComparisonOperator$GREATER_THAN` — the watched spend is above the threshold.
pub const COMPARISON_OPERATOR_GREATER_THAN: &str = "GREATER_THAN";

/// `ComparisonOperator$LESS_THAN` — the watched spend is below the threshold.
pub const COMPARISON_OPERATOR_LESS_THAN: &str = "LESS_THAN";

/// `ComparisonOperator$EQUAL_TO` — the watched spend is exactly the threshold.
pub const COMPARISON_OPERATOR_EQUAL_TO: &str = "EQUAL_TO";

/// `ThresholdType$PERCENTAGE` — the threshold type a notification that names none
/// carries: "Thresholds are always a percentage" (`Notification$Threshold`), so an
/// absent `ThresholdType` and an explicit `PERCENTAGE` describe the same threshold.
pub const THRESHOLD_TYPE_PERCENTAGE: &str = "PERCENTAGE";

/// Whether `left` and `right` name the SAME notification of a budget.
///
/// A notification carries no id of its own, so every operation that acts on one
/// (`DeleteNotification`, `UpdateNotification`, `DeleteSubscriber`, …) names it by
/// passing the whole `Notification` shape back. Its identity is therefore the members
/// that describe WHICH notification it is — the notification type, the comparison
/// operator, the threshold and the threshold type (absent meaning
/// [`THRESHOLD_TYPE_PERCENTAGE`], since "Thresholds are always a percentage").
///
/// `NotificationState` is deliberately NOT part of it: it "specifies whether this
/// notification is in alarm" (`Notification$NotificationState`), a state the service
/// moves on its own as spend crosses the threshold, so a caller naming a notification
/// cannot be required to guess the alarm state it is in at that instant.
///
/// The threshold is a `double`, so it is compared as the VALUE it is rather than with
/// IEEE equality ([`same_threshold`]).
pub fn same_notification(left: &Notification, right: &Notification) -> bool {
    fn threshold_type(notification: &Notification) -> &str {
        notification
            .threshold_type
            .as_deref()
            .unwrap_or(THRESHOLD_TYPE_PERCENTAGE)
    }
    left.notification_type == right.notification_type
        && left.comparison_operator == right.comparison_operator
        && same_threshold(left.threshold, right.threshold)
        && threshold_type(left) == threshold_type(right)
}

/// Whether two `Threshold`s are the same threshold, for the identity
/// [`same_notification`] is made of.
///
/// `Notification$Threshold` is a modelled `double`, and awsJson spells the non-finite
/// doubles as the strings `"NaN"`, `"Infinity"` and `"-Infinity"`, so a threshold that
/// is not a number is a value a caller can send. IEEE equality answers false for such a
/// value even against ITSELF, so an identity resting on `==` alone would hold a
/// notification that does not name itself: the budget could be given a second copy of
/// it where [`attaches_duplicate_notification`] says none may exist, and no later read,
/// update or delete could resolve either copy.
///
/// Identity is therefore reflexive over every value the member admits — two thresholds
/// name the same notification exactly when they are the same value, with all the
/// not-a-number thresholds being one value of their own, and the two spellings of zero
/// the single value they both name.
fn same_threshold(left: f64, right: f64) -> bool {
    left == right || (left.is_nan() && right.is_nan())
}

/// The notification of `stored` that `wanted` names ([`same_notification`]), or `None`
/// when the budget holds no such notification.
pub fn find_notification<'a>(
    stored: &'a StoredBudget,
    wanted: &Notification,
) -> Option<&'a StoredNotification> {
    stored
        .notifications
        .iter()
        .find(|held| same_notification(&held.notification, wanted))
}

/// As [`find_notification`], for the write paths that change a notification or the
/// subscribers held with it (`CreateSubscriber`, …): the same identity
/// ([`same_notification`]), handed back by mutable reference.
pub fn find_notification_mut<'a>(
    stored: &'a mut StoredBudget,
    wanted: &Notification,
) -> Option<&'a mut StoredNotification> {
    stored
        .notifications
        .iter_mut()
        .find(|held| same_notification(&held.notification, wanted))
}

/// The sentence refusing a notification a budget already holds, naming the budget it
/// would have belonged to.
///
/// One rule is being refused — a budget holds at most one notification per identity
/// ([`same_notification`]) — so every write that ATTACHES a notification words the
/// refusal from here, whether the notification arrives on its own
/// (`CreateNotification`) or in the set a `CreateBudget` attaches with the budget. The
/// notification itself is not named: its identity is four members a caller supplied in
/// the very request being refused.
pub fn duplicate_notification_refusal(budget_name: &str) -> String {
    format!("Unable to create notification: {budget_name} - the notification already exists.")
}

/// Whether the set of notifications ONE call would attach leaves a budget that already
/// holds `already_held` of them holding two notifications of one identity.
///
/// A notification's identity is what it alerts on ([`same_notification`]) and every
/// operation that acts on one names it by value, resolving the FIRST match
/// ([`find_notification`]) — so a budget holding a second notification of one identity
/// holds it where no later read, update or delete could reach it, and a delete of "the"
/// notification would leave a copy behind. A write that attaches a whole SET is
/// therefore held to the rule the single-notification path enforces, over what the
/// budget would END UP holding: the attached set is checked against the notifications
/// the budget already holds AND against its own earlier entries, so one call cannot
/// establish the state a second call is refused for.
pub fn attaches_duplicate_notification(
    already_held: &[StoredNotification],
    attaching: &[NotificationWithSubscribers],
) -> bool {
    let mut set: Vec<&Notification> = already_held.iter().map(|held| &held.notification).collect();
    for supplied in attaching {
        if set
            .iter()
            .any(|held| same_notification(held, &supplied.notification))
        {
            return true;
        }
        set.push(&supplied.notification);
    }
    false
}

/// Whether `left` and `right` name the SAME subscriber of a notification.
///
/// A subscriber carries no id either: every operation that acts on one
/// (`CreateSubscriber`, `DeleteSubscriber`, `UpdateSubscriber`) names it by passing the
/// whole `Subscriber` shape back, and that shape "consists of a subscription type and
/// either an Amazon SNS topic or an email address" (`Subscriber`) — both of its members,
/// both `@required`, so both are its identity. The same address reached over a different
/// `SubscriptionType` is a different subscriber.
pub fn same_subscriber(left: &Subscriber, right: &Subscriber) -> bool {
    left.subscription_type == right.subscription_type && left.address == right.address
}

/// Whether `set` already holds the subscriber `candidate` names ([`same_subscriber`]) —
/// the one place the set rule is applied, by both the helper that builds a set
/// ([`distinct_subscribers`]) and the one that counts its capacities
/// ([`subscriber_limit_violation`]).
fn set_holds<'a>(set: impl IntoIterator<Item = &'a Subscriber>, candidate: &Subscriber) -> bool {
    set.into_iter()
        .any(|held| same_subscriber(held, candidate))
}

/// The subscriber SET that `supplied` names: the same subscribers, with every repeat of
/// one already in the set collapsed into it, in the order they were first supplied.
///
/// A subscriber's two members ARE its identity ([`same_subscriber`]) — nothing else
/// distinguishes one — so two entries that agree on both name the SAME subscriber, and
/// the set a notification or an action ends up holding can hold it once. A repeat is
/// therefore not a second subscriber: every write that establishes a subscriber set from
/// a supplied list passes it through here, so no read reports one subscriber twice and
/// nothing a later `DeleteSubscriber` / `UpdateSubscriber` could never name is stored.
pub fn distinct_subscribers(supplied: impl IntoIterator<Item = Subscriber>) -> Vec<Subscriber> {
    let mut set: Vec<Subscriber> = Vec::new();
    for mut subscriber in supplied {
        drop_subscriber_derived(&mut subscriber);
        if !set_holds(set.iter(), &subscriber) {
            set.push(subscriber);
        }
    }
    set
}

/// Discard the member of `subscriber` whose value the SERVICE owns, on every write path
/// that stores one — the same treatment a notification's own derived member gets
/// ([`stamp_notification_owned`]).
///
/// `VerificationStatus` says whether the address has been CONFIRMED, which is a fact
/// about the mailbox or topic behind it rather than anything a request can assert: a
/// caller that could set it could claim an address it does not own had been confirmed.
/// So nothing a request carried for it is persisted, and the write path that establishes
/// a set the service does report it for stamps it instead
/// ([`action_subscribers`]).
pub fn drop_subscriber_derived(subscriber: &mut Subscriber) {
    subscriber.verification_status = None;
}

/// `VerificationStatus` for an address nobody has confirmed — the state a subscriber of a
/// budget action is established in and the only one this service's records reach.
///
/// Recorded behaviour: the SHIPPED model snapshot declares no such member, so this crate
/// declares it itself (see [`crate::ops::types::Subscriber`]). An address is confirmed by
/// whoever holds it, out of band: an email subscriber has to accept the subscription and
/// an SNS topic subscription has to be confirmed on the topic, and no operation of this
/// service — nor any call it makes — does either. So the status this service records for a
/// subscriber stays the one its write stamped, for the life of the action that holds it.
pub const SUBSCRIBER_VERIFICATION_PENDING: &str = "PENDING";

/// The subscriber set a budget ACTION write establishes: the set the supplied list names
/// ([`distinct_subscribers`]), each subscriber carrying the verification state this
/// service records for its address ([`SUBSCRIBER_VERIFICATION_PENDING`]).
///
/// An action's subscribers are the ones a read reports that state for — the recorded
/// `Action$Subscribers` carry it, while the recorded reads of a NOTIFICATION's
/// subscribers carry only the two members the model declares — so it is stamped on the
/// action write paths (`CreateBudgetAction`, `UpdateBudgetAction`) rather than on every
/// set this service stores, and a read reports what the write stamped rather than
/// deciding it per request.
pub fn action_subscribers(supplied: impl IntoIterator<Item = Subscriber>) -> Vec<Subscriber> {
    distinct_subscribers(supplied)
        .into_iter()
        .map(|subscriber| Subscriber {
            verification_status: Some(SUBSCRIBER_VERIFICATION_PENDING.to_string()),
            ..subscriber
        })
        .collect()
}

/// "A budget can have up to ten notifications" (`Notification`), which is also the
/// modelled `@length(max: 10)` of `NotificationWithSubscribersList` — the list a single
/// `CreateBudget` attaches. It is a capacity of the BUDGET, so it is the same ceiling
/// however the notifications arrive: `CreateNotification` answers
/// `CreationLimitExceededException` ("You've exceeded the notification or subscriber
/// limit") once a budget already holds this many, and `CreateBudget` answers it for a
/// set that would put the budget it creates over it.
pub const MAX_NOTIFICATIONS_PER_BUDGET: usize = 10;

/// "Each notification must have at least one subscriber. A notification can have one
/// SNS subscriber and up to 10 email subscribers, for a total of 11 subscribers"
/// (`Notification`). The at-least-one and the total of 11 are the modelled
/// `@length(min: 1, max: 11)` on `Subscribers`, refused at request validation before a
/// handler runs; the PER-TYPE halves are these two, which no constraint trait can
/// express — [`subscriber_limit_violation`] enforces them.
///
/// The split is a property of a SUBSCRIBER SET, not of notifications alone: every set
/// this service stores — a notification's and a budget action's — is the one modelled
/// `Subscribers` shape with the same `@length(1..11)` total, so the same two halves add
/// up to it wherever a write establishes one.
pub const MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION: usize = 1;

/// See [`MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION`].
pub const MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION: usize = 10;

/// `SubscriptionType$SNS` — "the type of notification that Amazon Web Services sends
/// to a subscriber", for a subscriber that is an Amazon SNS topic.
const SUBSCRIPTION_TYPE_SNS: &str = "SNS";

/// `SubscriptionType$EMAIL` — a subscriber that is an email address.
const SUBSCRIPTION_TYPE_EMAIL: &str = "EMAIL";

/// The per-type subscriber limits of ONE subscriber set — a notification's or a budget
/// action's — checked over the subscribers it would end up with. `Err` carries the
/// sentence that goes in the refusal's `Message`; `Ok` means the set is within both
/// halves of [`MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION`] /
/// [`MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION`].
///
/// Takes any iterator of subscribers, so a caller that grows a stored set by one
/// (`CreateSubscriber`) checks the set it would leave behind without building it.
///
/// The limits are capacities of a SET, so they are counted over DISTINCT subscribers
/// ([`same_subscriber`]): a repeat names a subscriber the set already holds
/// ([`distinct_subscribers`] collapses it on the way in), so it occupies no capacity of
/// its own.
pub fn subscriber_limit_violation<'a>(
    subscribers: impl IntoIterator<Item = &'a Subscriber>,
) -> Result<(), String> {
    let mut sns = 0_usize;
    let mut email = 0_usize;
    let mut counted: Vec<&Subscriber> = Vec::new();
    for subscriber in subscribers {
        if set_holds(counted.iter().copied(), subscriber) {
            continue;
        }
        counted.push(subscriber);
        match subscriber.subscription_type.as_str() {
            SUBSCRIPTION_TYPE_SNS => sns += 1,
            SUBSCRIPTION_TYPE_EMAIL => email += 1,
            // A subscription type the model does not list cannot reach here: the member
            // is a modelled enum, refused at request validation.
            _ => {}
        }
    }
    for (kind, limit, supplied) in [
        (
            SUBSCRIPTION_TYPE_SNS,
            MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION,
            sns,
        ),
        (
            SUBSCRIPTION_TYPE_EMAIL,
            MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION,
            email,
        ),
    ] {
        if supplied > limit {
            return Err(format!(
                "A set of subscribers can have at most {limit} {kind} subscriber(s), and \
                 {supplied} distinct {kind} subscriber(s) were supplied."
            ));
        }
    }
    Ok(())
}

/// Amazon SNS's own ARN namespace: the vendor field an address has to carry for it to
/// name an SNS topic at all ([`sns_topic`]), which is also the prefix SNS is
/// addressed by in a Region (the endpoint [`alerts`] publishes an alert to).
pub const SNS_NAMESPACE: &str = "sns";

/// The sentence refusing an address that carries a line break — `Subscriber$Address`'s
/// own sentence, which is the rule it states for an address of EITHER subscription type:
/// "When you create a subscriber, the value of `Address` can't contain line breaks."
const ADDRESS_LINE_BREAK_REFUSAL: &str =
    "When you create a subscriber, the value of Address can't contain line breaks.";

/// The sentence refusing an `EMAIL` subscriber whose address is not an email address,
/// stating the half of `Subscriber`'s rule that type names ("a subscription type and
/// either an Amazon SNS topic or an email address") about the member it was violated on.
const EMAIL_ADDRESS_REFUSAL: &str = "The Address of an EMAIL subscriber must be an email address.";

/// The sentence refusing an `SNS` subscriber whose address does not name a topic — the
/// other half of the same rule, as `Subscriber$Address` words it ("either an SNS topic or
/// an email").
const SNS_ADDRESS_REFUSAL: &str =
    "The Address of an SNS subscriber must be the ARN of an Amazon SNS topic.";

/// The sentence refusing a subscriber whose subscription type names NO address form —
/// `Subscriber`'s rule with neither half selected, for a type outside the two the model
/// lists (see [`subscriber_address_violation`]).
const SUBSCRIBER_FORM_REFUSAL: &str =
    "The Address of a subscriber must be either an Amazon SNS topic or an email address.";

/// The sentence refusing an `SNS` subscriber whose address names a topic of ANOTHER
/// account, stating the rule as `Subscriber$Address` words it ("the address that Amazon
/// Web Services sends budget notifications to") about the account a notification of this
/// budget is sent in: the account that owns the budget, which is the account whose topics
/// its alerts are published to ([`publishable_topic`]).
///
/// It names the account the REQUEST itself supplied, never the refused address nor the
/// account that address named: `Subscriber$Address` is `@sensitive`, so a refusal that
/// echoed either field of it would put a caller's topic into every error body and log.
fn sns_topic_account_refusal(account_id: &str) -> String {
    format!(
        "The Address of an SNS subscriber must be the ARN of an Amazon SNS topic in account \
         {account_id}."
    )
}

/// The ADDRESS rule every subscriber a write ESTABLISHES is held to.
///
/// A `Subscriber` "consists of a subscription type and either an Amazon SNS topic or an
/// email address", and `Subscriber$Address` is "the address that Amazon Web Services
/// sends budget notifications to, either an SNS topic or an email" — so an address has to
/// be the kind of address its own `SubscriptionType` names: an email address under
/// `EMAIL`, the ARN of an SNS topic under `SNS`. The member's documentation states one
/// further rule, which holds whichever type it is: "when you create a subscriber, the
/// value of `Address` can't contain line breaks."
///
/// None of that is expressible as a constraint trait — the modelled `@pattern`
/// `^(.*[\n\r\t\f\ ]?)*$` admits any text, line breaks included — so every operation that
/// establishes a subscriber checks the set it would leave behind here, before anything is
/// written, and refuses one that fails it with the client error each of them models
/// ("An error on the client occurred. Typically, the cause is an invalid input value.").
/// An address that could never receive a budget notification is therefore never stored,
/// so no read reports one.
///
/// `Err` carries the sentence that goes in that refusal's `Message`, stating the rule the
/// address broke as the model words it. The refused ADDRESS is never part of it:
/// `Subscriber$Address` is `@sensitive`, so a refusal that echoed it would put a caller's
/// mailbox or topic into every error body and log.
///
/// Neither form is decided here: an email address is read through the ONE shared reading
/// of a caller-supplied address (`service_common::email_address`) and a topic through the
/// ONE decision that says which topic a notification of this account is published to
/// ([`publishable_topic`], over the ONE shared parse of a caller-supplied ARN
/// [`sns_topic`]), so this service cannot disagree with another about what either is, nor
/// with itself about which addresses an alert reaches.
///
/// `account_id` is the account the request names, which is the account the subscriber is
/// established under: an `SNS` address therefore has to name a topic of THAT account, the
/// only account whose topics a notification of its budgets is published to.
///
/// Takes any iterator of subscribers, so an operation that establishes ONE subscriber and
/// one that establishes a whole set are held to the same rule.
pub fn subscriber_address_violation<'a>(
    account_id: &str,
    subscribers: impl IntoIterator<Item = &'a Subscriber>,
) -> Result<(), String> {
    for subscriber in subscribers {
        if subscriber.address.contains(['\n', '\r']) {
            return Err(ADDRESS_LINE_BREAK_REFUSAL.to_string());
        }
        let violation = match subscriber.subscription_type.as_str() {
            SUBSCRIPTION_TYPE_EMAIL => {
                (!::service_common::email_address::is_email_address(&subscriber.address))
                    .then(|| EMAIL_ADDRESS_REFUSAL.to_string())
            }
            // The SAME decision the alert published to such a subscriber is made by, so an
            // address this write accepts is one an alert of this account's budgets is
            // published to.
            SUBSCRIPTION_TYPE_SNS => publishable_topic(account_id, &subscriber.address).err(),
            // No request reaches this arm today: `SubscriptionType` is a modelled enum of
            // exactly these two, so a value outside them is refused at request validation.
            // It is a refusal rather than an acceptance because a type this service knows
            // no address form for can only mean the catalog grew, and an address nothing
            // checked is what this rule exists to keep out of the store.
            _ => Some(SUBSCRIBER_FORM_REFUSAL.to_string()),
        };
        if let Some(message) = violation {
            return Err(message);
        }
    }
    Ok(())
}

/// The TOPIC a budget notification of `account_id` is published to when `address` names
/// one — `Err` carrying the sentence that refuses the address when it names none.
///
/// The ONE decision for BOTH halves of what an `SNS` subscriber is for: the write that
/// establishes one ([`subscriber_address_violation`]) and the alert published to one
/// ([`alerts`]) each read an address through here, so an address a write accepts is an
/// address an alert can be published to and no stored subscriber is quietly skipped by
/// the delivery path.
///
/// Two things have to hold, and the address is read for both through the ONE parse of an
/// `SNS` subscriber's address ([`sns_topic`]) rather than split again here:
///
/// * it names an SNS topic at all, which — SNS being a regional service — is a topic in a
///   Region, and so an address a publish can be addressed to.
/// * that topic belongs to `account_id`, the account the budget is owned by. An alert
///   carries that budget's own figures (`alerts::message`), and a budget's notifications
///   are delivered to the topics of the account that owns it, so an address naming any
///   other account names no topic a notification of this budget is sent to — however
///   well-formed the ARN is. The same shape as the other caller-supplied ARN this service
///   holds, whose account field is likewise checked before the value is stored
///   ([`validate_execution_role_account`]).
pub(crate) fn publishable_topic<'a>(
    account_id: &str,
    address: &'a str,
) -> Result<::service_common::arn::Arn<'a>, String> {
    let topic = sns_topic(address).ok_or_else(|| SNS_ADDRESS_REFUSAL.to_string())?;
    if topic.account != account_id {
        return Err(sns_topic_account_refusal(account_id));
    }
    Ok(topic)
}

/// The Amazon SNS TOPIC `address` names, as the fields of its ARN — `None` when it names
/// no topic, which is what an `SNS` subscriber's address has to be.
///
/// The ONE reading of an `SNS` subscriber's address, which both halves of what such a
/// subscriber is for reach through [`publishable_topic`]: the write that establishes one
/// and the alert published to one read an address the same way, so this service cannot
/// hold one address to two different grammars.
///
/// Read through the ONE shared parse of a caller-supplied ARN (`service_common::arn`)
/// rather than split here: the value has to be a well-formed resource ARN in SNS's own
/// namespace ([`SNS_NAMESPACE`]). An ARN's resource field is the topic's NAME, so a
/// value whose resource field carries a `:` or a `/` names something beneath a topic (a
/// subscription of one) rather than the topic itself; a topic belongs to an account, so
/// the account field has to name one; Amazon SNS is a REGIONAL service, so a topic lives
/// in a Region and the Region field has to be a Region LABEL
/// (`service_common::arn::region_label`) — a partition-global ARN names no topic, and
/// neither does a value outside that grammar, which is no Region SNS can be addressed in;
/// and no field of an ARN carries whitespace.
pub(crate) fn sns_topic(address: &str) -> Option<::service_common::arn::Arn<'_>> {
    // A line break is whitespace, so this scan covers one too — but an address carrying
    // one never reaches the refusal this feeds: the caller refuses that first, with the
    // sentence the model states for it.
    if address.chars().any(char::is_whitespace) {
        return None;
    }
    let topic = ::service_common::arn::parse_of(address, SNS_NAMESPACE)?;
    let names_topic = ::service_common::arn::region_label(topic.region).is_some()
        && !topic.account.is_empty()
        && !topic.resource.contains([':', '/']);
    names_topic.then_some(topic)
}

/// The notification and subscriber limits over the set of notifications ONE call would
/// attach to a budget that already holds `already_held` of them.
///
/// A notification's capacity belongs to the BUDGET, not to the call that attaches it:
/// the only ceiling is [`MAX_NOTIFICATIONS_PER_BUDGET`] over what the budget ends up
/// holding, so a set attached in one batch and the same set appended one at a time reach
/// the same end state and are accepted alike.
///
/// `Err` carries the sentence that goes in `CreationLimitExceededException$Message`; `Ok`
/// means the call stays within [`MAX_NOTIFICATIONS_PER_BUDGET`] once the set is attached,
/// and within both per-type halves of [`subscriber_limit_violation`] for every
/// notification in it.
///
/// The count branch is defence in depth and NO request reaches it today: its only caller
/// creates a budget (so `already_held` is 0) and a longer list than the capacity is
/// already refused as a request-validation failure by the modelled `@length(0, 10)` of
/// `NotificationWithSubscribersList`, which happens to be the same ten. It stays because
/// the capacity is the rule, not the wire bound: a caller that attaches to a budget that
/// already holds notifications is checked against what the budget would end up with.
pub fn notification_set_limit_violation(
    already_held: usize,
    attaching: &[NotificationWithSubscribers],
) -> Result<(), String> {
    if already_held + attaching.len() > MAX_NOTIFICATIONS_PER_BUDGET {
        return Err(notification_quota_exceeded());
    }
    for supplied in attaching {
        subscriber_limit_violation(&supplied.subscribers)?;
    }
    Ok(())
}

/// The `PlannedBudgetLimits` entry in force at `at`.
///
/// The map's key is the start of the planned month or quarter, "`UTC` in epoch seconds"
/// (`Budget$PlannedBudgetLimits`), so the entry in force is the latest period that has
/// started. "After all of the `BudgetLimit` values in `PlannedBudgetLimits` are used, the
/// budget continues to use the last limit as the `BudgetLimit`" — which is the same entry
/// once no later period has started. A budget whose planned periods all begin in the
/// future ("If the budget begins at a date in the future, provide `PlannedBudgetLimits`
/// values from the start date of the budget") reports its first.
///
/// A key that is not an epoch names no period, so it is skipped; `None` for a map with
/// no usable entry at all.
pub fn planned_limit_in_force(planned: &HashMap<String, Spend>, at: SystemTime) -> Option<Spend> {
    planned_limit_in_force_at_epoch(planned, unix_secs(at))
}

/// The same derivation as [`planned_limit_in_force`], as of an instant named by the epoch
/// it is rather than by a host clock value.
///
/// Both sides of the comparison are epochs — a planned key is "the start of the month,
/// `UTC` in epoch seconds" (`Budget$PlannedBudgetLimits`) and so is every `TimePeriod`
/// bound — so it is made on the epochs themselves, which holds for every instant a stored
/// budget can name and not only those the platform's clock type can hold
/// ([`instant_at`]).
fn planned_limit_in_force_at_epoch(planned: &HashMap<String, Spend>, at: i64) -> Option<Spend> {
    let mut periods: Vec<(i64, &Spend)> = planned
        .iter()
        .filter_map(|(start, limit)| start.trim().parse::<i64>().ok().map(|at| (at, limit)))
        .collect();
    periods.sort_by_key(|(start, _)| *start);
    periods
        .iter()
        .rev()
        .find(|(start, _)| *start <= at)
        .or_else(|| periods.first())
        .map(|(_, limit)| (*limit).clone())
}

/// The `Budget` a read reports for `stored`, as of `at`.
///
/// Every member is the one the write path stored, except the two the SERVICE computes on
/// the read:
///
/// * the `BudgetLimit` of a planned-limit budget: "`DescribeBudget` and `DescribeBudgets`
///   response along with `PlannedBudgetLimits` also contain `BudgetLimit` representing the
///   current month or quarter limit present in `PlannedBudgetLimits`"
///   (`Budget$PlannedBudgetLimits`). Which entry is in force follows the clock, not
///   anything a caller did, so it is derived here and never written back.
/// * `CalculatedSpend`, the cost or usage the budget tracks (see [`calculated_spend`]),
///   which no request can set and every read reports.
pub fn project_budget(stored: &StoredBudget, at: SystemTime) -> Budget {
    budget_as_read(&stored.budget, at)
}

/// The same projection as [`project_budget`], over a `Budget` held on its own — what a
/// write path that has just built the budget it is about to store reads back for it,
/// without first having a [`StoredBudget`] to project.
pub fn budget_as_read(budget: &Budget, at: SystemTime) -> Budget {
    budget_as_read_at_epoch(budget, unix_secs(at))
}

/// The same projection as [`budget_as_read`], as of an instant named by the epoch it is.
///
/// The projection depends on the instant ONLY through which planned period has started by
/// then, which is a comparison between epochs
/// ([`planned_limit_in_force_at_epoch`]) — so the "as of" instant is carried as the epoch
/// it is. That is what lets a read project the budget as of a period of its own history
/// ([`amounts_in_period`]): a `TimePeriod` bound is whatever epoch it was stored with, the
/// model bounds it nowhere, and an epoch the platform's clock type cannot hold
/// ([`instant_at`]) is still an instant this projection answers for.
pub fn budget_as_read_at_epoch(budget: &Budget, at: i64) -> Budget {
    let mut budget = budget.clone();
    if budget.budget_limit.is_none() {
        if let Some(planned) = selection(&budget.planned_budget_limits) {
            budget.budget_limit = planned_limit_in_force_at_epoch(planned, at);
        }
    }
    budget.calculated_spend = calculated_spend(&budget);
    budget
}

/// `budget` as a read that takes `ShowFilterExpression` reports it: with its
/// `FilterExpression` only when the caller asked for it.
///
/// "Specifies whether the response includes the filter expression associated with the
/// budget" (`DescribeBudgetRequest$ShowFilterExpression`, and the same sentence on
/// `DescribeBudgetsRequest$ShowFilterExpression`) — so the member is left out until the
/// flag is sent true, and a request that omits the flag asked for nothing. Every other
/// member is reported either way.
///
/// The ONE home for that flag, applied on top of [`project_budget`] / [`budget_as_read`]
/// by each read that models it, so the single read and the list read cannot come to
/// report different members for the same stored budget.
pub fn with_shown_filter_expression(
    mut budget: Budget,
    show_filter_expression: Option<bool>,
) -> Budget {
    if !show_filter_expression.unwrap_or(false) {
        budget.filter_expression = None;
    }
    budget
}

/// The amount of cost or usage attributed to a budget this emulator holds.
///
/// Nothing in this emulator spends behind a budget — it holds budgets, their
/// notifications, their actions and their tags, and no usage records — so the amount
/// attributed to any one of them is the sum over an empty set of usage. That figure is
/// reported in the budget's own unit (see [`calculated_spend`]), and it satisfies the
/// `NumericValue` `@pattern` every `Spend$Amount` is validated against.
pub const ATTRIBUTED_SPEND_AMOUNT: &str = "0";

/// The `CalculatedSpend` a read reports for `budget` — "the actual and forecasted cost or
/// usage that the budget tracks" (`Budget$CalculatedSpend`).
///
/// The service, not the caller, computes it (`UpdateBudget`: "you can change every part of
/// a budget except for the `budgetName` and the `calculatedSpend`"), and
/// `CalculatedSpend$ActualSpend` is `@required`, so a read reports the spend it has for
/// the budget rather than leaving the member out:
///
/// * `ActualSpend` is "the amount of cost, usage, RI units, or Savings Plans units that
///   you used" — [`ATTRIBUTED_SPEND_AMOUNT`] here — measured in the unit the budget
///   tracks. `Spend$Unit` is "the unit of measurement that's used for the budget forecast,
///   actual spend, or budget threshold", one unit for all three, so it is the unit of the
///   limit THIS read reports: the fixed `BudgetLimit`, the planned entry in force, or the
///   utilization/coverage default (see [`apply_percentage_limit_default`]).
/// * `ForecastedSpend` is "the amount … that you're forecasted to use", "predicted based
///   on your historical usage profile" (`CalculatedSpend`). There is no usage history here
///   to predict from, so no forecast is reported — the member is optional, and a figure
///   nothing forecast would be invented rather than computed.
///
/// `None` for a budget whose read reports no limit at all — the auto-adjusting budget
/// whose amount AWS averages from cost data this emulator does not hold (see
/// [`drop_service_derived`]). Nothing in such a record names a unit of measurement, and
/// `Spend$Unit` is `@required`, so there is no spend to report rather than one measured in
/// a unit this service made up.
fn calculated_spend(budget: &Budget) -> Option<CalculatedSpend> {
    let limit = budget.budget_limit.as_ref()?;
    Some(CalculatedSpend {
        actual_spend: Spend {
            amount: ATTRIBUTED_SPEND_AMOUNT.to_string(),
            unit: limit.unit.clone(),
        },
        forecasted_spend: None,
    })
}

/// Marker the opaque pagination token carries, so a token minted by another
/// service (or invented by a caller) does not decode here.
const NEXT_TOKEN_PREFIX: &str = "budgets:offset:";

/// Encode an offset into the opaque `NextToken` a paginated operation hands back.
pub fn encode_next_token(offset: usize) -> String {
    hex::encode(format!("{NEXT_TOKEN_PREFIX}{offset}"))
}

/// Decode a `NextToken` minted by [`encode_next_token`].
///
/// `None` for anything else — the caller answers that with
/// `InvalidNextTokenException` ("The pagination token is invalid").
///
/// A pagination token is opaque, so the ONLY tokens that decode are the exact byte
/// strings this service hands out: the decoded offset is re-minted and the result must
/// equal the token presented, character for character. That identity check is what
/// rejects every other spelling of the same offset — a payload `str::parse::<usize>()`
/// also accepts (a leading `+`, leading zeros) and hex digits `hex::encode` never writes
/// (upper case) — none of which any call of this service ever handed out.
pub fn decode_next_token(token: &str) -> Option<usize> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let offset: usize = decoded.strip_prefix(NEXT_TOKEN_PREFIX)?.parse().ok()?;
    (encode_next_token(offset) == token).then_some(offset)
}

/// One page of `items`, plus the token for the page after it.
///
/// `max_results` is the EFFECTIVE page size, and the only shape it takes: whatever the
/// request named (or did not), [`page_size`] has already resolved it against the default
/// and the ceiling the calling operation's own `MaxResults` member documents, so the page
/// this serves is one AWS can produce and no caller resolves that size itself.
/// `next_token` is the token a previous page returned. `None` is returned when
/// `next_token` does not decode, or decodes to an offset this list never had a page at —
/// which the caller reports as `InvalidNextTokenException`.
pub fn page<T: Clone>(
    items: &[T],
    max_results: i32,
    next_token: Option<&str>,
) -> Option<(Vec<T>, Option<String>)> {
    let start = match next_token {
        Some(token) => {
            let offset = decode_next_token(token)?;
            // A token is only minted while items remain, so its offset was below the
            // length of the list it was minted for. An offset at or past the end was
            // never minted for THIS list: it is stale or tampered with, not a last page.
            if offset >= items.len() {
                return None;
            }
            offset
        }
        None => 0,
    };
    // At least one item per page: a zero page would hand back a token pointing at the
    // same offset, which is a caller loop that never terminates. [`page_size`] already
    // clamps to at least one, so this only restates that floor for a negative width no
    // resolution can produce.
    let per_page = usize::try_from(max_results).unwrap_or(1).max(1);
    let end = start.saturating_add(per_page).min(items.len());
    let next = (end < items.len()).then(|| encode_next_token(end));
    Some((items[start..end].to_vec(), next))
}

/// The modelled ceiling of the `MaxResults` member the per-budget and per-account
/// listings of this service share: `@range(min: 1, max: 100)` — "An integer that
/// represents how many entries a paginated response contains. The maximum is 100." It is
/// also the `@length(max)` of each list those operations page, so no page of theirs can
/// exceed it.
pub const MAX_RESULTS_CEILING: i32 = 100;

/// The EFFECTIVE page size one paginated read serves, handed to [`page`] — the ONE home
/// for that resolution, so every paginated read of this service resolves it the same way.
///
/// `max_results` is the page size the caller named, if any. `documented_default` is the
/// page size that operation's own `MaxResults` member documents a default for, and `None`
/// where the model documents none — in which case a request naming no `MaxResults` is
/// served a full `modelled_max`-sized page, the largest page that operation can report.
/// `modelled_max` is the member's `@range(max)`, which is also the `@length(max)` of the
/// list being paged, so the page never exceeds what the output shape holds; a named size
/// above it is clamped down to it, and the result is never below one entry (a zero page
/// would mint a token pointing back at its own offset). Both clamps are no-ops for the
/// sizes the boundary validator already admits, so they only keep a page inside the
/// output shape when nothing else would.
pub fn page_size(
    max_results: Option<i32>,
    documented_default: Option<i32>,
    modelled_max: i32,
) -> i32 {
    let ceiling = modelled_max.max(1);
    max_results
        .or(documented_default)
        .unwrap_or(ceiling)
        .clamp(1, ceiling)
}

/// Whether the clock has already made `stored`'s deletion due.
///
/// A budget's `TimePeriod$End` is the end of its life: "After the end date, Amazon Web
/// Services deletes the budget and all the associated notifications and subscribers"
/// (`Budget$TimePeriod`). The deletion falls due at that instant, so a budget whose end
/// date is in the PAST is no longer part of the account's state at all.
///
/// A budget that names no `End` names no such instant, so nothing is due for it. Every
/// write path stamps the documented default end ([`stamp_service_owned`]), so only a
/// budget placed in the store directly can be in that shape.
fn budget_deletion_due(stored: &StoredBudget, now: SystemTime) -> bool {
    let now = ::aws_smithy_types::DateTime::from(now);
    stored
        .budget
        .time_period
        .as_ref()
        .and_then(|period| period.end.as_ref())
        .is_some_and(|end| instant(end) < instant(&now))
}

/// Drop every budget whose deletion the clock has already made due
/// ([`budget_deletion_due`]), with the notifications, subscribers, actions and tags held
/// inside it — the deletion AWS performs after a budget's end date, materialised at the
/// moment a call observes the store.
///
/// This is the clock's own deletion, not a change any caller made: the same budgets would
/// be gone at this instant even if nobody had called, and the budget a read drops here
/// never comes back, so no later caller sees a budget this one did not.
///
/// Every operation that reaches this store runs it first ([`store_at`]), so a budget past
/// its end date is absent from every listing, is answered exactly as a budget that was
/// never created is answered — the `NotFoundException` on each call that resolves the
/// budget, the empty subscriber set on the read whose subject is a subscriber set — and
/// stops counting against the account's budget quota ([`budget_count_for_account`]).
fn expire_due_budgets(
    state: &mut ComAmazonawsBudgetsAwsbudgetservicegatewayState,
    now: SystemTime,
) {
    state
        .budgets
        .retain(|_, stored| !budget_deletion_due(stored, now));
}

/// Lock this service's store for a call that observes it at `now`, with every deletion the
/// clock owes already materialised ([`expire_due_budgets`]).
///
/// The one door into the store: a handler calls this instead of `state.lock()`, so no
/// operation can read or write a budget whose end date has passed.
pub fn store_at(
    state: &::engine::ServiceState<ComAmazonawsBudgetsAwsbudgetservicegatewayState>,
    now: SystemTime,
) -> ::std::sync::MutexGuard<'_, ComAmazonawsBudgetsAwsbudgetservicegatewayState> {
    let mut store = state.lock();
    expire_due_budgets(&mut store, now);
    store
}

/// The refusal message a request that names an account other than the caller's own
/// takes, or `None` when it names the caller's account.
///
/// The budgets control plane is scoped to the account the request is signed by: every
/// operation names the account "that is associated with the budget" — as an `AccountId`
/// member, or, on the ARN-addressed tagging operations, as the account field of the
/// `ResourceARN` ([`TaggedResource::account_id`]) — and declares
/// `AccessDeniedException` ("You are not authorized to use this operation with the
/// given parameters.", HTTP 403), and a caller reaches no account but its own through
/// it. This emulator answers for exactly one account
/// ([`::engine::emulator_account_id`]), so a request naming any other account is
/// refused with that error before it reads or writes anything — rather than being
/// served a budget namespace of its own.
///
/// Shared by every operation handler, so the one gate cannot drift between them.
pub fn foreign_account_refusal(account_id: &str) -> Option<String> {
    (account_id != ::engine::emulator_account_id())
        .then(|| ACCESS_DENIED_MESSAGE.to_string())
}

/// What [`foreign_account_refusal`] says: `AccessDeniedException`'s own documented
/// sentence, which states the one fact a caller needs (this operation is not available
/// with the parameters sent) and names neither the account asked for nor the one the
/// emulator answers for.
pub const ACCESS_DENIED_MESSAGE: &str =
    "You are not authorized to use this operation with the given parameters.";

/// Every budget stored for `account_id`, as the LISTING reports it at `at`, ordered by
/// `BudgetName`.
///
/// The order is the one the pagination token indexes into, so it must not depend
/// on `HashMap` iteration order. Only the `Budget` is projected — the notifications,
/// subscribers and tags hanging off a stored budget are not reported by a list call.
pub fn budgets_for_account(
    state: &ComAmazonawsBudgetsAwsbudgetservicegatewayState,
    account_id: &str,
    at: SystemTime,
) -> Vec<Budget> {
    let mut found: Vec<&StoredBudget> = state
        .budgets
        .iter()
        .filter(|((account, _), _)| account == account_id)
        .map(|(_, stored)| stored)
        .collect();
    found.sort_by(|a, b| a.budget.budget_name.cmp(&b.budget.budget_name));
    found
        .into_iter()
        .map(|stored| budget_as_listed(project_budget(stored, at)))
        .collect()
}

/// `budget` as the account-wide LISTING reports it: without the `CalculatedSpend` the
/// single-budget read reports for it.
///
/// Recorded behaviour, not modelled: the model documents `Budget$CalculatedSpend` on the
/// shape both reads report and says nothing about either read leaving it out, while the
/// recorded `DescribeBudgets` listing of a budget carries no `CalculatedSpend` and the
/// recorded `DescribeBudget` of that same budget carries it. The two reads differ in
/// exactly that one member: `BudgetLimit`, the other member a read derives rather than
/// stores ([`budget_as_read_at_epoch`]), is documented for BOTH — "`DescribeBudget` and
/// `DescribeBudgets` response along with `PlannedBudgetLimits` also contain
/// `BudgetLimit`" (`Budget$PlannedBudgetLimits`) — so the listing reports it, and a
/// client reads the spend a budget tracks by asking about that budget.
pub fn budget_as_listed(mut budget: Budget) -> Budget {
    budget.calculated_spend = None;
    budget
}

/// How many budgets `account_id` already holds — what the per-account quota below is
/// checked against.
pub fn budget_count_for_account(
    state: &ComAmazonawsBudgetsAwsbudgetservicegatewayState,
    account_id: &str,
) -> usize {
    state
        .budgets
        .keys()
        .filter(|(account, _)| account == account_id)
        .count()
}

/// AWS Budgets' documented default service quota: "Budgets: 20,000 per account"
/// (Billing and Cost Management service quotas). `CreateBudget` answers
/// `ServiceQuotaExceededException` ("You've reached a Service Quota limit on this
/// resource", HTTP 402) once an account holds this many.
pub const MAX_BUDGETS_PER_ACCOUNT: usize = 20_000;

/// The whole store lives in memory and a budget's name comes from the request, so this
/// is the emulator's own ceiling on the total number of budgets it will hold — not an
/// AWS quota — answered with the same modelled `ServiceQuotaExceededException`. Every
/// request is scoped to the one account this emulator answers for
/// ([`foreign_account_refusal`]), so the per-account quota above is what a caller
/// reaches first; this bound stands over whatever a test or a future operation seeds
/// into the store directly.
pub const MAX_BUDGETS_TOTAL: usize = 100_000;

/// `BudgetName`'s modelled `@pattern`, verbatim from the model
/// (`smithy_query(query="shape",shape="BudgetName")`). The rule
/// [`budget_name_matches_pattern`] enforces, in one place, so the code and the
/// comment above it cannot drift from the model.
pub const BUDGET_NAME_PATTERN: &str =
    r"^(?![^:\\]*/action/|(?i).*<script>.*</script>.*)[^:\\]+$";

/// Whether `name` satisfies [`BUDGET_NAME_PATTERN`] — documented on `BudgetName` as
/// "The ':' and '\' characters, and the '/action/' substring, aren't allowed", with
/// names "validated for content".
///
/// Checked here rather than left to the boundary validator because that pattern uses
/// a negative look-ahead, which the framework's regex engine cannot compile, and an
/// uncompilable `@pattern` is treated as matching ("we never reject a value because of
/// an unparseable model pattern"). The `@length(1, 100)` half of the constraint IS
/// enforced upstream, so it is not repeated here.
fn budget_name_matches_pattern(name: &str) -> bool {
    // `[^:\\]+`: at least one character, and no ':' or '\' anywhere. Any name holding
    // one of those two also fails the look-ahead's `[^:\\]*/action/` alternative, so
    // rejecting them here covers "/action/" with a prefix of other characters too.
    if name.is_empty() || name.contains(':') || name.contains('\\') {
        return false;
    }
    if name.contains("/action/") {
        return false;
    }
    // `(?i).*<script>.*</script>.*` — the case-insensitive half of the look-ahead.
    let lowered = name.to_lowercase();
    !lowered
        .split_once("<script>")
        .is_some_and(|(_, rest)| rest.contains("</script>"))
}

/// The refusal a `BudgetName` outside its modelled `@pattern` takes, or `None` when
/// the name satisfies it.
///
/// A value outside a MODELLED constraint is refused at request validation, before the
/// operation runs — the same class of refusal as a bad `AccountId` or an out-of-range
/// `MaxResults`, which the boundary validator answers with HTTP 400
/// `ValidationException` and a message naming the failing member path and the
/// constraint it violated. This pattern reaches a handler only because the framework's
/// regex engine cannot compile a negative look-ahead, so the refusal states the same
/// two facts, in the clause shape the framework's own renderer uses
/// (`N validation error(s) detected: Value … at '<path>' failed to satisfy constraint:
/// <requirement>` — [`::aws_smithy_schema_server::Violation::message`]).
///
/// It states them WITHOUT the rejected name and without the pattern, which is where it
/// departs from that renderer. `BudgetName` is a content rule, not a syntax rule: the
/// model documents that "Budget names are validated for content. Names that contain
/// phone numbers, URLs, or email addresses combined with certain terms may be
/// rejected", and the pattern itself refuses `<script>…</script>`. Echoing a name back
/// because it was refused for its content reflects exactly the content the rule exists
/// to keep out of this service, and restating the filter tells a caller how to get
/// around it. The member path and the constraint are what a caller needs to fix the
/// request, and they are what AWS's recorded refusal states.
///
/// `member_path` is the trail of Smithy member names from the operation input inwards
/// (`["Budget", "BudgetName"]` for `CreateBudget`, `["BudgetName"]` for the
/// operations that take the name directly); each segment is lowerCamel-ed by the
/// framework's own path builder.
pub fn budget_name_violation(member_path: &[&str], name: &str) -> Option<String> {
    if budget_name_matches_pattern(name) {
        return None;
    }
    let path: Vec<String> = member_path
        .iter()
        .map(|segment| ::aws_smithy_schema_server::constraints::path::member(segment))
        .collect();
    // Every segment is a Smithy member name chosen by a call site in this crate, so
    // nothing a caller sent reaches the sentence below.
    let path = ::aws_smithy_schema_server::constraints::path::join(&path);
    Some(format!(
        "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern"
    ))
}

/// The cross-member rules `CreateBudget` and `UpdateBudget` document for the
/// `Budget` they accept, none of which a Smithy constraint trait can express.
///
/// A rule the model DOES express is not checked here: a value outside a modelled
/// constraint is a request-validation refusal, answered by
/// [`budget_name_violation`]'s renderers, not by this service's own
/// `InvalidParameterException`.
///
/// `Err` carries the sentence that goes in `InvalidParameterException$Message`
/// ("An error on the client occurred. Typically, the cause is an invalid input
/// value."). Every rule here is quoted from the model's own documentation:
///
/// * "You can't use `BudgetLimit` with `PlannedBudgetLimits` for `CreateBudget`
///   and `UpdateBudget` actions." (`Budget$BudgetLimit`) — one of the three pairings of
///   the members that determine a budget's amount, which are mutually exclusive as a
///   set: the fixed `BudgetLimit`, the planned map and the `AutoAdjustData` whose amount
///   the service adjusts instead ([`BUDGET_AMOUNT_EXCLUSIVITY_REFUSAL`])
/// * "only one set of filter and metric selections can be present in the syntax at
///   one time. Either `FilterExpression` and `Metrics` or `CostFilters` and
///   `CostTypes`, not both or a different combination." (`CreateBudget`)
/// * "The start date must come before the end date." (`Budget$TimePeriod`) and "The end
///   date must come before `06/15/87 00:00 UTC`." (same member) — both judged over the
///   period the write LEAVES stored ([`stamped_time_period`]), so a bound the SERVICE
///   supplies is held to the same rule as one the caller spelled out
///   ([`validate_time_period`])
/// * "`BudgetLimit` is required for cost or usage budgets." (`Budget$BudgetLimit`) —
///   [`validate_budget_limit_present`]
/// * "RI and Savings Plans utilization or coverage budgets default to `100`. This is the
///   only valid value for RI or Savings Plans utilization or coverage budgets."
///   (`Budget$BudgetLimit`) — [`validate_percentage_limit_amount`]
/// * "`PlannedBudgetLimits` is available for cost or usage budget and supports both
///   monthly and quarterly `TimeUnit` ... The `key` is the start of the month, `UTC` in
///   epoch seconds" (`Budget$PlannedBudgetLimits`) — [`validate_planned_budget_limits`]
/// * the `AutoAdjustType`/`HistoricalOptions` pairing and the per-`TimeUnit` maximum
///   `BudgetAdjustmentPeriod` (`AutoAdjustData$HistoricalOptions`,
///   `HistoricalOptions$BudgetAdjustmentPeriod`) — [`validate_auto_adjust_data`]
///
/// Every rule here is stated over the selections the caller actually MADE: a collection
/// member sent empty (`PlannedBudgetLimits`, `CostFilters` or the `@length(min: 0)`
/// `Metrics`) carries no entry, so it names no limit and no filter syntax and is judged
/// the same way as a member that was not sent at all ([`selection`]).
///
/// Every rule holds for the whole `Budget` shape AS IT WILL BE STORED, so both write paths
/// (`CreateBudget$Budget` and `UpdateBudget$NewBudget`) run all of them before anything
/// is stored: an update that would leave a budget in a shape a create could not have
/// produced is refused, and the stored budget is left as it was. `at` is the instant of
/// that write — the one the service's own defaults are computed from — so a rule over a
/// member the service supplies judges the value the write will actually leave behind.
pub fn validate_budget(budget: &Budget, at: SystemTime) -> Result<(), String> {
    // The three members that can determine a budget's amount are mutually exclusive, so
    // a budget that supplies more than one of them names no single amount. Counted over
    // all three rather than checked pair by pair: the rule is "only one of them", and
    // the refusal names the whole set either way.
    let amount_sources = [
        budget.budget_limit.is_some(),
        selects(&budget.planned_budget_limits),
        budget.auto_adjust_data.is_some(),
    ]
    .into_iter()
    .filter(|supplied| *supplied)
    .count();
    if amount_sources > 1 {
        return Err(BUDGET_AMOUNT_EXCLUSIVITY_REFUSAL.to_string());
    }
    let legacy_filters = selects(&budget.cost_filters) || budget.cost_types.is_some();
    let expression_filters = budget.filter_expression.is_some() || selects(&budget.metrics);
    if legacy_filters && expression_filters {
        return Err(
            "Unable to create/update budget: FilterExpression and Metrics can't be used with \
             CostFilters and CostTypes."
                .to_string(),
        );
    }
    validate_time_period(&stamped_time_period(budget, at))?;
    validate_budget_limit_present(budget)?;
    validate_percentage_limit_amount(budget)?;
    validate_planned_budget_limits(budget)?;
    validate_auto_adjust_data(budget)?;
    Ok(())
}

/// A `DateTime` as the totally ordered instant it names, so two timestamps are compared
/// without any calendar arithmetic written here.
fn instant(at: &::aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// Whether `period` satisfies the rule every `TimePeriod` carries: "The start date must
/// come before the end date" (`TimePeriod`). A period that names only one of the two
/// bounds, or neither, satisfies it — "There are no restrictions on the end date".
///
/// The comparison is STRICT, so a period whose `Start` equals its `End` does not satisfy
/// the rule — on a write and on a read filter alike, since the rule is the shape's, not
/// one operation's. That is deliberate, and it is not in tension with
/// [`within_time_period`] treating the bounds as covered: a window is refused before it
/// ever filters anything, so the two never judge the same period.
pub fn time_period_ordered(period: &TimePeriod) -> bool {
    match (&period.start, &period.end) {
        (Some(start), Some(end)) => instant(start) < instant(end),
        _ => true,
    }
}

/// Whether `at` falls inside `period`, which is the window a read filters its records
/// by: "The period of time that's covered ... has a start date and an end date"
/// (`TimePeriod`). A bound the caller left out does not restrict that side, and the
/// bounds themselves are covered by the period they delimit.
///
/// This is the rule for ONE INSTANT — a record stamped with a timestamp. A read that
/// filters whole periods rather than instants uses [`period_overlaps`], which treats a
/// period as half-open so consecutive periods do not each claim the boundary between
/// them.
pub fn within_time_period(at: &::aws_smithy_types::DateTime, period: &TimePeriod) -> bool {
    let at = instant(at);
    period
        .start
        .as_ref()
        .is_none_or(|start| instant(start) <= at)
        && period.end.as_ref().is_none_or(|end| at <= instant(end))
}

/// Whether `period` and `window` have any instant in common — the rule a read follows
/// to decide whether a budget period belongs in the window it was asked about.
///
/// Both are treated as half-open, running from `Start` up to but not including `End`, so
/// two periods that merely touch (one ending exactly where the next begins) do not
/// overlap: that is what makes a series of consecutive budget periods a partition of the
/// time it covers rather than a set of periods that each share a boundary instant with
/// the next. A bound either side leaves out does not restrict that side (`TimePeriod`:
/// "There are no restrictions on the end date").
pub fn period_overlaps(period: &TimePeriod, window: &TimePeriod) -> bool {
    let strictly_before = |left: &Option<::aws_smithy_types::DateTime>,
                           right: &Option<::aws_smithy_types::DateTime>| {
        match (left, right) {
            (Some(left), Some(right)) => instant(left) < instant(right),
            _ => true,
        }
    };
    strictly_before(&period.start, &window.end) && strictly_before(&window.start, &period.end)
}

/// `TimeUnit$ANNUALLY` — the one time unit that has no performance history: "Describes
/// the history for DAILY, MONTHLY, and QUARTERLY budgets. Budget history isn't available
/// for ANNUAL budgets" (`DescribeBudgetPerformanceHistory`).
pub const TIME_UNIT_ANNUALLY: &str = "ANNUALLY";

/// `TimeUnit$CUSTOM` — the time unit that names no repeating period: such a budget covers
/// the one `TimePeriod` its caller chose, so that period is its whole history.
pub const TIME_UNIT_CUSTOM: &str = "CUSTOM";

/// How many periods of each `TimeUnit` a budget's performance history retains, from the
/// documentation of the history itself: "For DAILY budgets, the history saves the state
/// of the budget for the last 60 days. For MONTHLY budgets, the history saves the state
/// of the budget for the current month plus the last 12 months. For QUARTERLY budgets,
/// the history saves the state of the budget for the last four quarters"
/// (`DescribeBudgetPerformanceHistoryResponse$BudgetPerformanceHistory`).
///
/// Each count includes the period IN PROGRESS, which is the period those sentences count
/// from — "the current month plus the last 12 months" is thirteen entries.
const RETAINED_PERIODS: [(&str, i64); 3] = [("DAILY", 60), ("MONTHLY", 13), ("QUARTERLY", 4)];

/// Seconds in a UTC day — the length of every `DAILY` period on the calendar the
/// runtime's own `DateTime` implements, so stepping whole days from a midnight boundary
/// lands on a midnight boundary with no date arithmetic written here.
const SECONDS_PER_DAY: i64 = 24 * 60 * 60;

/// Months in a year, and in a calendar quarter: the two step lengths the `MONTHLY` and
/// `QUARTERLY` boundaries are counted in.
const MONTHS_PER_YEAR: i64 = 12;
const MONTHS_PER_QUARTER: i64 = 3;

/// The midnight-UTC first of the month `months` whole months from `from`, which is itself
/// the first of a month.
///
/// A month is not a fixed number of seconds, so the shift is counted on the civil
/// calendar — year and month read off the runtime's own rendering of `from` — and the
/// shifted boundary is turned back into an instant by the runtime's own parser.
///
/// `None` when that boundary is no date the calendar renders.
fn month_start_shifted(
    from: ::aws_smithy_types::DateTime,
    months: i64,
) -> Option<::aws_smithy_types::DateTime> {
    let (year, month, _) = civil_date(instant_at(from.secs())?)?;
    let total = year
        .checked_mul(MONTHS_PER_YEAR)?
        .checked_add(i64::from(month).checked_sub(1)?)?
        .checked_add(months)?;
    let year = total.div_euclid(MONTHS_PER_YEAR);
    let month = total.rem_euclid(MONTHS_PER_YEAR) + 1;
    if year < 0 {
        return None;
    }
    midnight_at(&format!("{year:04}-{month:02}-01T00:00:00Z"))
}

/// The start of the period of `time_unit` that lies `shift` periods from the period
/// containing `now`: `0` is that period, `-1` the period before it, `1` the one after.
///
/// `None` for a `time_unit` that names no repeating period, and for a boundary the
/// calendar does not render.
fn period_start_shifted(
    time_unit: &str,
    now: SystemTime,
    shift: i64,
) -> Option<::aws_smithy_types::DateTime> {
    let current = default_time_period_start(time_unit, now);
    match time_unit {
        "DAILY" => {
            let secs = current
                .secs()
                .checked_add(shift.checked_mul(SECONDS_PER_DAY)?)?;
            Some(::aws_smithy_types::DateTime::from_secs(secs))
        }
        "MONTHLY" => month_start_shifted(current, shift),
        "QUARTERLY" => month_start_shifted(current, shift.checked_mul(MONTHS_PER_QUARTER)?),
        _ => None,
    }
}

/// The periods of `time_unit` a budget's performance history retains as of `now`, OLDEST
/// FIRST and ending with the period in progress — the window [`RETAINED_PERIODS`] states
/// for that unit. Each period runs from its own midnight-UTC boundary up to the boundary
/// of the period after it, so consecutive entries abut without overlapping.
///
/// EMPTY for a time unit with no repeating period ([`TIME_UNIT_ANNUALLY`],
/// [`TIME_UNIT_CUSTOM`]) or one this emulator does not know: there is no series to count
/// back over.
pub fn retained_periods(time_unit: &str, now: SystemTime) -> Vec<TimePeriod> {
    let Some((_, count)) = RETAINED_PERIODS.iter().find(|(unit, _)| *unit == time_unit) else {
        return Vec::new();
    };
    (0..*count)
        .rev()
        .filter_map(|back| {
            Some(TimePeriod {
                start: Some(period_start_shifted(time_unit, now, -back)?),
                end: Some(period_start_shifted(time_unit, now, 1 - back)?),
            })
        })
        .collect()
}

/// The periods `stored` HAS BEEN MEASURED OVER as of `now`, OLDEST FIRST and ending with
/// the period in progress.
///
/// A budget is measured per period of its own `TimeUnit` ([`retained_periods`]), bounded
/// by two things:
///
/// * the budget's own `TimePeriod` — "the period of time that's covered by a budget" — so
///   a period the budget does not cover was never measured for it;
/// * the clock: a period still ahead of `now` has nothing to report yet.
///
/// A time unit that names no repeating period ([`TIME_UNIT_ANNUALLY`],
/// [`TIME_UNIT_CUSTOM`]) has ONE such period, the budget's own, which is the whole of
/// that budget's history.
///
/// The one answer to "which periods has this budget run for?", so the history a read
/// reports ([`amounts_in_period`]) and the breach counter a notification of it reports
/// ([`consecutive_breach_count`]) cannot come to count different periods.
pub fn covered_periods(budget: &Budget, now: SystemTime) -> Vec<TimePeriod> {
    let mut candidates = retained_periods(&budget.time_unit, now);
    if candidates.is_empty() {
        candidates = budget.time_period.clone().into_iter().collect();
    }
    let covered = budget.time_period.clone().unwrap_or_default();
    let begun = TimePeriod {
        start: None,
        end: Some(::aws_smithy_types::DateTime::from(now)),
    };
    candidates
        .into_iter()
        .filter(|period| period_overlaps(period, &begun))
        .filter(|period| period_overlaps(period, &covered))
        .collect()
}

/// The budgeted and actual amounts `stored` reports for one `period` it covers — "the
/// amount of cost or usage that you created the budget for, compared to your actual costs
/// or usage" (`BudgetedAndActualAmounts`).
///
/// Both figures are the ones a read of the budget reports AS OF that period
/// ([`budget_as_read_at_epoch`]), so a performance history cannot disagree with
/// `DescribeBudget`:
///
/// * `BudgetedAmount` is "the amount of cost or usage that you created the budget for" —
///   the limit in force while the period ran, which for a `PlannedBudgetLimits` budget is
///   the planned entry that period started under ([`planned_limit_in_force`]) and for
///   every other budget is its one `BudgetLimit`. It is the amount the caller WROTE, in
///   the spelling they wrote it in: `Spend$Amount` is a decimal STRING
///   (`NumericValue`, `@pattern ^([0-9]*\.)?[0-9]+$`), so its textual form is part of the
///   value and no read renders it through a numeric type.
/// * `ActualAmount` is "your actual costs or usage for a budget period": the cost or usage
///   this emulator has attributed to the budget ([`ATTRIBUTED_SPEND_AMOUNT`]), measured in
///   the unit that limit names.
///
/// Neither is reported for a budget whose read reports no limit at all (the auto-adjusting
/// budget of [`calculated_spend`]): nothing in such a record names a unit of measurement,
/// and `Spend$Unit` is `@required`.
pub fn amounts_in_period(stored: &StoredBudget, period: &TimePeriod) -> BudgetedAndActualAmounts {
    // The period is projected as of the epoch its `Start` names, whatever epoch that is:
    // the amounts a record reports follow from the budget, not from whether the platform's
    // clock type can hold that boundary, so every period the budget covers reports the
    // figures its read reports.
    //
    // A period that names no start is unbounded below, so it is projected as of the
    // earliest epoch there is — which reports the earliest planned entry, the entry a
    // budget whose planned periods have all still to start reports.
    let at = period.start.as_ref().map_or(i64::MIN, |start| start.secs());
    let projected = budget_as_read_at_epoch(&stored.budget, at);
    BudgetedAndActualAmounts {
        budgeted_amount: projected.budget_limit.clone(),
        actual_amount: projected.calculated_spend.map(|spend| spend.actual_spend),
        time_period: Some(period.clone()),
    }
}

/// `EventType$CREATE_ACTION` — the event a `CreateBudgetAction` call records.
pub const EVENT_TYPE_CREATE_ACTION: &str = "CREATE_ACTION";

/// `EventType$UPDATE_ACTION` — the event an `UpdateBudgetAction` call records.
pub const EVENT_TYPE_UPDATE_ACTION: &str = "UPDATE_ACTION";

/// `EventType$DELETE_ACTION` — the event a `DeleteBudgetAction` call records. The action
/// and the history held beside it go with that call, so nothing in this service reads such
/// a record back; the event type is one of the modelled catalog's and is worded here with
/// the rest.
pub const EVENT_TYPE_DELETE_ACTION: &str = "DELETE_ACTION";

/// `EventType$EXECUTE_ACTION` — the event an `ExecuteBudgetAction` call records.
pub const EVENT_TYPE_EXECUTE_ACTION: &str = "EXECUTE_ACTION";

/// `EventType$SYSTEM` — the event a status change this service makes on its own records:
/// the one event type no caller triggers ([`StoredAction::settle_due`]).
pub const EVENT_TYPE_SYSTEM: &str = "SYSTEM";

/// What a history record SAYS happened, which is fixed by the event's `EventType` alone.
///
/// `ActionHistoryDetails$Message` is "the description of the details for the event" and
/// `ActionHistory$EventType` "distinguishes between whether the events are triggered by
/// the user or are generated by the system" — so the sentence names the ACTOR and the
/// thing that was done to the action, and carries no variable part: every other member of
/// the record (the status, the timestamp, the action itself) already states what this
/// particular event did.
///
/// An event a CALLER triggered is worded from that caller's point of view; the one event
/// type no caller triggers names the system that made the move instead.
pub fn action_event_message(event_type: &str) -> &'static str {
    match event_type {
        EVENT_TYPE_UPDATE_ACTION => "Customer updated a budget action.",
        EVENT_TYPE_DELETE_ACTION => "Customer deleted a budget action.",
        EVENT_TYPE_EXECUTE_ACTION => "Customer executed a budget action.",
        EVENT_TYPE_SYSTEM => "System updated a budget action.",
        // `EVENT_TYPE_CREATE_ACTION`, and any event type the modelled catalog grows that
        // this service does not record: an action's history begins with its creation.
        _ => "Customer created a budget action.",
    }
}

/// How many history records one action keeps. NOT an AWS rule: every write against an
/// action appends a record, so without a ceiling a caller could grow this process
/// without limit. The oldest records are dropped first, so a read always reports the
/// most recent history.
pub const MAX_ACTION_HISTORY_RECORDS: usize = 1_000;

/// The two rules a budget's `TimePeriod` carries, over the period a write LEAVES on the
/// budget ([`stamped_time_period`]) rather than over the bounds a request spelled out:
///
/// * "The start date must come before the end date" ([`time_period_ordered`]), so the
///   period a write stores is never the inverted one the model says cannot exist —
///   whichever of its two bounds the service itself supplied;
/// * "The end date must come before `06/15/87 00:00 UTC`" (`Budget$TimePeriod`), which is
///   also the end date a budget written without one is given
///   ([`default_time_period_end`]) — so that instant itself is accepted and anything after
///   it is not.
fn validate_time_period(period: &TimePeriod) -> Result<(), String> {
    if !time_period_ordered(period) {
        return Err(BUDGET_TIME_PERIOD_ORDER_REFUSAL.to_string());
    }
    let Some(end) = period.end.as_ref() else {
        return Ok(());
    };
    let last = default_time_period_end();
    if instant(end) > instant(&last) {
        return Err(
            "Unable to create/update budget: TimePeriod End must not come after \
             06/15/87 00:00 UTC."
                .to_string(),
        );
    }
    Ok(())
}

/// "`BudgetLimit` is required for cost or usage budgets, but optional for RI or Savings
/// Plans utilization or coverage budgets" (`Budget$BudgetLimit`).
///
/// Three members can carry such a budget's limit, and exactly one of them has to: the
/// fixed `BudgetLimit`; a `PlannedBudgetLimits` map that plans at least one period
/// ([`selection`]), which a read derives the current `BudgetLimit` from ("`DescribeBudget`
/// and `DescribeBudgets` response along with `PlannedBudgetLimits` also contain
/// `BudgetLimit`"); and `AutoAdjustData`, "the
/// parameters that determine the budget amount for an auto-adjusting budget"
/// (`Budget$AutoAdjustData`), whose amount the service adjusts rather than the caller
/// setting it.
fn validate_budget_limit_present(budget: &Budget) -> Result<(), String> {
    if !LIMIT_REQUIRED_BUDGET_TYPES.contains(&budget.budget_type.as_str()) {
        return Ok(());
    }
    if budget.budget_limit.is_some()
        || selects(&budget.planned_budget_limits)
        || budget.auto_adjust_data.is_some()
    {
        return Ok(());
    }
    Err(
        "Unable to create/update budget: BudgetLimit is required for cost or usage budgets."
            .to_string(),
    )
}

/// "RI and Savings Plans utilization or coverage budgets default to `100`. This is the
/// only valid value for RI or Savings Plans utilization or coverage budgets"
/// (`Budget$BudgetLimit`) — so a budget whose type tracks a utilization or coverage
/// percentage ([`PERCENTAGE_LIMIT_BUDGET_TYPES`]) is accepted with no `BudgetLimit` at
/// all (it is then stored with the documented default, see
/// [`apply_percentage_limit_default`]) or with one naming that single value, and refused
/// any other amount — on create and on update alike, so no write path can leave a stored
/// budget in a shape the model says cannot exist.
///
/// The amount is compared as the NUMBER it names rather than as a spelling: `NumericValue`
/// is a string (`^([0-9]*\.)?[0-9]+$`), so every spelling of that one value (`100`,
/// `100.0`, `0100`) names it, and the comparison goes through the same
/// [`numeric_amount`] parser every other amount comparison in this crate uses — a figure
/// that is no finite number names no valid value either.
///
/// The `Spend$Unit` is NOT constrained here: the documentation says nothing about the unit
/// such a limit is measured in and `UnitValue`'s `.*` pattern accepts anything, so a unit
/// the caller sent is stored as sent (see [`PERCENTAGE_LIMIT_UNIT`]).
fn validate_percentage_limit_amount(budget: &Budget) -> Result<(), String> {
    if !PERCENTAGE_LIMIT_BUDGET_TYPES.contains(&budget.budget_type.as_str()) {
        return Ok(());
    }
    let Some(limit) = &budget.budget_limit else {
        return Ok(());
    };
    let only_valid = numeric_amount(PERCENTAGE_LIMIT_AMOUNT);
    if numeric_amount(&limit.amount) == only_valid {
        return Ok(());
    }
    // The amount is not echoed back: the member, the value it must name and the budget
    // types the rule holds for are what a caller needs to correct the request.
    Err(format!(
        "Unable to create/update budget: BudgetLimit amount must be \
         {PERCENTAGE_LIMIT_AMOUNT} for RI or Savings Plans utilization or coverage budgets."
    ))
}

/// "`PlannedBudgetLimits` is available for cost or usage budget and supports both monthly
/// and quarterly `TimeUnit`" and "The `key` is the start of the month, `UTC` in epoch
/// seconds" / "the start of the quarter, `UTC` in epoch seconds"
/// (`Budget$PlannedBudgetLimits`) — so a planned map is refused on any other budget type
/// or granularity, and every key has to be the midnight-UTC start of one of the periods
/// the budget's own `TimeUnit` names.
fn validate_planned_budget_limits(budget: &Budget) -> Result<(), String> {
    // A map sent empty plans no period at all ([`selection`]), so the budget it is sent
    // with is written with no planned limits and this rule has nothing to hold over.
    let Some(planned) = selection(&budget.planned_budget_limits) else {
        return Ok(());
    };
    if !LIMIT_REQUIRED_BUDGET_TYPES.contains(&budget.budget_type.as_str())
        || !PLANNED_LIMIT_TIME_UNITS.contains(&budget.time_unit.as_str())
    {
        return Err(
            "Unable to create/update budget: PlannedBudgetLimits is available for cost or \
             usage budgets with a MONTHLY or QUARTERLY TimeUnit."
                .to_string(),
        );
    }
    // The key is reported back as stored, so it is never echoed into this message: the
    // member and the rule it failed are what a caller needs. `TimeUnit` is a modelled
    // enum, already checked at request validation.
    if !planned
        .keys()
        .all(|start| start.parse::<i64>().is_ok_and(|secs| is_period_start(&budget.time_unit, secs)))
    {
        return Err(format!(
            "Unable to create/update budget: every PlannedBudgetLimits key must be the start \
             of a {} period, UTC in epoch seconds.",
            budget.time_unit
        ));
    }
    Ok(())
}

/// The rules the model documents for an auto-adjusting budget:
///
/// * `HistoricalOptions` are "the parameters that define or describe the historical data
///   that your auto-adjusting budget is based on"
///   (`AutoAdjustData$HistoricalOptions`), so they belong to — and are needed by — the
///   `HISTORICAL` auto-adjust type, and to no other.
/// * `BudgetAdjustmentPeriod` counts "budget periods included in the moving-average
///   calculation", and "the maximum value depends on the `TimeUnit` granularity of the
///   budget" ([`MAX_BUDGET_ADJUSTMENT_PERIOD`]).
fn validate_auto_adjust_data(budget: &Budget) -> Result<(), String> {
    let Some(auto_adjust) = &budget.auto_adjust_data else {
        return Ok(());
    };
    let historical = auto_adjust.auto_adjust_type == AUTO_ADJUST_TYPE_HISTORICAL;
    match (&auto_adjust.historical_options, historical) {
        (None, true) => {
            return Err(
                "Unable to create/update budget: HistoricalOptions is required for a \
                 HISTORICAL AutoAdjustType."
                    .to_string(),
            )
        }
        (Some(_), false) => {
            return Err(
                "Unable to create/update budget: HistoricalOptions is only valid for a \
                 HISTORICAL AutoAdjustType."
                    .to_string(),
            )
        }
        _ => {}
    }
    if let (Some(options), Some(max)) = (
        &auto_adjust.historical_options,
        max_budget_adjustment_period(&budget.time_unit),
    ) {
        if options.budget_adjustment_period > max {
            return Err(format!(
                "Unable to create/update budget: BudgetAdjustmentPeriod must not exceed {max} \
                 for a {} budget.",
                budget.time_unit
            ));
        }
    }
    Ok(())
}

/// Record one event against `stored`'s history, as of `at`.
///
/// `ActionHistory$Status` is "the status of action at the time of the event", and
/// `ActionHistoryDetails$Action` is the action as that event left it — so both are taken
/// from the stored action AFTER the caller applied the event to it. What the record SAYS
/// follows from `event_type` alone ([`action_event_message`]), so no caller words an event
/// of its own. The record is appended, never edited: a history is append-only, bounded by
/// [`MAX_ACTION_HISTORY_RECORDS`].
pub fn record_action_event(stored: &mut StoredAction, event_type: &str, at: SystemTime) {
    stored.history.push(ActionHistory {
        timestamp: ::aws_smithy_types::DateTime::from(at),
        status: stored.action.status.clone(),
        event_type: event_type.to_string(),
        action_history_details: ActionHistoryDetails {
            message: action_event_message(event_type).to_string(),
            action: stored.action.clone(),
        },
    });
    let excess = stored
        .history
        .len()
        .saturating_sub(MAX_ACTION_HISTORY_RECORDS);
    if excess > 0 {
        stored.history.drain(..excess);
    }
}

/// The history records of `stored` that `period` covers, oldest first; a caller that
/// named no period gets the whole history.
///
/// Filtering only. The ordering rule every `TimePeriod` carries ([`time_period_ordered`])
/// is decided by the request alone, so the calling handler refuses a window that breaks
/// it — with the modelled `InvalidParameterException` — before it ever looks a resource
/// up, which is where every other rule a request carries in itself is applied too.
pub fn action_history_in_period(
    stored: &StoredAction,
    period: Option<&TimePeriod>,
) -> Vec<ActionHistory> {
    match period {
        None => stored.history.clone(),
        Some(period) => stored
            .history
            .iter()
            .filter(|record| within_time_period(&record.timestamp, period))
            .cloned()
            .collect(),
    }
}

/// The ARN namespace this service's resources are named under: the model's own
/// `arnNamespace` (`smithy_query(query="service_info")`), which is also this service's
/// signing name.
pub const ARN_SERVICE: &str = "budgets";

/// The resource part of a budget's ARN opens with this collection name, so the budget
/// name is what follows it: `budget/{BudgetName}`.
const ARN_BUDGET_PREFIX: &str = "budget/";

/// A budget ACTION is named beneath the budget it acts on (`Action$BudgetName`), so its
/// ARN is the budget's with the action's own id below it:
/// `budget/{BudgetName}/action/{ActionId}`.
///
/// `BudgetName`'s `@pattern` forbids this whole substring — both slashes — inside a
/// name, but NOT a name that merely ends in `/action`: such a name contributes its own
/// `/action` plus this separator's leading slash as an EARLIER occurrence of the
/// separator's text. `ActionId` is a UUID (`@pattern`
/// `^[a-fA-F0-9]{8}-…-[a-fA-F0-9]{12}$`, `@length(36, 36)`), so it holds no slash and
/// contributes no occurrence of its own. The separator is therefore the LAST occurrence
/// in the resource field, which is the split [`parse_resource_arn`] takes.
const ARN_ACTION_INFIX: &str = "/action/";

/// The resource an `AmazonResourceName` handed to the tagging operations names: a budget
/// or a budget action — "a budget or budget action resource" is what `TagResource`,
/// `UntagResource` and `ListTagsForResource` document as taggable here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// `arn:{partition}:budgets::{AccountId}:budget/{BudgetName}`.
    Budget {
        account_id: String,
        budget_name: String,
    },
    /// `arn:{partition}:budgets::{AccountId}:budget/{BudgetName}/action/{ActionId}`.
    Action {
        account_id: String,
        budget_name: String,
        action_id: String,
    },
}

impl TaggedResource {
    /// The account that owns the resource — the ARN's account field, which is half of
    /// the identity the store is keyed by and so the account this request names.
    ///
    /// What the ARN-addressed operations hand to [`foreign_account_refusal`], so the one
    /// account gate reads the same account whether the request spells it as an
    /// `AccountId` member or inside a `ResourceARN`.
    pub fn account_id(&self) -> &str {
        match self {
            Self::Budget { account_id, .. } | Self::Action { account_id, .. } => account_id,
        }
    }
}

/// The resource `arn` names, or `Err` with the sentence that goes in
/// `InvalidParameterException$Message` for a value that is not an ARN of a resource this
/// service holds.
///
/// An ARN has six colon-separated fields — `arn:{partition}:{service}:{region}:{account}:{resource}`
/// — and only the resource field may itself hold colons, which is why the split is
/// bounded. A budgets resource is global, so the region field is empty; the account
/// field is the account that owns the budget, spelled the way `AccountId` is modelled
/// (`@pattern ^\d{12}$`), since that half of a budget's identity cannot be read off the
/// name alone.
///
/// The refused value is NOT echoed back: an ARN carries a caller-supplied budget name,
/// and a name is never reflected into a refusal.
pub fn parse_resource_arn(arn: &str) -> Result<TaggedResource, String> {
    let malformed = || {
        format!(
            "Unable to resolve ResourceARN: expected an ARN of the form \
             arn:<partition>:{ARN_SERVICE}::<account-id>:{ARN_BUDGET_PREFIX}<budget-name> or \
             arn:<partition>:{ARN_SERVICE}::<account-id>:{ARN_BUDGET_PREFIX}<budget-name>\
             {ARN_ACTION_INFIX}<action-id>."
        )
    };
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    // Destructured rather than indexed, so a value with fewer fields is a refusal rather
    // than a panic on the request path.
    let [prefix, partition, service, region, account_id, resource] = fields.as_slice() else {
        return Err(malformed());
    };
    // `arn`, a non-empty partition, this service's namespace, and the empty region every
    // global service's ARN carries.
    if *prefix != "arn" || partition.is_empty() || *service != ARN_SERVICE || !region.is_empty() {
        return Err(malformed());
    }
    if account_id.len() != 12 || !account_id.chars().all(|digit| digit.is_ascii_digit()) {
        return Err(malformed());
    }
    let resource = resource.strip_prefix(ARN_BUDGET_PREFIX).ok_or_else(malformed)?;
    // The separator is the LAST occurrence, not the first: a budget name may END in
    // `/action` (only the two-slash substring is forbidden inside a name), and such a
    // name plus this separator's leading slash spells the separator's own text one
    // position earlier. Taking the last occurrence resolves the ARN back to the budget
    // name it was rendered from, whatever that name ends in.
    match resource.rsplit_once(ARN_ACTION_INFIX) {
        Some((budget_name, action_id)) => {
            if budget_name.is_empty() || action_id.is_empty() {
                return Err(malformed());
            }
            Ok(TaggedResource::Action {
                account_id: account_id.to_string(),
                budget_name: budget_name.to_string(),
                action_id: action_id.to_string(),
            })
        }
        None => {
            if resource.is_empty() {
                return Err(malformed());
            }
            Ok(TaggedResource::Budget {
                account_id: account_id.to_string(),
                budget_name: resource.to_string(),
            })
        }
    }
}

/// The tags `target` holds, or `None` when this store holds no such resource — which the
/// caller answers with `NotFoundException`.
///
/// A READ path: it does not materialise an action's owed settle, because a tag list does
/// not depend on an action's status.
pub fn resource_tags<'a>(
    state: &'a ComAmazonawsBudgetsAwsbudgetservicegatewayState,
    target: &TaggedResource,
) -> Option<&'a Vec<ResourceTag>> {
    match target {
        TaggedResource::Budget {
            account_id,
            budget_name,
        } => state
            .budgets
            .get(&(account_id.clone(), budget_name.clone()))
            .map(|stored| &stored.tags),
        TaggedResource::Action {
            account_id,
            budget_name,
            action_id,
        } => state
            .budgets
            .get(&(account_id.clone(), budget_name.clone()))?
            .actions
            .iter()
            .find(|held| &held.action.action_id == action_id)
            .map(|held| &held.tags),
    }
}

/// As [`resource_tags`], for a call that writes them.
pub fn resource_tags_mut<'a>(
    state: &'a mut ComAmazonawsBudgetsAwsbudgetservicegatewayState,
    target: &TaggedResource,
) -> Option<&'a mut Vec<ResourceTag>> {
    match target {
        TaggedResource::Budget {
            account_id,
            budget_name,
        } => state
            .budgets
            .get_mut(&(account_id.clone(), budget_name.clone()))
            .map(|stored| &mut stored.tags),
        TaggedResource::Action {
            account_id,
            budget_name,
            action_id,
        } => state
            .budgets
            .get_mut(&(account_id.clone(), budget_name.clone()))?
            .actions
            .iter_mut()
            .find(|held| &held.action.action_id == action_id)
            .map(|held| &mut held.tags),
    }
}

/// How many tags one resource will hold: `ResourceTagList` is `@length(0, 200)`, so a
/// list of 200 is the longest tag set this service can report for a resource or accept
/// for one. A write that would take a resource past it is answered with the modelled
/// `ServiceQuotaExceededException` ("You've reached a Service Quota limit on this
/// resource"), since the per-REQUEST half of that bound is already refused at the
/// boundary.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The refusal a tag set naming one key twice takes, or `None` when every key in it is
/// distinct: "each key must be unique for the resource" (`ResourceTags`), so a request
/// that gives one key two values states no single value for it.
///
/// `Err` carries the sentence that goes in `InvalidParameterException$Message`. The key
/// IS named: it is the caller's own tag key, and the caller cannot fix the request
/// without knowing which one repeated.
pub fn duplicate_tag_key_violation(supplied: &[ResourceTag]) -> Option<String> {
    let mut seen: Vec<&str> = Vec::with_capacity(supplied.len());
    for tag in supplied {
        if seen.contains(&tag.key.as_str()) {
            return Some(format!(
                "Unable to apply ResourceTags: the tag key {} is given more than once, and \
                 each key must be unique for the resource.",
                tag.key
            ));
        }
        seen.push(tag.key.as_str());
    }
    None
}

/// How many tags `held` would hold once `supplied` is applied over it — the count the
/// ceiling above is checked against, counted BEFORE anything is written so a refused
/// write changes nothing. A supplied key the resource already carries replaces that
/// tag's value rather than adding a tag, so only the keys that are new count.
pub fn tag_count_after(held: &[ResourceTag], supplied: &[ResourceTag]) -> usize {
    held.len()
        + supplied
            .iter()
            .filter(|tag| !held.iter().any(|existing| existing.key == tag.key))
            .count()
}

/// Apply `supplied` over the tags `held` by a resource: a key the resource already
/// carries takes the supplied value IN PLACE, and a key it does not carry is appended.
///
/// Both halves follow from one rule — "each key must be unique for the resource"
/// (`ResourceTags`): a resource holds at most one tag per key, so tagging a key twice
/// re-values it. Keeping the existing tag's position makes the tag list's order the
/// order the keys were first created in, which is what a read reports.
pub fn apply_tags(held: &mut Vec<ResourceTag>, supplied: Vec<ResourceTag>) {
    for tag in supplied {
        match held.iter_mut().find(|existing| existing.key == tag.key) {
            Some(existing) => existing.value = tag.value,
            None => held.push(tag),
        }
    }
}

/// Drop from the tags `held` by a resource every tag whose key is named in `keys`.
///
/// The mirror of [`apply_tags`], from the same rule — a resource holds at most one tag
/// per key ("each key must be unique for the resource") — so a key names at most one tag
/// to delete and naming it is enough to delete it: no value is supplied, and a key the
/// resource does not carry has nothing to delete, which is already the state the request
/// asks for rather than a failure. Repeating a key in one request therefore states the
/// same deletion twice, not two conflicting ones.
///
/// The tags that stay keep their relative order, so the order a read reports — the order
/// the keys were first created in — survives a deletion in the middle of the set.
pub fn remove_tag_keys(held: &mut Vec<ResourceTag>, keys: &[String]) {
    held.retain(|tag| !keys.iter().any(|key| key == &tag.key));
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBudgetsAwsbudgetservicegatewayState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>)
}
