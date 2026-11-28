// SPDX-License-Identifier: CC0-1.0

//! The stored QUEUE record: what `CreateQueue` writes, what every queue read projects, and
//! the delivery state that makes a queue's records in flight, acknowledged, dead-letter or
//! visible again.
//!
//! A queue "delivers records from the stream to a queue consumer, tracks the delivery state of
//! each record, and redelivers any record that is not acknowledged before its visibility
//! timeout expires" (`CreateQueue`). Three things live here, because every queue operation
//! consults them:
//!
//! * the stored record itself ([`Queue`]) — the stream it delivers from, the position delivery
//!   begins at, its visibility timeout, its dead-letter configuration, and the instants its
//!   `CREATING` -> `ACTIVE`, `UPDATING` -> `ACTIVE` and `DELETING` -> gone transitions fall
//!   due ([`Queue::status`], [`Queue::is_held`]);
//! * the delivery state of one record of the stream inside one queue ([`QueueDelivery`]) —
//!   the receipt handle its current delivery was issued under, how many times it has been
//!   received without being acknowledged, and the instant it becomes visible again; and
//! * the one rule that decides which of those a receive serves, which is derived from the
//!   stored instants and counts rather than written by a reader
//!   ([`QueueDelivery::is_in_flight`], [`QueueDelivery::is_dead_letter`]).
//!
//! The records themselves are the STREAM's: a queue holds a cursor per shard and the delivery
//! state of the records it has drawn past that cursor ([`pending_for`]), so acknowledging a
//! record "does not affect the record in the data stream" (`AcknowledgeQueueRecords`) and
//! deleting a queue "does not delete the data stream the queue was created on"
//! (`DeleteQueue`).
//!
//! The parts of that live beside this file, each one concern: the delivery state of one record
//! ([`delivery`]), the store's own lookups and the one place a queue meets its stream's records
//! ([`store`]), and how every refusal a queue operation states is worded ([`refusals`]).

#[cfg(test)]
mod tests;

mod delivery;
mod refusals;
mod store;

pub use delivery::*;
pub use refusals::*;
pub use store::*;

use super::*;

/// A queue that is still being created, spelled as `QueueStatus` models it
/// (`smithy_query(query="shape",shape="QueueStatus")`).
pub const QUEUE_CREATING: &str = "CREATING";

/// A queue that is ready: "A queue delivers records while it is in the ACTIVE state"
/// (`ReceiveQueueRecords`).
pub const QUEUE_ACTIVE: &str = "ACTIVE";

/// A queue a configuration change has been accepted for: "Upon receiving an UpdateQueue
/// request, Amazon Kinesis Data Streams sets the QueueStatus to UPDATING, and sets it back to
/// ACTIVE when the update is complete" (`UpdateQueue`).
pub const QUEUE_UPDATING: &str = "UPDATING";

/// A queue a delete has been accepted for: "Upon receiving a DeleteQueue request, Amazon
/// Kinesis Data Streams sets the QueueStatus to DELETING. The queue is deleted when the
/// operation completes" (`DeleteQueue`).
pub const QUEUE_DELETING: &str = "DELETING";

/// The `QueueStartingPositionType` that delivers "records that arrive after you create the
/// queue. This is the default" (`QueueStartingPosition$Type`).
pub const QUEUE_STARTING_POSITION_LATEST: &str = "LATEST";

/// The `QueueStartingPositionType` that starts "at the last untrimmed record in the stream,
/// which is the oldest record the stream still retains" (`QueueStartingPosition$Type`).
pub const QUEUE_STARTING_POSITION_TRIM_HORIZON: &str = "TRIM_HORIZON";

/// The `QueueStartingPositionType` that delivers "records that arrive at or after a time stamp
/// that you provide" (`QueueStartingPosition$Type`).
pub const QUEUE_STARTING_POSITION_AT_TIMESTAMP: &str = "AT_TIMESTAMP";

/// The `TargetType` that enables dead-letter handling: "BUILT_IN enables it: when a record has
/// been received MaxReceiveCount times without being acknowledged, it becomes a dead-letter
/// record" (`DeadLetterConfiguration$TargetType`).
pub const TARGET_TYPE_BUILT_IN: &str = "BUILT_IN";

