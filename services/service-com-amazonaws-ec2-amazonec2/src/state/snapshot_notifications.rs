// SPDX-License-Identifier: CC0-1.0

//! The event an EBS snapshot's OUTCOME is published as.
//!
//! A snapshot action is not finished when the API answers it: the answer reports a `pending`
//! snapshot, and what becomes of that snapshot — it holds the source's data, or it cannot be
//! written at all — is settled afterwards. Amazon EBS publishes that OUTCOME, once per
//! snapshot it was asked for, onto the account's default EventBridge bus under the `source`
//! this service owns ([`super::EC2_EVENT_SOURCE`]) and the `detail-type` of a snapshot
//! notification ([`SNAPSHOT_NOTIFICATION_DETAIL_TYPE`]), naming the snapshot in the
//! envelope's `resources` and carrying in the event `detail` which action it is the outcome
//! of ([`CREATE_SNAPSHOT_EVENT`], [`CREATE_SNAPSHOTS_EVENT`], [`COPY_SNAPSHOT_EVENT`]), the
//! `result` that action reached, the `cause` of a result that failed, the `state` the
//! snapshot settled in, and the snapshot and its source. So a client that drives automation
//! off a snapshot's fate — a backup pipeline, an alarm, a cleanup job — learns that a
//! snapshot failed without polling EC2 for data that will never arrive, and the ASYNCHRONOUS
//! failure of a copy (a call EC2 accepts, whose source it resolves afterwards) is observable
//! at all.
//!
//! The notification is built from the STORED record ([`SnapshotNotification::of`]), so every
//! value in it is one `DescribeSnapshots` reports, and it is published WHEN THE OUTCOME FALLS
//! DUE ([`SnapshotNotification::publish_when_due`]) — the action itself answers the `pending`
//! snapshot it accepted and never waits for the publication, and nothing holds a store lock
//! across it. A consumer that acts on the event therefore finds the snapshot as the event
//! reports it, rather than still `pending`.
//!
//! * `state` is the `SnapshotState` the record settles in
//!   ([`super::SnapshotRecord::state_at`]): the state a later read of the snapshot reports,
//!   so the event and the API cannot say two different things about one snapshot.
//! * `result` is whether that state is the one the action set out to reach
//!   ([`SNAPSHOT_RESULT_SUCCEEDED`]) or not ([`SNAPSHOT_RESULT_FAILED`]).
//! * `cause` is why a failed outcome failed, in the words of the service
//!   ([`super::SnapshotRecord::failure`]), and EMPTY for an outcome that did not fail,
//!   nothing having caused it.
//! * `snapshot_id` and `source` name the snapshot the outcome is about and what it was made
//!   from — the source volume of a create, the source snapshot of a copy — each as the ARN
//!   the account knows that resource by.
//! * `startTime` and `endTime` are when the action was initiated and when its outcome fell
//!   due, so a consumer reads how long the snapshot took; both are read off the record, so
//!   `startTime` never follows `endTime`.
//! * the event's `time` is the instant the outcome falls DUE
//!   ([`super::SnapshotRecord::completed_at`]), not the instant the publication was made, so
//!   a consumer reads when the snapshot settled.
//! * `request-id` identifies the call the outcome belongs to, in the form most AWS services
//!   report one and minted by the ONE mint this tree has for it
//!   (`service_common::query_error_envelope::new_request_id`), so two outcomes are told apart
//!   by it.
//!
//! EventBridge is reached the way every emulated service reaches another: the AWS SDK over
//! the framework's in-process callback, through the ONE client construction
//! [`::service_common::eventbridge_client`] holds, so nothing here opens a socket or names
//! another service's crate. The only thing the address is built from is the Region the action
//! was signed for, so no part of a request is a destination. Publication is BEST EFFORT — an
//! emulator with no EventBridge mounted has nowhere to publish — so a failure is traced and
//! the operation's own answer stands.

use ::std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ::smithy_server_runtime::EmbeddedEmulator;

use super::{DEFAULT_EVENT_BUS, EC2_EVENT_SOURCE, SnapshotRecord};

/// The `detail-type` every notification about one EBS SNAPSHOT is published under — the
/// whole class of them, which is what a rule written for a snapshot's fate matches on.
pub const SNAPSHOT_NOTIFICATION_DETAIL_TYPE: &str = "EBS Snapshot Notification";

