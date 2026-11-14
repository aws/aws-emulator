// SPDX-License-Identifier: CC0-1.0

//! The DRIFT DETECTION STATUS-CHANGE EVENTS this service publishes onto the account's
//! default EventBridge bus.
//!
//! ## The rule this module implements
//!
//! A drift detection is ASYNCHRONOUS — the call that starts one is answered with its id
//! while the detection is still running — so CloudFormation is an event SOURCE for it and
//! not only a status a client polls: every status a drift-detection operation reaches is
//! published onto the account's default event bus
//! ([`super::stack_set_events::DEFAULT_EVENT_BUS`]) under the `source` this service owns
//! ([`super::stack_set_events::EVENT_SOURCE`]) and the one `detail-type` such a change
//! carries ([`DRIFT_DETECTION_STATUS_CHANGE`]), in the Region the stack belongs to, naming
//! the stack's id in the event's `resources` and in `detail.stack-id`. A rule on that bus
//! whose `EventPattern` matches then delivers it to that rule's targets, which is how a
//! client alarms on drift, or gates a deployment on a detection concluding, without polling
//! anything.
//!
//! What each `detail` carries is the detection record a read of the same instant answers
//! from: the detection's own id, the status it reached and, under
//! `drift-detection-details`, how many of the stack's resources that detection found
//! drifted. While the detection is still running there is no verdict and no count yet, which
//! the event states with the two sentinels AWS publishes for them
//! ([`NO_VERDICT_YET`], [`NO_DRIFTED_COUNT_YET`]) rather than by leaving the members out, so
//! a consumer can tell an unconcluded detection from a concluded one on the same two
//! members.
//!
//! ## When an event is queued, and when it is published
//!
//! Nothing here decides WHAT changed. A detection status this service writes is what queues
//! the event for it — the call that starts the detection
//! ([`crate::ops::detect_stack_drift`]) for the status it is accepted in, and that
//! detection's own settle
//! ([`super::ComAmazonawsCloudformationCloudformationState::settle`]) for the conclusion
//! the clock makes due — so a detection nobody started publishes nothing, and a refused
//! call publishes nothing either.
//!
//! Publishing is a call to ANOTHER service, which cannot be made where the change is
//! recorded: that is under this store's lock. So each change is queued on the detection it
//! belongs to and [`publish_pending`] is what makes the call, on a task of its own, exactly
//! as the sibling publisher [`super::stack_set_events`] does for a stack set's changes —
//! it CLAIMS every event the detections of a Region owe and publishes what it claimed.
//! Claiming is TAKING, so an event is published exactly once however many callers drive it.
//!
//! A detection's conclusion falls due after the call that started it has answered, and a
//! client driven by these events never calls again — so that call also starts
//! [`settle_when_due`], which materialises exactly the transition the clock has made due,
//! through the same one settle path a status read uses, and publishes what that settled.
//!
//! Publication is BEST EFFORT: an emulator with no EventBridge mounted has nowhere to
//! publish, and no caller is failed because the fan-out could not be made.

use ::std::sync::Arc;
use ::std::time::{Duration, SystemTime, UNIX_EPOCH};

use smithy_server_runtime::EmbeddedEmulator;

use super::stack_set_events::{
    TaskSlot, DEFAULT_EVENT_BUS, EVENT_SOURCE, MAX_PUT_EVENTS_ENTRIES, MAX_TASKS_IN_FLIGHT,
};

/// The `detail-type` of every status a drift-detection operation reaches.
pub const DRIFT_DETECTION_STATUS_CHANGE: &str = "CloudFormation Drift Detection Status Change";

/// The `detail.status-details.stack-drift-status` of a detection that has not concluded:
/// EMPTY, because such a detection has reached no verdict about the stack yet — the same
/// thing `DescribeStackDriftDetectionStatus` states by answering no `StackDriftStatus` at
/// all ("this value will be NULL for any stack drift detection operation that has not yet
/// concluded").
pub const NO_VERDICT_YET: &str = "";

/// The `detail.drift-detection-details.drifted-stack-resource-count` of a detection that has
/// not concluded: `-1`, a count no real detection can report, because there is no count yet
/// — the same thing `DescribeStackDriftDetectionStatus` states by answering no
/// `DriftedStackResourceCount` ("this is NULL until the drift detection operation reaches a
/// status of DETECTION_COMPLETE").
pub const NO_DRIFTED_COUNT_YET: i32 = -1;

