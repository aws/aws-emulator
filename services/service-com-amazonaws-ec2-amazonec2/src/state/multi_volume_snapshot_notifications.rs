// SPDX-License-Identifier: CC0-1.0

//! The event the COMPLETION of a multi-volume snapshot set is published as.
//!
//! `CreateSnapshots` takes one point-in-time snapshot of every Amazon EBS volume of an
//! instance, and the set is not finished when the call is answered: each snapshot is handed
//! back `pending` and settles afterwards. Besides the outcome of each snapshot on its own
//! ([`super::SnapshotNotification`]), Amazon EBS publishes ONE event for the SET once its
//! snapshots have completed — onto the account's default event bus
//! ([`super::DEFAULT_EVENT_BUS`]) under the `source` this service owns
//! ([`super::EC2_EVENT_SOURCE`]) and the `detail-type` of a multi-volume completion
//! ([`MULTI_VOLUME_SNAPSHOTS_COMPLETION_DETAIL_TYPE`]) — naming every snapshot of the set in
//! the envelope's `resources` and listing, in the event `detail`, each snapshot with the
//! volume it was taken of, the state it settled in and the interval it ran over. So a client
//! that promotes a backup set only once the WHOLE set is safe has one signal to wait for
//! instead of joining the snapshots' own events itself.
//!
//! The completion is built from the snapshots' own published outcomes
//! ([`MultiVolumeSnapshotsCompletion::of`]), so the set and each snapshot of it cannot say
//! two different things about one snapshot, and it is published when the LAST of those
//! outcomes falls due ([`MultiVolumeSnapshotsCompletion::publish_when_due`]): the set is
//! complete when every snapshot in it is, so the event reports an interval that ends with the
//! last snapshot and a `result` that fails where any snapshot of the set did.
//!
//! EventBridge is reached the way every emulated service reaches another: the AWS SDK over
//! the framework's in-process callback, through the ONE client construction
//! [`::service_common::eventbridge_client`] holds, so nothing here opens a socket or names
//! another service's crate. The only thing the address is built from is the Region the call
//! was signed for, so no part of a request is a destination. Publication is BEST EFFORT — an
//! emulator with no EventBridge mounted has nowhere to publish — so a failure is traced and
//! the operation's own answer stands.

use ::std::time::Duration;

use ::smithy_server_runtime::EmbeddedEmulator;

use super::{
    DEFAULT_EVENT_BUS, EC2_EVENT_SOURCE, SNAPSHOT_RESULT_FAILED, SNAPSHOT_RESULT_SUCCEEDED,
    SnapshotNotification, event_timestamp,
};

/// The `detail-type` the completion of one multi-volume snapshot set is published under —
/// the whole class of them, which is what a rule written for a backup set matches on.
pub const MULTI_VOLUME_SNAPSHOTS_COMPLETION_DETAIL_TYPE: &str =
    "EBS Multi-Volume Snapshots Completion Status";

/// How many set completions this process publishes AT ONCE.
///
/// Each publication waits out the span the store owes the set's last snapshot and then makes
/// one in-process call, so the work in flight grows with the rate `CreateSnapshots` calls
/// arrive at unless it is capped. The figure is well above the sets one caller's calls can
/// have in flight within a single transition span, and one publication is made per CALL
/// rather than per snapshot, so an ordinary client never meets it and a flood of calls cannot
/// grow this process without limit — the same cap, and for the same reason, as the snapshot
/// outcomes beside them ([`super::MAX_CONCURRENT_SNAPSHOT_PUBLICATIONS`]).
pub const MAX_CONCURRENT_SET_PUBLICATIONS: usize = 64;

/// The slots those publications take, one each, held until the publication ends.
static PUBLICATION_SLOTS: ::tokio::sync::Semaphore =
    ::tokio::sync::Semaphore::const_new(MAX_CONCURRENT_SET_PUBLICATIONS);

/// One snapshot of the set, as the completion event lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedSnapshot {
    /// The snapshot, as the ARN the account knows it by.
    pub snapshot_arn: String,
    /// The volume it was taken of, as the ARN the account knows that volume by.
    pub source_arn: String,
    /// The state the snapshot settled in.
    pub status: String,
    /// WHEN the action that started it was initiated, in epoch milliseconds.
    pub started_millis: i64,
    /// WHEN its outcome fell due, in epoch milliseconds.
    pub at_millis: i64,
}