/// The `event` member of a notification about a `CreateSnapshot`: which action the outcome
/// is the outcome of, spelled as the API action it names.
pub const CREATE_SNAPSHOT_EVENT: &str = "createSnapshot";

/// The `event` member of a notification about one snapshot of a `CreateSnapshots` set.
pub const CREATE_SNAPSHOTS_EVENT: &str = "createSnapshots";

/// The `event` member of a notification about a `CopySnapshot`.
pub const COPY_SNAPSHOT_EVENT: &str = "copySnapshot";

/// The `result` of an action whose snapshot holds the source's data: the outcome it set out
/// to reach.
pub const SNAPSHOT_RESULT_SUCCEEDED: &str = "succeeded";

/// The `result` of an action whose snapshot cannot hold that data at all.
pub const SNAPSHOT_RESULT_FAILED: &str = "failed";

/// How many snapshot outcomes this process publishes AT ONCE.
///
/// Each publication waits out the span the store owes the snapshot and then makes one
/// in-process call, so the work in flight grows with the rate snapshot actions arrive at
/// unless it is capped. The figure is well above the outcomes one caller's actions can have
/// in flight within a single transition span and far below the snapshots this store holds at
/// all ([`super::MAX_STORED_SNAPSHOTS`]), so an ordinary client never meets it and a flood of
/// creates cannot grow this process without limit — the same cap, and for the same reason,
/// as the volume outcomes beside them ([`super::MAX_CONCURRENT_VOLUME_PUBLICATIONS`]).
pub const MAX_CONCURRENT_SNAPSHOT_PUBLICATIONS: usize = 64;

/// The slots those publications take, one each, held until the publication ends.
static PUBLICATION_SLOTS: ::tokio::sync::Semaphore =
    ::tokio::sync::Semaphore::const_new(MAX_CONCURRENT_SNAPSHOT_PUBLICATIONS);

/// The ARN the account knows the snapshot `snapshot_id` of `region` by.
///
/// The one place a snapshot's ARN is built, so the event's `resources` and its `detail`
/// cannot name one snapshot two ways. It is the form every ARN this emulator emits takes
/// (`::engine::emulator_arn`), with the resource type a snapshot is addressed under — the
/// same `snapshot/<id>` the service's own authorization layer judges one by.
#[must_use]
pub fn snapshot_arn(region: &str, snapshot_id: &str) -> String {
    ::engine::emulator_arn("ec2", region, &format!("snapshot/{snapshot_id}"))
}

/// Why a snapshot COPY whose source could not be resolved failed, in AWS's own words for
/// that failure: the detail the failed copy reports as its `StateMessage` ("if a snapshot
/// copy operation fails … this field displays error state details to help you diagnose why
/// the error occurred", `Snapshot$StateMessage`) and the `cause` its published outcome
/// carries ([`super::SnapshotNotification`]).
///
/// The sentence interpolates nothing — it names neither the id nor the Region — which is how
/// AWS words it: the recorded copy of a source the account no longer held reported exactly
/// this text. A copy resolves its source AFTER the call is answered, the source living in the
/// Region `SourceRegion` names, so a source that is not there is not a refusal of the request
/// but the cause of the copy's own failure.
pub const COPY_SOURCE_SNAPSHOT_NOT_FOUND_MESSAGE: &str = "Source snapshot is not found";

/// One EBS snapshot notification, rendered from the stored snapshot and waiting to be
/// published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotNotification {
    /// The Region the event is published in: the Region the action was signed for.
    pub region: String,
    /// The snapshot's ARN, which the envelope carries in `resources`: the event names the
    /// resource it is about, which is what a rule filtering on `resources` reads.
    pub snapshot_arn: String,
    /// The ARN of what the snapshot was made FROM — the source volume of a create, the
    /// source snapshot of a copy — and empty for a source this service cannot name.
    pub source_arn: String,
    /// WHEN the outcome falls due, in epoch milliseconds.
    pub at_millis: i64,
    /// WHEN the action that started this snapshot was initiated, in epoch milliseconds: the
    /// instant the record reports as the snapshot's `StartTime`.
    pub started_millis: i64,
    /// How long the clock still owes before that instant.
    pub due_in: Duration,
    /// The action this is the outcome of ([`CREATE_SNAPSHOT_EVENT`] and the two beside it).
    pub event: &'static str,
    /// The `SnapshotState` the snapshot settles in.
    pub state: &'static str,
    /// Whether that state is the one the action set out to reach.
    pub result: &'static str,
    /// Why a failed outcome failed, and empty for one that did not fail.
    pub cause: String,
    /// The id of the call this is the outcome of.
    ///
    /// The id the ANSWER to that call carries is written by the response renderer after the
    /// handler has returned (`vendor/smithy-schema-server`'s ec2Query renderer), which no
    /// handler can reach, so the event names the call with an id of its own in the same form.
    pub request_id: String,
}