/// How many unpublished events ONE detection holds.
///
/// THIS CEILING IS THE EMULATOR'S: a queued event waits for the next caller to publish it,
/// and an emulator with no EventBridge mounted never drains one, so the queue is bounded
/// here and the oldest entries are reclaimed first. A detection owes exactly two of them —
/// the status it is accepted in and the conclusion the clock makes due — so no detection
/// loses an event to it.
pub const MAX_PENDING_EVENTS: usize = 4;

/// How long after the conclusion fell due [`settle_when_due`] settles it: enough that the
/// settle runs with the due instant already past rather than racing it.
const SETTLE_SLACK: Duration = Duration::from_millis(250);

/// How many times ONE [`settle_when_due`] task waits for a conclusion whose due instant
/// moved out from under it. Bounded so a moving target cannot keep a task alive
/// indefinitely; a detection still in flight after them is settled by the next read of it.
const MAX_SETTLE_WAITS: usize = 4;

/// ONE queued event: the detection it is about, the status that detection reached and WHEN
/// it reached it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftDetectionEvent {
    /// The Region the event is published in: the stack's own, which is where its resources
    /// live.
    pub region: String,
    /// "The ID of the stack" the detection ran against, which the event names in `resources`
    /// and in `detail.stack-id`.
    pub stack_id: String,
    /// `StackDriftDetectionId`: the id the call that started the detection answered with,
    /// which is what joins both of a detection's events to that call.
    pub detection_id: String,
    /// [`super::drift_detection_status`]: the status the detection reached.
    pub detection_status: String,
    /// The stack-level verdict the detection reached, or [`NO_VERDICT_YET`] while it is
    /// still running.
    pub stack_drift_status: String,
    /// How many of the stack's resources the detection found drifted, or
    /// [`NO_DRIFTED_COUNT_YET`] while it is still running.
    pub drifted_resource_count: i32,
    /// WHEN the detection reached that status — the instant it was accepted or the instant
    /// its conclusion fell due, not the instant the queue happened to be drained.
    pub at: SystemTime,
}

impl DriftDetectionEvent {
    /// The event `detail` this change is reported in.
    ///
    /// Public because it IS the contract: the members a rule's `EventPattern` filters on,
    /// which is what a suite of this emulator holds the published event to.
    #[must_use]
    pub fn detail(&self) -> ::serde_json::Value {
        ::serde_json::json!({
            "stack-id": self.stack_id,
            "stack-drift-detection-id": self.detection_id,
            "status-details": {
                "detection-status": self.detection_status,
                "stack-drift-status": self.stack_drift_status,
            },
            "drift-detection-details": {
                "drifted-stack-resource-count": self.drifted_resource_count,
            },
        })
    }
}

impl super::StoredDriftDetection {
    /// Queue the event this detection owes for the status it carries RIGHT NOW, as of `at`.
    ///
    /// Everything the event states is read off this record, so an event and a
    /// `DescribeStackDriftDetectionStatus` read of the same instant cannot disagree; a
    /// detection that has not concluded has no verdict and no count, which the event states
    /// with the two sentinels AWS publishes for them.
    ///
    /// Bounded at [`MAX_PENDING_EVENTS`]: a queue nothing drains — an emulator with no
    /// EventBridge mounted — must not grow without limit, and the oldest entry goes first.
    pub fn queue_event(&mut self, at: SystemTime) {
        self.pending_events.push(DriftDetectionEvent {
            region: self.region.clone(),
            stack_id: self.stack_id.clone(),
            detection_id: self.detection_id.clone(),
            detection_status: self.detection_status.clone(),
            stack_drift_status: self
                .stack_drift_status
                .clone()
                .unwrap_or_else(|| NO_VERDICT_YET.to_string()),
            drifted_resource_count: self
                .drifted_resource_count
                .unwrap_or(NO_DRIFTED_COUNT_YET),
            at,
        });
        if self.pending_events.len() > MAX_PENDING_EVENTS {
            let excess = self.pending_events.len() - MAX_PENDING_EVENTS;
            self.pending_events.drain(..excess);
        }
    }
}

impl super::ComAmazonawsCloudformationCloudformationState {
    /// Take every event the drift detections of `region` owe, in the order the statuses were
    /// reached.
    ///
    /// Claiming is TAKING: an event leaves the queue as it is claimed, so each one is
    /// published once however many callers drive [`publish_pending`].
    pub fn claim_drift_events(&mut self, region: &str) -> Vec<DriftDetectionEvent> {
        let mut claimed: Vec<DriftDetectionEvent> = Vec::new();
        for detection in self
            .drift_detections
            .iter_mut()
            .filter(|detection| detection.region == region)
        {
            claimed.append(&mut detection.pending_events);
        }
        claimed
    }
}

