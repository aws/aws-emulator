// SPDX-License-Identifier: CC0-1.0

//! The STACK SET STATUS-CHANGE EVENTS this service publishes onto the account's default
//! EventBridge bus.
//!
//! ## The rule this module implements
//!
//! CloudFormation is an event SOURCE for its stack sets, not only a control plane a client
//! polls: every status change of a stack set, of a stack-set OPERATION and of a stack
//! INSTANCE is published onto the account's default event bus ([`DEFAULT_EVENT_BUS`]) under
//! the `source` this service owns ([`EVENT_SOURCE`]) and the `detail-type` of that kind of
//! change ([`StackSetChange::detail_type`]), in the Region the stack set belongs to, naming
//! the stack set's ARN in the event's `resources` and in `detail.stack-set-arn`. A rule on
//! that bus whose `EventPattern` matches then delivers it to that rule's targets, which is
//! how a pipeline gates on a rollout finishing, or an alarm fires on one failing, without
//! the client polling anything.
//!
//! What each `detail` carries is the record the change happened to, and nothing else: the
//! operation's own id, action and status for an operation, the instance's stack id and its
//! status, detailed status and status reason for an instance, and the stack set's status for
//! the stack set itself. Every variable part is read off the stored record a read of the
//! same instant answers from, so an event and a `DescribeStackSetOperation` /
//! `ListStackInstances` / `ListStackSets` read of the same moment cannot disagree.
//!
//! ## When an event is queued, and when it is published
//!
//! Nothing here decides WHAT changed. A status this service writes is what queues the event
//! for it ([`super::stack_sets::StoredStackSet::note_status_change`] for the stack set,
//! [`super::stack_sets::StoredStackSet::record_operation`] for an operation starting, and
//! that stack set's own settle for every conclusion the clock makes due), so a change
//! nothing made publishes nothing and a refused call publishes nothing either.
//!
//! Publishing is a call to ANOTHER service, which cannot be made where the change is
//! recorded — that is under this store's lock. So each change is queued on the stack set it
//! belongs to and [`publish_pending`] is what makes the call: it CLAIMS every event the
//! stack sets of a Region owe and publishes what it claimed, on a task of its own, since the
//! operation that queued them still holds the lock the claim needs — the same shape
//! [`super::stack_notifications`] uses for the other notification a change of this store
//! owes. Claiming is TAKING, so an event is published exactly once however many callers
//! drive it.
//!
//! A conclusion the clock owes falls due after the call that started the operation has
//! answered, and a client driven by these events never calls again — so the operation that
//! starts one also starts [`settle_when_due`], which materialises exactly the transition
//! the clock has made due, through the same one settle path a status read uses, and
//! publishes what that settled. The work both of those start is bounded
//! ([`MAX_TASKS_IN_FLIGHT`]).
//!
//! Publication is BEST EFFORT: an emulator with no EventBridge mounted has nowhere to
//! publish, and no caller is failed because the fan-out could not be made.

use ::std::sync::atomic::{AtomicUsize, Ordering};
use ::std::sync::Arc;
use ::std::time::{Duration, SystemTime, UNIX_EPOCH};

use smithy_server_runtime::EmbeddedEmulator;

/// The `source` of every event CloudFormation publishes: EventBridge names the publishing
/// service as `aws.<endpoint-prefix>`, and this service's prefix is `cloudformation`.
pub const EVENT_SOURCE: &str = "aws.cloudformation";

/// The bus these events go to: the events an AWS service generates are published onto the
/// account's default event bus, which is the bus a rule carrying no `EventBusName` is
/// created on.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// The `detail-type` of a change to the STACK SET itself.
pub const STACK_SET_STATUS_CHANGE: &str = "CloudFormation StackSet Status Change";

/// The `detail-type` of a change to a stack-set OPERATION.
pub const STACK_SET_OPERATION_STATUS_CHANGE: &str =
    "CloudFormation StackSet Operation Status Change";

/// The `detail-type` of a change to one stack INSTANCE of a stack set.
pub const STACK_SET_STACK_INSTANCE_STATUS_CHANGE: &str =
    "CloudFormation StackSet StackInstance Status Change";