/// The completion of one multi-volume snapshot set, rendered from the snapshots' own
/// outcomes and waiting to be published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiVolumeSnapshotsCompletion {
    /// The Region the event is published in: the Region the call was signed for.
    pub region: String,
    /// Every snapshot of the set, in the order the call created them.
    pub snapshots: Vec<CompletedSnapshot>,
    /// The action the set is the outcome of, spelled as the API action it names
    /// ([`super::CREATE_SNAPSHOTS_EVENT`]).
    pub event: &'static str,
    /// Whether every snapshot of the set reached the state it set out to.
    pub result: &'static str,
    /// Why a failed set failed — the first cause one of its snapshots reported — and empty
    /// for a set that did not fail, nothing having caused it.
    pub cause: String,
    /// WHEN the set was initiated, in epoch milliseconds: the earliest of its snapshots'
    /// start instants, which for one call's set is the one instant they share.
    pub started_millis: i64,
    /// WHEN the set is complete, in epoch milliseconds: the instant the LAST of its
    /// snapshots' outcomes falls due.
    pub at_millis: i64,
    /// How long the clock still owes before that instant.
    pub due_in: Duration,
    /// The id of the call this is the completion of.
    ///
    /// The id the ANSWER to that call carries is written by the response renderer after the
    /// handler has returned (`vendor/smithy-schema-server`'s ec2Query renderer), which no
    /// handler can reach, so the event names the call with an id of its own in the same form.
    pub request_id: String,
}

impl MultiVolumeSnapshotsCompletion {
    /// The completion of the set whose snapshots publish the outcomes `outcomes`, or `None`
    /// for a call that created no snapshot at all — there being no set to report the
    /// completion of.
    ///
    /// Every value is read off those outcomes rather than restated here, so the set's event
    /// and each snapshot's own event cannot disagree: the set is complete when its LAST
    /// snapshot is, it began when the first of them did, and it has failed where any snapshot
    /// of it did.
    #[must_use]
    pub fn of(outcomes: &[SnapshotNotification]) -> Option<Self> {
        let first = outcomes.first()?;
        let snapshots: Vec<CompletedSnapshot> = outcomes
            .iter()
            .map(|outcome| CompletedSnapshot {
                snapshot_arn: outcome.snapshot_arn.clone(),
                source_arn: outcome.source_arn.clone(),
                status: outcome.state.to_string(),
                started_millis: outcome.started_millis,
                at_millis: outcome.at_millis,
            })
            .collect();
        let failed = outcomes
            .iter()
            .find(|outcome| outcome.result == SNAPSHOT_RESULT_FAILED);
        Some(Self {
            region: first.region.clone(),
            event: first.event,
            result: if failed.is_some() {
                SNAPSHOT_RESULT_FAILED
            } else {
                SNAPSHOT_RESULT_SUCCEEDED
            },
            cause: failed.map_or_else(String::new, |outcome| outcome.cause.clone()),
            started_millis: outcomes
                .iter()
                .map(|outcome| outcome.started_millis)
                .min()
                .unwrap_or_default(),
            at_millis: outcomes
                .iter()
                .map(|outcome| outcome.at_millis)
                .max()
                .unwrap_or_default(),
            due_in: outcomes
                .iter()
                .map(|outcome| outcome.due_in)
                .max()
                .unwrap_or_default(),
            request_id: ::service_common::query_error_envelope::new_request_id(),
            snapshots,
        })
    }

    /// The event's `detail`: which action, which result, why, over which interval, and every
    /// snapshot of the set.
    #[must_use]
    pub fn detail(&self) -> ::serde_json::Value {
        let snapshots: Vec<::serde_json::Value> = self
            .snapshots
            .iter()
            .map(|snapshot| {
                ::serde_json::json!({
                    "snapshot_id": snapshot.snapshot_arn,
                    "source": snapshot.source_arn,
                    "status": snapshot.status,
                    // The interval that snapshot ran over, held to its own outcome's instant
                    // for the reason the set's interval below is.
                    "startTime": event_timestamp(snapshot.started_millis.min(snapshot.at_millis)),
                    "endTime": event_timestamp(snapshot.at_millis),
                })
            })
            .collect();
        ::serde_json::json!({
            "event": self.event,
            "result": self.result,
            "cause": self.cause,
            "request-id": self.request_id,
            // The interval the set ran over. The start is held to the completion's instant,
            // so a wall clock adjusted backwards between the write and the publication cannot
            // make a set end before it started.
            "startTime": event_timestamp(self.started_millis.min(self.at_millis)),
            "endTime": event_timestamp(self.at_millis),
            "snapshots": snapshots,
        })
    }