impl SnapshotNotification {
    /// The notification for the snapshot `record` an `event` action has just stored, as of
    /// `now`, made from the resource `source_arn` names.
    ///
    /// The state, the result and the cause are read off the RECORD rather than off the
    /// request: the store settles what becomes of the snapshot
    /// ([`SnapshotRecord::failure`]), so the outcome published is the outcome a later
    /// `DescribeSnapshots` reports.
    #[must_use]
    pub fn of(
        event: &'static str,
        record: &SnapshotRecord,
        now: Instant,
        region: &str,
        source_arn: &str,
    ) -> Self {
        // The state the snapshot settles in rather than the one it stands in now: the record
        // is `pending` until the clock-owed move falls due, and this event is published at
        // that instant.
        let state = record.state_at(record.completed_at);
        let cause = record.failure.clone().unwrap_or_default();
        let result = if record.failure.is_some() {
            SNAPSHOT_RESULT_FAILED
        } else {
            SNAPSHOT_RESULT_SUCCEEDED
        };
        Self {
            region: region.to_string(),
            snapshot_arn: snapshot_arn(
                region,
                record.view.snapshot_id.as_deref().unwrap_or_default(),
            ),
            source_arn: source_arn.to_string(),
            at_millis: outcome_millis(record, now),
            // "The time stamp when the snapshot was initiated" (`Snapshot$StartTime`), read
            // off the record so the event's `startTime` is the one the API reports.
            started_millis: record
                .view
                .start_time
                .as_ref()
                .and_then(|started| started.to_millis().ok())
                .unwrap_or_default(),
            due_in: record.completed_at.saturating_duration_since(now),
            event,
            state,
            result,
            cause,
            request_id: ::service_common::query_error_envelope::new_request_id(),
        }
    }

    /// The event's `detail`: which action, which result, which state, why, of what, and over
    /// which interval.
    #[must_use]
    pub fn detail(&self) -> ::serde_json::Value {
        ::serde_json::json!({
            "event": self.event,
            "result": self.result,
            "state": self.state,
            "cause": self.cause,
            "request-id": self.request_id,
            "snapshot_id": self.snapshot_arn,
            "source": self.source_arn,
            // The interval the action ran over. The start is held to the outcome's instant,
            // so a wall clock adjusted backwards between the write and the publication
            // cannot make a snapshot end before it started.
            "startTime": event_timestamp(self.started_millis.min(self.at_millis)),
            "endTime": event_timestamp(self.at_millis),
        })
    }