/// How many entries one `PutEvents` request carries: the bound the service published to
/// declares (`PutEventsRequestEntryList` is `@length(min: 1, max: 10)`), so a claim holding
/// more is published in several requests rather than one that service would refuse.
///
/// Shared with the sibling publisher ([`super::drift_events`]), which publishes onto the
/// same bus through the same service.
pub(super) const MAX_PUT_EVENTS_ENTRIES: usize = 10;

/// How many unpublished events ONE stack set holds.
///
/// THIS CEILING IS THE EMULATOR'S: a queued event waits for the next caller to publish it,
/// and an emulator with no EventBridge mounted never drains one, so the queue is bounded
/// here and the oldest entries are reclaimed first. It is the operations one stack set
/// retains ([`super::stack_sets::MAX_OPERATIONS_PER_STACK_SET`]) times the events one
/// operation's start and conclusion can owe over the instances it acts on, so no realistic
/// lifecycle loses an event to it.
pub const MAX_PENDING_EVENTS: usize = 512;

/// How many tasks THIS SERVICE's event publishers may have outstanding at once across this
/// process — the publications [`publish_pending`] starts and the settles
/// [`settle_when_due`] does, here and in the sibling publisher ([`super::drift_events`]),
/// which takes its slots from the same count.
///
/// THIS CEILING IS THE EMULATOR'S: each task is one timer, one claim and one `PutEvents`,
/// and a stream of calls must not leave unbounded work outstanding. A call that
/// finds no slot free starts no task — its events stay on the (bounded) queue for the next
/// publication, and its conclusion is settled by the next read of it, exactly as it was
/// before this existed.
pub(super) const MAX_TASKS_IN_FLIGHT: usize = 64;

/// How long after the conclusion fell due [`settle_when_due`] settles it: enough that the
/// settle runs with the due instant already past rather than racing it.
const SETTLE_SLACK: Duration = Duration::from_millis(250);

/// How many times ONE [`settle_when_due`] task waits for a conclusion whose due instant
/// moved out from under it (the operation's own call put it further out, or a later call
/// did). Bounded so a moving target cannot keep a task alive indefinitely; an operation
/// still in flight after them is settled by the next read of it.
const MAX_SETTLE_WAITS: usize = 4;

/// The tasks this module has outstanding right now.
static TASKS_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// WHAT changed, which is both the event's `detail-type` and what its `detail` states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StackSetChange {
    /// The stack set itself reached `status` (`StackSet$Status`).
    StackSet {
        /// [`super::stack_sets::status`]: the status the stack set now carries.
        status: String,
    },
    /// One stack-set operation reached `status`.
    Operation {
        /// `OperationId`: "the unique ID of the StackSet operation".
        operation_id: String,
        /// `Action`: [`super::stack_sets::operation_action`].
        action: String,
        /// `Status`: [`super::stack_sets::operation_status`].
        status: String,
    },
    /// One stack instance of the stack set reached `status`.
    StackInstance {
        /// `StackId`: "the ID of the stack instance" — the stack the instance deploys.
        stack_id: String,
        /// `Status`: [`super::stack_sets::instance_status`].
        status: String,
        /// `StackInstanceStatus$DetailedStatus`: [`super::stack_sets::detailed_status`].
        detailed_status: String,
        /// `StatusReason`: "the explanation for the specific status code assigned to this
        /// stack instance", absent for an instance that has none.
        status_reason: Option<String>,
    },
}