    /// Publish this completion once the LAST snapshot of the set has settled, without the
    /// call that created them waiting for it.
    ///
    /// The set is answered with `pending` snapshots and completes a transition span later, so
    /// the completion this event reports is not a fact yet when the call is answered:
    /// publishing it then would tell a consumer that a backup set was safe while a read of
    /// its snapshots still said `pending`. The publication is therefore made from a task that
    /// resumes at the due instant — because no later call of this account can carry it: a
    /// consumer of the bus makes none. That task holds no lock and nothing of the store, it
    /// makes exactly one publication and ends, and there is at most one of it per call this
    /// service served.
    ///
    /// The tasks in flight are CAPPED at [`MAX_CONCURRENT_SET_PUBLICATIONS`]: a slot is taken
    /// before one is spawned and held inside it until it ends, on every path, so the work and
    /// the in-process calls this makes cannot grow with the rate calls arrive at. A
    /// completion that finds no slot is not published, and says so in the log, which is the
    /// same best-effort answer as an emulator with no EventBridge to publish to.
    ///
    /// Call it with NO store lock held. Nothing about the answer to the call changes,
    /// whatever becomes of the publication, for the reason the module docs give.
    pub fn publish_when_due(self, callback: EmbeddedEmulator) {
        let Ok(slot) = PUBLICATION_SLOTS.try_acquire() else {
            ::tracing::warn!(
                snapshots = self.snapshots.len(),
                "this process is already publishing {MAX_CONCURRENT_SET_PUBLICATIONS} \
                 snapshot-set completions, so this one is not published"
            );
            return;
        };
        ::tokio::spawn(async move {
            // Held for the whole publication and given back when this task ends, however it
            // ends, so the cap above counts what is really in flight.
            let _slot = slot;
            if !self.due_in.is_zero() {
                ::tokio::time::sleep(self.due_in).await;
            }
            self.publish(&callback).await;
        });
    }

    /// Publish this completion onto the account's default event bus in the set's Region.
    ///
    /// Call it with NO store lock held. Nothing about the answer changes what the caller of
    /// the operation is told, for the reason the module docs give.
    pub async fn publish(self, callback: &EmbeddedEmulator) {
        let client =
            ::service_common::eventbridge_client::eventbridge_client(callback, &self.region);
        let mut entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
            .event_bus_name(DEFAULT_EVENT_BUS)
            .source(EC2_EVENT_SOURCE)
            .detail_type(MULTI_VOLUME_SNAPSHOTS_COMPLETION_DETAIL_TYPE)
            // The event's `time` is WHEN the set completes, not when it was published:
            // `PutEventsRequestEntry$Time` is "the time stamp of the event, per RFC3339. If
            // no time stamp is provided, the time stamp of the PutEvents call is used."
            .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                self.at_millis,
            ))
            .detail(self.detail().to_string());
        // The envelope names EVERY snapshot the set produced: the event is about the set, so
        // a rule filtering on `resources` reads each of them.
        for snapshot in &self.snapshots {
            entry = entry.resources(&snapshot.snapshot_arn);
        }
        match client.put_events().entries(entry.build()).send().await {
            Ok(answer) if answer.failed_entry_count() > 0 => {
                ::tracing::warn!(
                    snapshots = self.snapshots.len(),
                    region = %self.region,
                    "a multi-volume snapshot completion was not accepted by the default event \
                     bus"
                );
            }
            Ok(_) => {}
            Err(error) => {
                ::tracing::warn!(
                    snapshots = self.snapshots.len(),
                    region = %self.region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "a multi-volume snapshot completion could not be published"
                );
            }
        }
    }
}
