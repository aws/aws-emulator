// SPDX-License-Identifier: CC0-1.0

//! The delivery state of ONE record of a stream inside ONE queue: the receipt handle its
//! current delivery was issued under, how many times it has been received without being
//! acknowledged, and the instant it becomes visible again — with the two predicates the
//! receive paths are decided by derived from those rather than stored.

use super::*;

/// The delivery state of ONE record of the stream inside ONE queue.
///
/// The record's own payload is copied in when the queue draws it past its cursor, because
/// "dead-letter records carry the same data and partition key as when they were first
/// delivered" (`ReceiveDeadLetterQueueRecords`) — a record the stream's retention window has
/// since released is still the record this queue is tracking.
#[derive(Debug, Clone)]
pub struct QueueDelivery {
    /// `QueueRecord$RecordId` — "The unique identifier of the record. This value stays the
    /// same across every delivery of the record, and it is the same in every queue on the
    /// stream that delivers the record", as [`record_id`] derives it.
    pub record_id: String,
    /// The shard the record was written to, which is where this queue's cursor stands.
    pub shard_id: String,
    /// The record's sequence number inside that shard, which orders the cursor.
    pub sequence_number: String,
    /// `QueueRecord$PartitionKey` — "The partition key the record was written to the data
    /// stream with".
    pub partition_key: Option<String>,
    /// `QueueRecord$Data` — the record's data blob, answered byte for byte.
    pub data: Vec<u8>,
    /// `QueueRecord$ApproximateArrivalTimestamp` — "The approximate time the record arrived in
    /// the data stream".
    pub arrived_at: SystemTime,
    /// `QueueRecord$FirstReceiveTimestamp` — "The approximate time the record was first
    /// received from the queue". `None` until the queue has delivered it once.
    pub first_received_at: Option<SystemTime>,
    /// `QueueRecord$ReceiveCount` — "The number of times this record has been received without
    /// being acknowledged", which is what the dead-letter configuration's `MaxReceiveCount` is
    /// counted against and what a restore resets.
    pub receive_count: i32,
    /// The receipt handle the CURRENT delivery was issued under: "A new receipt handle is
    /// issued each time the record is received" (`QueueRecord$ReceiptHandle`). `None` on a
    /// record the queue has never delivered.
    pub receipt_handle: Option<String>,
    /// The instant the record becomes visible in the queue again — the end of its visibility
    /// timeout. A record that has never been delivered is visible from the instant it arrived.
    pub visible_at: SystemTime,
    /// How many receipt handles this queue has issued for the record, so each delivery's handle
    /// differs from every earlier one ([`receipt_handle`]).
    pub deliveries: u64,
}

impl QueueDelivery {
    /// Whether the record is in flight at `now`: "it is not visible to any consumer instance
    /// for the duration of its visibility timeout" (`ReceiveQueueRecords`).
    pub fn is_in_flight(&self, now: SystemTime) -> bool {
        now < self.visible_at
    }

    /// Whether the record has been received as many times as the queue's dead-letter
    /// configuration admits: "when a record has been received MaxReceiveCount times without
    /// being acknowledged, it becomes a dead-letter record" (`DeadLetterConfiguration`).
    ///
    /// `max_receive_count` is the queue's ([`Queue::max_receive_count`]), `None` on a queue
    /// whose dead-letter handling is off \u2014 where no record ever reaches a ceiling, and "an
    /// unacknowledged record continues to be redelivered until it is acknowledged or its
    /// retention period ends".
    pub fn reached_receive_ceiling(&self, max_receive_count: Option<i32>) -> bool {
        max_receive_count.is_some_and(|ceiling| self.receive_count >= ceiling)
    }

    /// Whether the record is a dead-letter record at `now`: it has reached the ceiling AND the
    /// delivery it was last handed out under is over \u2014 a record still in flight has not failed
    /// to be acknowledged yet.
    ///
    /// Derived from the stored count and instant, so two reads at one instant agree and no
    /// read has to write.
    pub fn is_dead_letter_at(&self, max_receive_count: Option<i32>, now: SystemTime) -> bool {
        self.reached_receive_ceiling(max_receive_count) && !self.is_in_flight(now)
    }

    /// [`Self::is_dead_letter_at`] against the queue the record belongs to.
    pub fn is_dead_letter(&self, queue: &Queue, now: SystemTime) -> bool {
        self.is_dead_letter_at(queue.max_receive_count, now)
    }

    /// Whether `ReceiveQueueRecords` serves the record at `now`: visible, and not a
    /// dead-letter record, which that operation "no longer returns"
    /// (`DeadLetterConfiguration`).
    pub fn is_visible(&self, queue: &Queue, now: SystemTime) -> bool {
        !self.is_in_flight(now) && !self.is_dead_letter(queue, now)
    }

    /// Hand the record to a consumer at `now` for `visibility_timeout`: the receive count
    /// grows, the first-receive instant is pinned if this is the first delivery, and a NEW
    /// receipt handle is issued, which is the handle the record is then acknowledged, made
    /// visible or restored under.
    pub fn deliver(
        &mut self,
        queue_arn: &str,
        now: SystemTime,
        visibility_timeout: Duration,
    ) -> QueueRecord {
        self.receive_count = self.receive_count.saturating_add(1);
        self.first_received_at = Some(self.first_received_at.unwrap_or(now));
        self.deliveries = self.deliveries.saturating_add(1);
        let handle = receipt_handle(queue_arn, &self.record_id, self.deliveries);
        self.receipt_handle = Some(handle);
        self.visible_at = now
            .checked_add(visibility_timeout)
            .unwrap_or(self.visible_at);
        self.to_shape(now)
    }

    /// This delivery as the modelled `QueueRecord`, as the receive that issued its current
    /// handle answers it.
    pub fn to_shape(&self, now: SystemTime) -> QueueRecord {
        QueueRecord {
            receipt_handle: self.receipt_handle.clone().unwrap_or_default(),
            record_id: self.record_id.clone(),
            partition_key: self.partition_key.clone(),
            data: self.data.clone(),
            approximate_arrival_timestamp: timestamp(self.arrived_at),
            first_receive_timestamp: timestamp(self.first_received_at.unwrap_or(now)),
            receive_count: self.receive_count,
        }
    }
}