impl StackSetChange {
    /// The `detail-type` a change of this kind is published under.
    #[must_use]
    pub fn detail_type(&self) -> &'static str {
        match self {
            Self::StackSet { .. } => STACK_SET_STATUS_CHANGE,
            Self::Operation { .. } => STACK_SET_OPERATION_STATUS_CHANGE,
            Self::StackInstance { .. } => STACK_SET_STACK_INSTANCE_STATUS_CHANGE,
        }
    }

    /// The event `detail` this change is reported in, for the stack set `stack_set_arn`.
    ///
    /// Public because it IS the contract: the members a rule's `EventPattern` filters on,
    /// which is what a suite of this crate holds the published event to. The status of
    /// whatever changed is stated under `status-details`, which is where every one of these
    /// three events carries it.
    #[must_use]
    pub fn detail(&self, stack_set_arn: &str) -> ::serde_json::Value {
        let mut detail = ::serde_json::Map::new();
        detail.insert(
            "stack-set-arn".to_string(),
            ::serde_json::Value::String(stack_set_arn.to_string()),
        );
        let mut status_details = ::serde_json::Map::new();
        match self {
            Self::StackSet { status } => {
                status_details.insert(
                    "status".to_string(),
                    ::serde_json::Value::String(status.clone()),
                );
            }
            Self::Operation {
                operation_id,
                action,
                status,
            } => {
                detail.insert(
                    "stack-set-operation-id".to_string(),
                    ::serde_json::Value::String(operation_id.clone()),
                );
                detail.insert(
                    "action".to_string(),
                    ::serde_json::Value::String(action.clone()),
                );
                status_details.insert(
                    "status".to_string(),
                    ::serde_json::Value::String(status.clone()),
                );
            }
            Self::StackInstance {
                stack_id,
                status,
                detailed_status,
                status_reason,
            } => {
                detail.insert(
                    "stack-id".to_string(),
                    ::serde_json::Value::String(stack_id.clone()),
                );
                status_details.insert(
                    "status".to_string(),
                    ::serde_json::Value::String(status.clone()),
                );
                status_details.insert(
                    "detailed-status".to_string(),
                    ::serde_json::Value::String(detailed_status.clone()),
                );
                if let Some(reason) = status_reason {
                    status_details.insert(
                        "status-reason".to_string(),
                        ::serde_json::Value::String(reason.clone()),
                    );
                }
            }
        }
        detail.insert(
            "status-details".to_string(),
            ::serde_json::Value::Object(status_details),
        );
        ::serde_json::Value::Object(detail)
    }
}

/// ONE queued event: the change, the stack set it is about and WHEN it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackSetEvent {
    /// The Region the event is published in: the stack set's own, which is the Region the
    /// administrator call that changed it was signed for.
    pub region: String,
    /// The stack set's ARN, which the event names in `resources` and in
    /// `detail.stack-set-arn`.
    pub stack_set_arn: String,
    /// What changed ([`StackSetChange`]).
    pub change: StackSetChange,
    /// WHEN the change happened — the instant the record was written or the conclusion fell
    /// due, not the instant the queue happened to be drained.
    pub at: SystemTime,
}

/// Publish every event the stack sets of `region` owe, on a task of its own.
///
/// Called by the operation that queued them, which still holds this store's lock: the
/// publication has to CLAIM what it publishes, which needs that lock, and it then reaches
/// EventBridge's own handler, which matches the event against the account's rules and
/// delivers it to their targets — a target that calls back into this service would deadlock
/// against the very call the event set off. So the publication is taken on a task, which
/// takes the lock once the call that queued the events has answered and holds none of its
/// own while it publishes.
///
/// Claiming is TAKING, so an event is published exactly once however many callers drive
/// this. The task takes one of [`MAX_TASKS_IN_FLIGHT`] slots and gives it back however it
/// ends; a call that finds none free publishes nothing, and the events it queued are
/// published by the next call that does.
pub fn publish_pending<Eng: Send + Sync + 'static>(
    engine: Arc<Eng>,
    callback: EmbeddedEmulator,
    region: String,
) {
    if super::service_state(&*engine).is_none() {
        return;
    }
    let Some(slot) = TaskSlot::claim() else {
        ::tracing::warn!(
            "{MAX_TASKS_IN_FLIGHT} StackSet event tasks are already in flight; these events \
             will be published by the next StackSet call"
        );
        return;
    };
    ::tokio::spawn(async move {
        let _slot = slot;
        let claimed = match super::service_state(&*engine) {
            None => Vec::new(),
            Some(state) => state.lock().claim_stack_set_events(&region),
        };
        publish(&callback, &region, &claimed).await;
    });
}

/// Publish what the stack set of `settling` owes and settle that operation's conclusion when
/// it comes due: what every call that STARTS a stack-set operation owes this bus, in one
/// call, so no handler states half of it.
pub fn publish_and_settle<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    callback: &EmbeddedEmulator,
    settling: SettlingOperation,
) {
    publish_pending(
        Arc::clone(engine),
        callback.clone(),
        settling.region.clone(),
    );
    settle_when_due(Arc::clone(engine), callback.clone(), settling);
}