/// The `TargetType` that disables it: "NONE disables it, and an unacknowledged record
/// continues to be redelivered until it is acknowledged or its retention period ends"
/// (`DeadLetterConfiguration$TargetType`).
pub const TARGET_TYPE_NONE: &str = "NONE";

/// The visibility timeout a queue created without one carries: "If you omit this attribute
/// when you create a queue, the queue uses 30 seconds"
/// (`QueueAttributes$VisibilityTimeoutSeconds`). A DOCUMENTED duration, so it keeps the value
/// the doc states.
pub const DEFAULT_VISIBILITY_TIMEOUT_SECONDS: i32 = 30;

/// The receive count a queue created without a `DeadLetterConfiguration` carries: "A queue
/// that you create without DeadLetterConfiguration gets a MaxReceiveCount of 6"
/// (`DeadLetterConfiguration$MaxReceiveCount`).
pub const DEFAULT_MAX_RECEIVE_COUNT: i32 = 6;

/// How many records one receive returns when the request names no `Limit`: "The maximum number
/// of records to return... The default is 10" (`ReceiveQueueRecordsInput$Limit`).
pub const DEFAULT_RECEIVE_LIMIT: i32 = 10;

/// How long one receive waits for a record when the request names no `WaitTimeSeconds`: "The
/// duration, in seconds, that the call waits for a record to become visible before returning.
/// The default is 9" (`ReceiveQueueRecordsInput$WaitTimeSeconds`).
pub const DEFAULT_WAIT_TIME_SECONDS: i32 = 9;

/// How many records one receive returns at most: "Receives up to 10 records from the specified
/// queue" (`ReceiveQueueRecords`), which is also the ceiling the member's modelled range
/// declares (`ReceiveQueueRecordsInputLimit` is `range 1..10`). A caller-chosen count is
/// clamped to it server-side before it sizes anything.
pub const MAX_RECEIVE_LIMIT: i32 = 10;

/// How long one receive waits at most, in seconds: the ceiling the member's modelled range
/// declares (`ReceiveQueueRecordsWaitTimeSeconds` is `range 1..20`). A caller-chosen wait is
/// clamped to it server-side before the call waits at all.
pub const MAX_RECEIVE_WAIT_SECONDS: i32 = 20;

/// How long after the `CreateQueue` call the queue's `CREATING` -> `ACTIVE` transition falls
/// due, which is the span this emulator gives a stream's own activation
/// ([`ACTIVATION_DELAY`]). A CHOSEN span, not an AWS-documented duration: it only has to be
/// non-zero, so a caller that reads the queue straight after creating it still observes
/// `CREATING`. The instant it falls due is stored on the record and never slept on.
pub const QUEUE_ACTIVATION_DELAY: Duration = ACTIVATION_DELAY;

/// How long after an accepted `UpdateQueue` the queue's `UPDATING` -> `ACTIVE` transition
/// falls due. A CHOSEN span, like [`QUEUE_ACTIVATION_DELAY`], and stored rather than slept on.
pub const QUEUE_UPDATE_DELAY: Duration = ACTIVATION_DELAY;

/// How long after the `DeleteQueue` call the queue's removal falls due, taking it from
/// `DELETING` to gone. A CHOSEN span, like [`DELETION_DELAY`] on a stream, and stored rather
/// than slept on.
pub const QUEUE_DELETION_DELAY: Duration = DELETION_DELAY;

/// The most tags one queue carries: "A set of up to 50 key-value pairs to apply to the queue"
/// (`CreateQueueInput$Tags`), the same count a stream carries ([`MAX_STREAM_TAGS`]) and
/// narrower than the `TagMap` `@length` of 1..200 the boundary validator enforces.
pub const MAX_QUEUE_TAGS: usize = MAX_STREAM_TAGS;

/// How many queues one region's endpoint holds. The emulator's OWN ceiling, not a modelled
/// limit: the store grows by one record per accepted create, so the creates one caller can
/// make are bounded.
pub const MAX_QUEUES_PER_REGION: usize = 50;

/// How many queues this emulator holds across every region — the ceiling no request can
/// partition, since the region a create is counted against is the caller's to choose. The
/// emulator's OWN ceiling, not a modelled limit.
pub const MAX_QUEUES_TOTAL: usize = 100;