    /// Publish this notification once the outcome it reports has FALLEN DUE, without the
    /// call that started the snapshot waiting for it.
    ///
    /// A snapshot is answered `pending` and settles a transition span later, so the outcome
    /// this event reports is not a fact yet when the action is answered: publishing it then
    /// would tell a consumer that a snapshot had completed, or failed, while a read of it
    /// still said `pending`. The publication is therefore made from a task that resumes at
    /// the due instant — because no later call of this account can carry it: a consumer of
    /// the bus makes none. That task holds no lock and nothing of the store, it makes exactly
    /// one publication and ends, and there is at most one of it per snapshot this service was
    /// asked for.
    ///
    /// The tasks in flight are CAPPED at [`MAX_CONCURRENT_SNAPSHOT_PUBLICATIONS`]: a slot is
    /// taken before one is spawned and held inside it until it ends, on every path, so the
    /// work and the in-process calls this makes cannot grow with the rate actions arrive at.
    /// An outcome that finds no slot is not published, and says so in the log, which is the
    /// same best-effort answer as an emulator with no EventBridge to publish to.
    ///
    /// Call it with NO store lock held. Nothing about the answer to the action changes,
    /// whatever becomes of the publication, for the reason the module docs give.
    pub fn publish_when_due(self, callback: EmbeddedEmulator) {
        let Ok(slot) = PUBLICATION_SLOTS.try_acquire() else {
            ::tracing::warn!(
                snapshot = %self.snapshot_arn,
                "this process is already publishing {MAX_CONCURRENT_SNAPSHOT_PUBLICATIONS} \
                 snapshot outcomes, so this one is not published"
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

    /// Publish this notification onto the account's default event bus in the snapshot's
    /// Region.
    ///
    /// Call it with NO store lock held. Nothing about the answer changes what the caller of
    /// the operation is told, for the reason the module docs give.
    pub async fn publish(self, callback: &EmbeddedEmulator) {
        let client = ::service_common::eventbridge_client::eventbridge_client(callback, &self.region);
        let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
            .event_bus_name(DEFAULT_EVENT_BUS)
            .source(EC2_EVENT_SOURCE)
            .detail_type(SNAPSHOT_NOTIFICATION_DETAIL_TYPE)
            // The event's `time` is WHEN the outcome falls due, not when it was published:
            // `PutEventsRequestEntry$Time` is "the time stamp of the event, per RFC3339. If
            // no time stamp is provided, the time stamp of the PutEvents call is used."
            .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                self.at_millis,
            ))
            .resources(&self.snapshot_arn)
            .detail(self.detail().to_string())
            .build();
        match client.put_events().entries(entry).send().await {
            Ok(answer) if answer.failed_entry_count() > 0 => {
                ::tracing::warn!(
                    snapshot = %self.snapshot_arn,
                    region = %self.region,
                    "an EBS snapshot notification was not accepted by the default event bus"
                );
            }
            Ok(_) => {}
            Err(error) => {
                ::tracing::warn!(
                    snapshot = %self.snapshot_arn,
                    region = %self.region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "an EBS snapshot notification could not be published"
                );
            }
        }
    }
}

/// One instant as an event timestamp: RFC3339 at SECOND precision, the form AWS stamps a
/// notification's `startTime` and `endTime` in (`2019-05-01T19:04:21Z`), so the two read as
/// one kind of value and compare as one.
///
/// Shared with the completion of a multi-volume snapshot SET
/// ([`super::MultiVolumeSnapshotsCompletion`]), which stamps the same two members of the same
/// class of event, so one notification's interval and another's are the same kind of value.
#[must_use]
pub fn event_timestamp(millis: i64) -> String {
    ::aws_smithy_types::DateTime::from_secs(millis.div_euclid(1_000))
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// The wall-clock instant a move or outcome that is `due_in` away falls due, in epoch
/// milliseconds.
///
/// This store keeps its due instants on the monotonic clock it moves every transition on
/// ([`Instant`]), which has no epoch of its own, so the span still owed is added to the wall
/// clock. A clock that cannot be read against the epoch at all leaves the event to be stamped
/// with no time of this service's choosing.
///
/// The ONE computation of an event's `time` for every notification this service publishes —
/// a volume's outcome, a snapshot's, an instance's move and an AMI's
/// ([`super::VolumeNotification`], [`SnapshotNotification`],
/// [`super::InstanceStateNotification`], [`super::ImageStateNotification`]) — so no two
/// classes of event can stamp the same instant two ways. `Duration::ZERO` is an outcome that
/// is already a fact, which stamps the event with now.
#[must_use]
pub fn epoch_millis_in(due_in: Duration) -> i64 {
    (SystemTime::now() + due_in)
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since_epoch| i64::try_from(since_epoch.as_millis()).ok())
        .unwrap_or_default()
}

/// The wall-clock instant a snapshot's outcome falls due, in epoch milliseconds.
///
/// The store keeps that instant on the monotonic clock it moves every transition on
/// ([`SnapshotRecord::completed_at`]), so the span still owed at `now` is what the one
/// computation of an event's time ([`epoch_millis_in`]) is asked for.
fn outcome_millis(record: &SnapshotRecord, now: Instant) -> i64 {
    epoch_millis_in(record.completed_at.saturating_duration_since(now))
}