/// Publish `events` onto the default bus of `region`.
async fn publish(callback: &EmbeddedEmulator, region: &str, events: &[StackSetEvent]) {
    if events.is_empty() {
        return;
    }
    let client = ::service_common::eventbridge_client::eventbridge_client(callback, region);
    for window in events.chunks(MAX_PUT_EVENTS_ENTRIES) {
        let mut request = client.put_events();
        for event in window {
            let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                .event_bus_name(DEFAULT_EVENT_BUS)
                .source(EVENT_SOURCE)
                .detail_type(event.change.detail_type())
                // The event's `time` is WHEN the change happened:
                // `PutEventsRequestEntry$Time` is "the time stamp of the event, per
                // RFC3339. If no time stamp is provided, the time stamp of the PutEvents
                // call is used."
                .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                    epoch_millis(event.at),
                ))
                // "The Amazon Web Services resources, identified by Amazon Resource Name
                // (ARN), which the event primarily concerns": the stack set the change is
                // about.
                .resources(&event.stack_set_arn)
                .detail(event.change.detail(&event.stack_set_arn).to_string())
                .build();
            request = request.entries(entry);
        }
        if let Err(error) = request.send().await {
            ::tracing::warn!(
                region = %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a CloudFormation StackSet status change could not be published to EventBridge"
            );
        }
    }
}

/// WHICH operation one [`settle_when_due`] task waits for: the stack set that holds it,
/// addressed the way every read of this store addresses one, and the operation's own id.
#[derive(Debug, Clone)]
pub struct SettlingOperation {
    /// The Region the stack set belongs to, which is also the Region its events are
    /// published in.
    pub region: String,
    /// The stack set holding the operation, by name or id.
    pub stack_set_id: String,
    /// `OperationId`: the operation whose conclusion this task waits for.
    pub operation_id: String,
}

/// Settle the conclusion the clock owes the operation `settling` names once it is due, and
/// publish what that settled.
///
/// Returns at once: the settle runs in a task of its own, because the call that started the
/// operation has to answer while that operation is still running. It exists for the client
/// driven by these events — such a client never calls this service again, so without it the
/// conclusion's events would wait for a status read that never comes.
///
/// WHEN the conclusion is due is read off the operation itself rather than recomputed here,
/// so an operation whose own call put its conclusion further out (a drift detection) is
/// waited for as long as it really takes; a due instant that moves while the task waits is
/// waited for again, at most [`MAX_SETTLE_WAITS`] times, so the task cannot be kept alive
/// indefinitely by a moving target.
///
/// It materialises exactly the transitions the clock has already made due, through the same
/// one settle path a status read uses, so what this writes is what a read of the same
/// instant would have written; a conclusion a reader already settled is left alone, and each
/// event is published exactly once whichever got there first.
pub fn settle_when_due<Eng: Send + Sync + 'static>(
    engine: Arc<Eng>,
    callback: EmbeddedEmulator,
    settling: SettlingOperation,
) {
    if super::service_state(&*engine).is_none() {
        return;
    }
    let Some(slot) = TaskSlot::claim() else {
        ::tracing::warn!(
            "{MAX_TASKS_IN_FLIGHT} StackSet event tasks are already in flight; this \
             operation's conclusion will be settled by the next read of it"
        );
        return;
    };
    ::tokio::spawn(async move {
        let _slot = slot;
        for _ in 0..MAX_SETTLE_WAITS {
            let Some(due) = conclusion_due(&*engine, &settling) else {
                // The operation has concluded — settled by this task or by a reader that
                // got there first — so there is nothing left to wait for.
                break;
            };
            let wait = due
                .duration_since(SystemTime::now())
                .unwrap_or_default()
                .saturating_add(SETTLE_SLACK);
            ::tokio::time::sleep(wait).await;
            let claimed = match super::service_state(&*engine) {
                None => Vec::new(),
                Some(state) => {
                    let mut store = state.lock();
                    store.settle(SystemTime::now());
                    store.claim_stack_set_events(&settling.region)
                }
            };
            publish(&callback, &settling.region, &claimed).await;
        }
    });
}