/// Publish every event the drift detections of `region` owe, on a task of its own.
///
/// Called by the operation that queued them, which may still hold this store's lock: the
/// publication has to CLAIM what it publishes, which needs that lock, and it then reaches
/// EventBridge's own handler, which matches the event against the account's rules and
/// delivers it to their targets — a target that calls back into this service would deadlock
/// against the very call the event set off. So the publication is taken on a task, which
/// takes the lock once the call that queued the events has answered and holds none of its
/// own while it publishes.
///
/// Claiming is TAKING, so an event is published exactly once however many callers drive
/// this. The task takes one of the slots this service's event tasks share and gives it back
/// however it ends; a call that finds none free publishes nothing, and the events it queued
/// are published by the next call that does.
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
            "{MAX_TASKS_IN_FLIGHT} CloudFormation event tasks are already in flight; these \
             drift detection events will be published by the next call that drains them"
        );
        return;
    };
    ::tokio::spawn(async move {
        let _slot = slot;
        let claimed = match super::service_state(&*engine) {
            None => Vec::new(),
            Some(state) => state.lock().claim_drift_events(&region),
        };
        publish(&callback, &region, &claimed).await;
    });
}

/// Publish what the detection `settling` names owes and settle its conclusion when it comes
/// due: what the call that STARTS a drift detection owes this bus, in one call, so no
/// handler states half of it.
pub fn publish_and_settle<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    callback: &EmbeddedEmulator,
    settling: SettlingDetection,
) {
    publish_pending(
        Arc::clone(engine),
        callback.clone(),
        settling.region.clone(),
    );
    settle_when_due(Arc::clone(engine), callback.clone(), settling);
}

/// WHICH detection one [`settle_when_due`] task waits for.
#[derive(Debug, Clone)]
pub struct SettlingDetection {
    /// The Region the stack belongs to, which is also the Region its events are published
    /// in.
    pub region: String,
    /// `StackDriftDetectionId`: the detection whose conclusion this task waits for.
    pub detection_id: String,
}

/// Settle the conclusion the clock owes the detection `settling` names once it is due, and
/// publish what that settled.
///
/// Returns at once: the settle runs in a task of its own, because the call that started the
/// detection has to answer while that detection is still running. It exists for the client
/// driven by these events — such a client never calls this service again, so without it the
/// conclusion's event would wait for a status read that never comes.
///
/// WHEN the conclusion is due is read off the detection itself rather than recomputed here;
/// a due instant that moves while the task waits is waited for again, at most
/// [`MAX_SETTLE_WAITS`] times, so the task cannot be kept alive indefinitely by a moving
/// target.
///
/// It materialises exactly the transition the clock has already made due, through the same
/// one settle path a status read uses, so what this writes is what a read of the same
/// instant would have written; a conclusion a reader already settled is left alone, and each
/// event is published exactly once whichever got there first.
pub fn settle_when_due<Eng: Send + Sync + 'static>(
    engine: Arc<Eng>,
    callback: EmbeddedEmulator,
    settling: SettlingDetection,
) {
    if super::service_state(&*engine).is_none() {
        return;
    }
    let Some(slot) = TaskSlot::claim() else {
        ::tracing::warn!(
            "{MAX_TASKS_IN_FLIGHT} CloudFormation event tasks are already in flight; this \
             drift detection's conclusion will be settled by the next read of it"
        );
        return;
    };
    ::tokio::spawn(async move {
        let _slot = slot;
        for _ in 0..MAX_SETTLE_WAITS {
            let Some(due) = conclusion_due(&*engine, &settling.detection_id) else {
                // The detection has concluded — settled by this task or by a reader that got
                // there first — so there is nothing left to wait for.
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
                    store.claim_drift_events(&settling.region)
                }
            };
            publish(&callback, &settling.region, &claimed).await;
        }
    });
}

/// The instant the conclusion of the detection `detection_id` names is due, or `None` for
/// one this store no longer holds in flight.
fn conclusion_due<Eng: Send + Sync + 'static>(
    engine: &Eng,
    detection_id: &str,
) -> Option<SystemTime> {
    let state = super::service_state(engine)?;
    let store = state.lock();
    let pending = store.drift_detection(detection_id)?.pending.as_ref()?;
    Some(pending.due_at)
}