/// How many queue summaries `ListQueues` returns when the request names no `MaxResults`. The
/// member documents no default ("The maximum number of queue summaries to return in one
/// response", `ListQueuesInput$MaxResults`), so this page size is the emulator's own, inside
/// the member's modelled range of 1..10000.
pub const DEFAULT_LIST_QUEUES_LIMIT: i32 = 100;

/// How often a receive that is waiting for a record looks again, while the wait the request
/// asked for has not elapsed: "Use WaitTimeSeconds to control how long the call waits for a
/// record to become visible before returning. If a record becomes visible sooner, the call
/// returns sooner" (`ReceiveQueueRecords`). The EMULATOR's own cadence, small enough that
/// "sooner" is observable; the wait itself is the caller's.
pub const RECEIVE_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How many delivery records one queue tracks at a time: the queue draws records from the
/// stream as its consumer receives them, and this bounds the store a queue occupies while its
/// consumer is behind. The emulator's OWN ceiling, not a modelled limit; a queue that has
/// reached it draws the next records once the ones it holds are acknowledged.
pub const MAX_TRACKED_RECORDS_PER_QUEUE: usize = 10_000;

/// How many bytes of the digest a `RecordId` carries, hex-encoded — 32 lowercase hexadecimal
/// characters, inside the member's modelled `@length` of 1..64 and its `^[a-z0-9]+$` pattern.
const RECORD_ID_DIGEST_BYTES: usize = 16;

/// Which of a queue's records an operation addresses: the queue's own receive path, or the
/// dead-letter path the four operations that "use the same queue ARN" (`CreateQueue`) reach
/// the same records through once they have reached the queue's receive ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueRecordPath {
    /// What `ReceiveQueueRecords` serves and `AcknowledgeQueueRecords` /
    /// `UpdateQueueRecordsVisibility` address.
    Queue,
    /// What `ReceiveDeadLetterQueueRecords` serves and
    /// `AcknowledgeDeadLetterQueueRecords` / `UpdateDeadLetterQueueRecordsVisibility` /
    /// `RestoreDeadLetterQueueRecords` address.
    DeadLetter,
}

/// What ONE receive out of a queue asks for: which of the queue's records it serves, how many
/// at once, how long they are then in flight, and how many delivery records the queue may hold
/// while it is served.
///
/// The operation that drives the growth is the one that states its bounds, so the three
/// ceilings a receive is held to travel together from the handler that resolved them
/// ([`crate::ops::receive_queue_records`]) to the one place that applies them
/// ([`ComAmazonawsKinesisKinesis20131202State::receive_from_queue`]).
#[derive(Debug, Clone, Copy)]
pub struct QueueReceive {
    /// Which of the queue's records it serves.
    pub path: QueueRecordPath,
    /// The most records it returns, clamped by the caller to the ceiling the `Limit` member's
    /// modelled range declares ([`MAX_RECEIVE_LIMIT`]).
    pub limit: usize,
    /// How long the records it serves are then in flight, when the request named a timeout of
    /// its own: "If you do not specify this value, the queue's visibility timeout is used"
    /// (`ReceiveQueueRecordsInput$VisibilityTimeoutSeconds`).
    pub visibility_timeout_seconds: Option<i32>,
    /// The most delivery records the queue may hold at once while this receive draws from its
    /// stream ([`MAX_TRACKED_RECORDS_PER_QUEUE`]).
    pub tracked_ceiling: usize,
}

/// Why a receive served no record of the queue it named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueReceiveFailure {
    /// This account and region hold no queue under that ARN.
    NotFound,
    /// The queue is held, but is not in a state that delivers records: the stored queue's name
    /// and the state it is in instead ([`Queue::delivers`]).
    NotDelivering { name: String, status: String },
}