/// The instant the conclusion of the operation `settling` names is due, or `None` for one
/// this store no longer holds in flight.
fn conclusion_due<Eng: Send + Sync + 'static>(
    engine: &Eng,
    settling: &SettlingOperation,
) -> Option<SystemTime> {
    let state = super::service_state(engine)?;
    let store = state.lock();
    let pending = store
        .resolve_stack_set(&settling.region, &settling.stack_set_id)?
        .operation(&settling.operation_id)?
        .pending
        .as_ref()?;
    Some(pending.due_at)
}

/// ONE of the [`MAX_TASKS_IN_FLIGHT`] slots, held for as long as the task that took it runs.
///
/// The slot is given back when this is DROPPED, so a task that ends early — or panics —
/// cannot leak it; a task holds its slot by owning one of these. Shared with the sibling
/// publisher ([`super::drift_events`]), so every event task of this service is counted
/// against the one ceiling.
pub(super) struct TaskSlot;

impl TaskSlot {
    /// Take one slot, or `None` when they are all taken: the count is only ever raised from
    /// below the ceiling, so two callers cannot both take the last one.
    pub(super) fn claim() -> Option<Self> {
        TASKS_IN_FLIGHT
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |held| {
                (held < MAX_TASKS_IN_FLIGHT).then_some(held + 1)
            })
            .ok()
            .map(|_| Self)
    }
}

impl Drop for TaskSlot {
    fn drop(&mut self) {
        TASKS_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// `at` as the epoch milliseconds the event's `time` carries.
fn epoch_millis(at: SystemTime) -> i64 {
    i64::try_from(
        at.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or_default()
}

impl super::ComAmazonawsCloudformationCloudformationState {
    /// Take every event the stack sets of `region` owe, in the order the changes happened.
    ///
    /// Claiming is TAKING: an event leaves the queue as it is claimed, so each one is
    /// published once however many callers drive [`publish_pending`].
    pub fn claim_stack_set_events(&mut self, region: &str) -> Vec<StackSetEvent> {
        let mut claimed: Vec<StackSetEvent> = Vec::new();
        for stack_set in self
            .stack_sets
            .iter_mut()
            .filter(|stack_set| stack_set.region == region)
        {
            claimed.append(&mut stack_set.pending_events);
        }
        claimed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `detail` of each kind of change states the members a rule filters on, and the
    /// status of whatever changed under `status-details`.
    #[test]
    fn each_change_states_its_own_members() {
        let arn = "arn:aws:cloudformation:us-east-1:123456789012:stackset/set:abc";
        for (change, expected) in [
            (
                StackSetChange::StackSet {
                    status: "ACTIVE".to_string(),
                },
                ::serde_json::json!({
                    "stack-set-arn": arn,
                    "status-details": { "status": "ACTIVE" },
                }),
            ),
            (
                StackSetChange::Operation {
                    operation_id: "op-1".to_string(),
                    action: "CREATE".to_string(),
                    status: "RUNNING".to_string(),
                },
                ::serde_json::json!({
                    "stack-set-arn": arn,
                    "stack-set-operation-id": "op-1",
                    "action": "CREATE",
                    "status-details": { "status": "RUNNING" },
                }),
            ),
            (
                StackSetChange::StackInstance {
                    stack_id: "stack-1".to_string(),
                    status: "CURRENT".to_string(),
                    detailed_status: "SUCCEEDED".to_string(),
                    status_reason: None,
                },
                ::serde_json::json!({
                    "stack-set-arn": arn,
                    "stack-id": "stack-1",
                    "status-details": { "status": "CURRENT", "detailed-status": "SUCCEEDED" },
                }),
            ),
            (
                StackSetChange::StackInstance {
                    stack_id: "stack-2".to_string(),
                    status: "OUTDATED".to_string(),
                    detailed_status: "FAILED".to_string(),
                    status_reason: Some("why".to_string()),
                },
                ::serde_json::json!({
                    "stack-set-arn": arn,
                    "stack-id": "stack-2",
                    "status-details": {
                        "status": "OUTDATED",
                        "detailed-status": "FAILED",
                        "status-reason": "why",
                    },
                }),
            ),
        ] {
            let actual = change.detail(arn);
            assert_eq!(
                actual, expected,
                "case {change:?}: expected {expected}, got {actual}"
            );
        }
    }
}