/// Publish `events` onto the default bus of `region`.
async fn publish(callback: &EmbeddedEmulator, region: &str, events: &[DriftDetectionEvent]) {
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
                .detail_type(DRIFT_DETECTION_STATUS_CHANGE)
                // The event's `time` is WHEN the detection reached that status:
                // `PutEventsRequestEntry$Time` is "the time stamp of the event, per RFC3339.
                // If no time stamp is provided, the time stamp of the PutEvents call is
                // used."
                .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                    epoch_millis(event.at),
                ))
                // "The Amazon Web Services resources, identified by Amazon Resource Name
                // (ARN), which the event primarily concerns": the stack the detection ran
                // against.
                .resources(&event.stack_id)
                .detail(event.detail().to_string())
                .build();
            request = request.entries(entry);
        }
        if let Err(error) = request.send().await {
            ::tracing::warn!(
                region = %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a CloudFormation drift detection status change could not be published to \
                 EventBridge"
            );
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A detection's event states the detection, the stack and the status it reached — with
    /// the two sentinels while it is still running, and the verdict and count once it has
    /// concluded.
    #[test]
    fn the_detail_states_the_record_the_status_was_read_from() {
        let stack_id = "arn:aws:cloudformation:us-east-1:123456789012:stack/probe/abc";
        for (case, detection, expected) in [
            (
                "a detection that has not concluded",
                super::super::StoredDriftDetection {
                    detection_id: "detection-1".to_string(),
                    region: "us-east-1".to_string(),
                    stack_id: stack_id.to_string(),
                    initiated_at: UNIX_EPOCH,
                    detection_status: super::super::drift_detection_status::DETECTION_IN_PROGRESS
                        .to_string(),
                    detection_status_reason: None,
                    stack_drift_status: None,
                    drifted_resource_count: None,
                    pending: None,
                    pending_events: Vec::new(),
                },
                ::serde_json::json!({
                    "stack-id": stack_id,
                    "stack-drift-detection-id": "detection-1",
                    "status-details": {
                        "detection-status": "DETECTION_IN_PROGRESS",
                        "stack-drift-status": "",
                    },
                    "drift-detection-details": { "drifted-stack-resource-count": -1 },
                }),
            ),
            (
                "a detection that concluded with a drifted resource",
                super::super::StoredDriftDetection {
                    detection_id: "detection-2".to_string(),
                    region: "us-east-1".to_string(),
                    stack_id: stack_id.to_string(),
                    initiated_at: UNIX_EPOCH,
                    detection_status: super::super::drift_detection_status::DETECTION_COMPLETE
                        .to_string(),
                    detection_status_reason: None,
                    stack_drift_status: Some(super::super::DRIFT_STATUS_DRIFTED.to_string()),
                    drifted_resource_count: Some(1),
                    pending: None,
                    pending_events: Vec::new(),
                },
                ::serde_json::json!({
                    "stack-id": stack_id,
                    "stack-drift-detection-id": "detection-2",
                    "status-details": {
                        "detection-status": "DETECTION_COMPLETE",
                        "stack-drift-status": "DRIFTED",
                    },
                    "drift-detection-details": { "drifted-stack-resource-count": 1 },
                }),
            ),
        ] {
            let mut detection = detection;
            detection.queue_event(UNIX_EPOCH);
            let queued = detection
                .pending_events
                .first()
                .unwrap_or_else(|| panic!("case {case:?}: the event must be queued"));
            let actual = queued.detail();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected}, got {actual}"
            );
        }
    }

    /// The queue one detection holds is bounded, oldest reclaimed first.
    #[test]
    fn the_queue_of_one_detection_is_bounded() {
        let mut detection = super::super::StoredDriftDetection {
            detection_id: "detection-3".to_string(),
            region: "us-east-1".to_string(),
            stack_id: "arn:aws:cloudformation:us-east-1:123456789012:stack/probe/abc".to_string(),
            initiated_at: UNIX_EPOCH,
            detection_status: super::super::drift_detection_status::DETECTION_IN_PROGRESS
                .to_string(),
            detection_status_reason: None,
            stack_drift_status: None,
            drifted_resource_count: None,
            pending: None,
            pending_events: Vec::new(),
        };
        for queued in 0..(MAX_PENDING_EVENTS + 3) {
            detection.queue_event(UNIX_EPOCH + Duration::from_secs(queued as u64));
            let held = detection.pending_events.len();
            assert!(
                held <= MAX_PENDING_EVENTS,
                "case {queued:?}: at most {MAX_PENDING_EVENTS} events are held, got {held}"
            );
        }
        let oldest = detection
            .pending_events
            .first()
            .map(|event| event.at)
            .unwrap_or(UNIX_EPOCH);
        let expected = UNIX_EPOCH + Duration::from_secs(3);
        assert_eq!(
            oldest, expected,
            "the oldest events are reclaimed first: expected the queue to start at \
             {expected:?}, got {oldest:?}"
        );
    }
}