/// One stored Kinesis queue: the authoritative record every queue operation reads.
/// `CreateQueue` inserts it and `DescribeQueue` projects it onto its output shape.
#[derive(Debug, Clone)]
pub struct Queue {
    /// The `QueueName` the create carried, which is half of this record's key: "The name is
    /// scoped to the Amazon Web Services account and Region used by the request"
    /// (`CreateQueueInput$QueueName`).
    pub name: String,
    /// The region the creating request was signed for.
    pub region: String,
    /// The `QueueARN`, as [`queue_arn`] builds it: "The resource identifier in the ARN is
    /// generated when the queue is created and is not derived from QueueName"
    /// (`QueueDescription$QueueARN`).
    pub arn: String,
    /// When `CreateQueue` minted the queue.
    pub created_at: SystemTime,
    /// When the queue's `CREATING` -> `ACTIVE` transition falls due.
    pub active_at: SystemTime,
    /// When the queue's `UPDATING` -> `ACTIVE` transition falls due, once a configuration
    /// change has been accepted for it. `None` on a queue no change has been accepted for.
    pub updating_until: Option<SystemTime>,
    /// When the queue's removal falls due, set by the `DeleteQueue` that accepted it. `None`
    /// on a queue nothing has deleted.
    pub deleted_at: Option<SystemTime>,
    /// The ARN of the one data stream the queue delivers records from: "The list must contain
    /// exactly one entry, because a queue delivers records from a single data stream"
    /// (`CreateQueueInput$StreamConfigurationList`).
    pub stream_arn: String,
    /// That stream's creation instant, which a summary reports beside its ARN
    /// (`QueueStreamIdentifier$StreamCreationTimestamp`) and a listing filter may pin
    /// (`QueueStreamFilter$StreamCreationTimestamp`).
    pub stream_created_at: SystemTime,
    /// The `QueueStartingPositionType` delivery begins at.
    pub starting_position_type: String,
    /// The instant `AT_TIMESTAMP` begins at, as the create carried it. `None` for the two
    /// positions that take none.
    pub starting_position_at: Option<SystemTime>,
    /// `QueueAttributes$VisibilityTimeoutSeconds` — the duration a received record is in
    /// flight for, as the create or the last accepted update resolved it.
    pub visibility_timeout_seconds: i32,
    /// `DeadLetterConfiguration$MaxReceiveCount` when dead-letter handling is enabled, `None`
    /// when the configuration turned it off (`TargetType` `NONE`).
    pub max_receive_count: Option<i32>,
    /// The tags the create carried.
    pub tags: HashMap<String, String>,
    /// How far this queue has drawn each shard of its stream: the sequence number of the last
    /// record it took into [`Self::tracked`], keyed by shard id. A record at or before the
    /// cursor is already tracked, so no later receive draws it twice.
    pub cursor: ::std::collections::BTreeMap<String, String>,
    /// The delivery state of every record this queue has drawn and not yet acknowledged,
    /// oldest first.
    pub tracked: Vec<QueueDelivery>,
}

impl Queue {
    /// Whether this account and region still hold the queue at `now`: a queue a delete armed
    /// is held, in `DELETING`, until the removal that delete owes falls due, and names no
    /// resource from then on. Derived from the stored instant, so a reader reaches the same
    /// answer as a writer without either of them having to prune the store.
    pub fn is_held(&self, now: SystemTime) -> bool {
        self.deleted_at.is_none_or(|due| now < due)
    }

    /// The queue's status at `now`: `DELETING` once a delete has been accepted, `CREATING`
    /// until the transition the create armed falls due, `UPDATING` until the one an accepted
    /// change armed falls due, `ACTIVE` from then on. Derived from the stored instants, so two
    /// reads at the same instant agree and no read has to write.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if self.deleted_at.is_some() {
            QUEUE_DELETING
        } else if now < self.active_at {
            QUEUE_CREATING
        } else if self.updating_until.is_some_and(|due| now < due) {
            QUEUE_UPDATING
        } else {
            QUEUE_ACTIVE
        }
    }

    /// Whether the queue delivers records at `now`: "A queue delivers records while it is in
    /// the ACTIVE state" (`ReceiveQueueRecords`), and "You can continue to receive and
    /// acknowledge records while the queue is UPDATING" (`UpdateQueue`), so the two states a
    /// queue delivers in are those — never `CREATING`, and never `DELETING`, where "A queue in
    /// the DELETING state does not deliver records" (`DeleteQueue`).
    pub fn delivers(&self, now: SystemTime) -> bool {
        matches!(self.status(now), QUEUE_ACTIVE | QUEUE_UPDATING)
    }

    /// How long a record this queue hands out stays in flight.
    pub fn visibility_timeout(&self) -> Duration {
        visibility_timeout(self.visibility_timeout_seconds)
    }

    /// The earliest arrival instant this queue delivers a record from, given the stream's
    /// `trim_horizon` — "the earliest time the stream can be read from"
    /// (`QueueStartingPosition$Timestamp`).
    ///
    /// ONE rule for the three positions: `LATEST` delivers "records that arrive after you
    /// create the queue", `TRIM_HORIZON` starts "at the last untrimmed record in the stream",
    /// and `AT_TIMESTAMP` at "the timestamp you provided or the earliest time the stream can be
    /// read from, whichever is later" — so each is its own configured instant raised to the
    /// trim horizon.
    pub fn effective_start(&self, trim_horizon: SystemTime) -> SystemTime {
        let configured = match self.starting_position_type.as_str() {
            QUEUE_STARTING_POSITION_TRIM_HORIZON => trim_horizon,
            QUEUE_STARTING_POSITION_AT_TIMESTAMP => {
                self.starting_position_at.unwrap_or(self.created_at)
            }
            // `LATEST`, which is also the position a create that named none takes.
            _ => self.created_at,
        };
        configured.max(trim_horizon)
    }

    /// The queue's starting position as a read reports it, given the stream's `trim_horizon`.
    ///
    /// "While the queue is CREATING, the response echoes what you configured: for
    /// AT_TIMESTAMP, the timestamp you provided, and for TRIM_HORIZON and LATEST, this member
    /// is absent because the effective position is not yet determined. Once the queue is
    /// ACTIVE, the response always includes this member" (`QueueStartingPosition$Timestamp`),
    /// where the instant reported is the effective one ([`Self::effective_start`]).
    pub fn reported_starting_position(
        &self,
        now: SystemTime,
        trim_horizon: Option<SystemTime>,
    ) -> QueueStartingPosition {
        let creating = self.status(now) == QUEUE_CREATING;
        let reported = match (creating, trim_horizon) {
            (true, _) => self.starting_position_at,
            (false, Some(horizon)) => Some(self.effective_start(horizon)),
            // The stream the queue delivers from is no longer held, so there is no horizon to
            // raise the configured instant to: what the create configured is what is reported.
            (false, None) => Some(self.starting_position_at.unwrap_or(self.created_at)),
        };
        QueueStartingPosition {
            r#type: self.starting_position_type.clone(),
            timestamp: reported.map(timestamp),
        }
    }

    /// The queue's configuration attributes as a read reports them: every documented default
    /// is already resolved on the stored record, so what a create accepted is what every later
    /// read reports.
    pub fn to_attributes(&self) -> QueueAttributes {
        QueueAttributes {
            visibility_timeout_seconds: Some(self.visibility_timeout_seconds),
            dead_letter_configuration: Some(DeadLetterConfiguration {
                target_type: if self.max_receive_count.is_some() {
                    TARGET_TYPE_BUILT_IN.to_string()
                } else {
                    TARGET_TYPE_NONE.to_string()
                },
                max_receive_count: self.max_receive_count,
            }),
        }
    }

    /// The stored record projected onto `QueueDescription`, which is what `CreateQueue`,
    /// `UpdateQueue` and `DescribeQueue` all answer with.
    pub fn to_description(
        &self,
        now: SystemTime,
        trim_horizon: Option<SystemTime>,
    ) -> QueueDescription {
        QueueDescription {
            queue_name: self.name.clone(),
            queue_arn: self.arn.clone(),
            queue_status: self.status(now).to_string(),
            queue_creation_timestamp: timestamp(self.created_at),
            stream_configuration_list: vec![QueueStreamConfiguration {
                stream_arn: self.stream_arn.clone(),
                starting_position: Some(self.reported_starting_position(now, trim_horizon)),
            }],
            queue_attributes: Some(self.to_attributes()),
        }
    }

    /// The stored record projected onto `QueueSummary`, which is what `ListQueues` reports.
    /// The members it shares with [`Self::to_description`] come from the same stored fields, so
    /// a listing and a read of one queue never disagree, and its stream is reported as the
    /// identifier pair (`QueueStreamIdentifier`) a summary carries.
    pub fn to_summary(&self, now: SystemTime) -> QueueSummary {
        QueueSummary {
            queue_name: self.name.clone(),
            queue_arn: self.arn.clone(),
            queue_status: self.status(now).to_string(),
            queue_creation_timestamp: timestamp(self.created_at),
            streams: vec![QueueStreamIdentifier {
                stream_arn: self.stream_arn.clone(),
                stream_creation_timestamp: timestamp(self.stream_created_at),
            }],
        }
    }

    /// The queue's tags as the modelled `Tag` list, ordered by key — the SAME projection of
    /// the SAME kind of stored tag set a stream's and a channel's tags are reported through,
    /// because one rule orders the tags of any Kinesis resource.
    pub fn tags_ordered(&self) -> Vec<Tag> {
        tags_ordered(&self.tags)
    }

    /// Whether the queue delivers records from the stream `stream_arn` names, created at
    /// `created_at` when the caller pinned an instant — the `ListQueues` filter: "Specify this
    /// value to distinguish between a deleted stream and a stream that was recreated with the
    /// same name" (`QueueStreamFilter$StreamCreationTimestamp`).
    ///
    /// Instants are compared at the millisecond this service keeps one at
    /// ([`requested_millis`]), because an instant the wire carried as an epoch-seconds number
    /// arrives a few nanoseconds either side of it.
    pub fn delivers_from(
        &self,
        stream_arn: &str,
        created_at: Option<::aws_smithy_types::DateTime>,
    ) -> bool {
        self.stream_arn == stream_arn
            && created_at.is_none_or(|pinned| {
                requested_millis(pinned) == requested_millis(timestamp(self.stream_created_at))
            })
    }

    /// Take the records `pending` into the queue's tracked set, moving its cursor past each of
    /// them: the queue now tracks their delivery state, whatever the stream's retention window
    /// does with the records themselves.
    ///
    /// `tracked_ceiling` bounds what one queue holds at once — the operation that drives the
    /// growth passes it ([`MAX_TRACKED_RECORDS_PER_QUEUE`]), and the cursor advances only for a
    /// record actually taken, so the records past the ceiling are drawn by a later receive
    /// rather than dropped.
    pub fn absorb(&mut self, pending: Vec<QueueDelivery>, tracked_ceiling: usize) {
        for delivery in pending {
            if self.tracked.len() >= tracked_ceiling {
                break;
            }
            self.cursor
                .insert(delivery.shard_id.clone(), delivery.sequence_number.clone());
            self.tracked.push(delivery);
        }
    }

    /// The tracked record the receipt handle `handle` was issued for, if it is this queue's
    /// current handle for one.
    ///
    /// Only the CURRENT handle of a delivery resolves: "A new receipt handle is issued each
    /// time the record is received" (`QueueRecord$ReceiptHandle`), so a handle from an earlier
    /// delivery names a delivery that is over, and receipt handles of a deleted queue "stop
    /// being valid" (`DeleteQueue`) with the record they were issued for.
    pub fn delivery_by_handle_mut(&mut self, handle: &str) -> Option<&mut QueueDelivery> {
        self.tracked
            .iter_mut()
            .find(|delivery| delivery.receipt_handle.as_deref() == Some(handle))
    }

    /// The tracked record `handle` names ON `path`, if any: the two paths address disjoint
    /// sets of a queue's records, because each is reached by its own set of operations "that
    /// use the same queue ARN" (`CreateQueue`) — the receive path addresses what
    /// `ReceiveQueueRecords` can return, and the dead-letter path what has reached the
    /// queue's receive ceiling.
    fn delivery_on_path_mut(
        &mut self,
        path: QueueRecordPath,
        handle: &str,
        now: SystemTime,
    ) -> Option<&mut QueueDelivery> {
        let ceiling = self.max_receive_count;
        self.tracked.iter_mut().find(|delivery| {
            delivery.receipt_handle.as_deref() == Some(handle)
                && match path {
                    // A record whose last delivery took it to the ceiling is still on the
                    // receive path while that delivery lasts: its consumer holds a valid
                    // handle and has until the visibility timeout to acknowledge it.
                    QueueRecordPath::Queue => !delivery.is_dead_letter_at(ceiling, now),
                    QueueRecordPath::DeadLetter => delivery.reached_receive_ceiling(ceiling),
                }
        })
    }

    /// Serve up to `limit` of the queue's records on `path` at `now`, each for
    /// `visibility_timeout`: the records a receive returns, in the order the queue drew them.
    ///
    /// ONE rule for both receives, which differ only in WHICH records they serve: a visible
    /// record for `ReceiveQueueRecords`, a dead-letter record for
    /// `ReceiveDeadLetterQueueRecords` — "the dead-letter record stays in the same queue"
    /// (`ReceiveDeadLetterQueueRecords`), so both read one tracked set.
    pub fn receive(
        &mut self,
        now: SystemTime,
        path: QueueRecordPath,
        limit: usize,
        visibility_timeout: Duration,
    ) -> Vec<QueueRecord> {
        let arn = self.arn.clone();
        let ceiling = self.max_receive_count;
        let mut served: Vec<QueueRecord> = Vec::new();
        for delivery in &mut self.tracked {
            if served.len() >= limit {
                break;
            }
            let serves = match path {
                QueueRecordPath::Queue => {
                    !delivery.is_in_flight(now) && !delivery.is_dead_letter_at(ceiling, now)
                }
                QueueRecordPath::DeadLetter => delivery.is_dead_letter_at(ceiling, now),
            };
            if !serves {
                continue;
            }
            served.push(delivery.deliver(&arn, now, visibility_timeout));
        }
        served
    }

    /// Acknowledge the record `handle` names on `path`: "After a record is acknowledged, the
    /// queue does not deliver it again" (`AcknowledgeQueueRecords`), so the queue stops
    /// tracking it — which is also what acknowledging a dead-letter record does, where it
    /// "discards it" (`ReceiveDeadLetterQueueRecords`). `true` when the handle named a record
    /// on that path.
    pub fn acknowledge_on(&mut self, path: QueueRecordPath, handle: &str, now: SystemTime) -> bool {
        let Some(delivery) = self.delivery_on_path_mut(path, handle, now) else {
            return false;
        };
        let record_id = delivery.record_id.clone();
        self.tracked.retain(|delivery| delivery.record_id != record_id);
        true
    }

    /// Change the visibility timeout of the record `handle` names on `path`: "The new
    /// duration, in seconds, that the record is in flight and not visible to any consumer
    /// instance, measured from when this request is processed. Specify 0 to make the record
    /// visible in the queue immediately"
    /// (`UpdateQueueRecordsVisibilityRequestEntry$VisibilityTimeoutSeconds`). `true` when the
    /// handle named a record on that path.
    pub fn change_visibility_on(
        &mut self,
        path: QueueRecordPath,
        handle: &str,
        visibility_timeout_seconds: i32,
        now: SystemTime,
    ) -> bool {
        let timeout = visibility_timeout(visibility_timeout_seconds);
        let Some(delivery) = self.delivery_on_path_mut(path, handle, now) else {
            return false;
        };
        delivery.visible_at = now.checked_add(timeout).unwrap_or(now);
        true
    }

    /// Restore the dead-letter record `handle` names: "Restoring a record clears its
    /// dead-letter state and resets its receive count, so ReceiveQueueRecords can return it
    /// again, up to MaxReceiveCount times before it becomes a dead-letter record again"
    /// (`RestoreDeadLetterQueueRecords`).
    ///
    /// The count is what the dead-letter state is derived from, so clearing the state IS
    /// resetting the count; the record becomes visible at once, and the handle the dead-letter
    /// delivery was issued under is spent — the next receive issues a new one. The instant the
    /// record was FIRST received stays as it was: that delivery happened, and the member
    /// reports "the approximate time the record was first received from the queue"
    /// (`QueueRecord$FirstReceiveTimestamp`). `true` when the handle named a dead-letter
    /// record of this queue.
    pub fn restore(&mut self, handle: &str, now: SystemTime) -> bool {
        let Some(delivery) = self.delivery_on_path_mut(QueueRecordPath::DeadLetter, handle, now)
        else {
            return false;
        };
        delivery.receive_count = 0;
        delivery.receipt_handle = None;
        delivery.visible_at = now;
        true
    }
}
