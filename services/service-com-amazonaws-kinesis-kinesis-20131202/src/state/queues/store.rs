// SPDX-License-Identifier: CC0-1.0

//! How the STORE reaches its queues — the lookups every queue operation resolves an ARN or a
//! name through, the counts its ceilings are read from — and the one place a queue meets its
//! stream's records ([`pending_for`], [`ComAmazonawsKinesisKinesis20131202State::draw_into_queue`]).

use super::*;

impl ComAmazonawsKinesisKinesis20131202State {
    /// The queue `region`'s endpoint holds under `arn` at `now`, if any: `QueueARN` is the one
    /// member every queue read and write addresses a queue by.
    pub fn queue_by_arn(&self, now: SystemTime, region: &str, arn: &str) -> Option<&Queue> {
        self.queues
            .values()
            .find(|queue| queue.region == region && queue.arn == arn)
            .filter(|queue| queue.is_held(now))
    }

    /// [`Self::queue_by_arn`] for a caller that WRITES the record it finds, so the status it
    /// reads and the fields it writes belong to one borrow of the store and no second change
    /// can slip between them.
    pub fn queue_by_arn_mut(
        &mut self,
        now: SystemTime,
        region: &str,
        arn: &str,
    ) -> Option<&mut Queue> {
        self.queues
            .values_mut()
            .find(|queue| queue.region == region && queue.arn == arn)
            .filter(|queue| queue.is_held(now))
    }

    /// The queue `region`'s endpoint holds under `name` at `now`, if any — the lookup a create
    /// makes to tell a second record from a conflict.
    pub fn queue_in(&self, now: SystemTime, region: &str, name: &str) -> Option<&Queue> {
        self.queues
            .get(&RegionalName::new(region, name))
            .filter(|queue| queue.is_held(now))
    }

    /// Every queue `region`'s endpoint holds at `now`, ordered by name.
    ///
    /// A listing is paged by an opaque cursor that names the queue the next page starts at
    /// ([`RegionalPageToken`]), so the queues a read reports need ONE total order that every
    /// page agrees on; a queue is addressed by a name unique in its region, so that order is by
    /// name.
    pub fn queues_ordered_in(&self, now: SystemTime, region: &str) -> Vec<&Queue> {
        let mut queues: Vec<&Queue> = self
            .queues
            .values()
            .filter(|queue| queue.region == region && queue.is_held(now))
            .collect();
        queues.sort_by(|left, right| left.name.cmp(&right.name));
        queues
    }

    /// How many queues are held on the stream `stream_arn` names at `now`, which is what
    /// `StreamDescriptionSummary$QueueCount` reports ("The number of queues created on the data
    /// stream") and what the stream's own consumer ceiling counts a queue against: "A queue
    /// counts as a registered consumer on the stream and shares the stream's registered
    /// consumer limit with enhanced fan-out consumers" (`CreateQueue`).
    pub fn queues_on(&self, now: SystemTime, stream_arn: &str) -> usize {
        self.queues
            .values()
            .filter(|queue| queue.is_held(now) && queue.stream_arn == stream_arn)
            .count()
    }

    /// How many queues `region`'s endpoint holds at `now`, which is what
    /// [`MAX_QUEUES_PER_REGION`] bounds.
    pub fn queues_held_in(&self, now: SystemTime, region: &str) -> usize {
        self.queues
            .values()
            .filter(|queue| queue.region == region && queue.is_held(now))
            .count()
    }

    /// How many queues this emulator holds at `now` across every region, which is what
    /// [`MAX_QUEUES_TOTAL`] bounds: the ceiling no request can partition.
    pub fn queues_held(&self, now: SystemTime) -> usize {
        self.queues
            .values()
            .filter(|queue| queue.is_held(now))
            .count()
    }

    /// Drop every queue whose removal has fallen due at `now`, so the name it held is free for
    /// another create and the store does not carry it any more. Called by the writes that
    /// change which queues this account holds, the way [`Self::drop_removed_streams`] is.
    pub fn drop_removed_queues(&mut self, now: SystemTime) {
        self.queues.retain(|_, queue| queue.is_held(now));
    }

    /// Draw the records the queue `arn` names has not taken from its stream yet into its
    /// tracked set, at `now`.
    ///
    /// The queue holds the delivery state and the STREAM holds the records, so this is the one
    /// place the two meet: the records are read first ([`pending_for`]) and the queue is then
    /// written, which is also what keeps the two borrows of the store disjoint. A queue whose
    /// stream this account no longer holds draws nothing: "When a record reaches the end of the
    /// data stream's retention period, the queue stops delivering it" (`CreateQueue`), and a
    /// stream that is gone retains none.
    ///
    /// `tracked_ceiling` is the most delivery records the queue may hold at once, so the draw
    /// grows its tracked set by no more than that ([`Queue::absorb`]).
    pub fn draw_into_queue(
        &mut self,
        now: SystemTime,
        region: &str,
        arn: &str,
        tracked_ceiling: usize,
    ) {
        let Some(queue) = self.queue_by_arn(now, region, arn) else {
            return;
        };
        let stream_arn = queue.stream_arn.clone();
        let pending = match self.stream_by_arn(now, region, &stream_arn) {
            Some(stream) => pending_for(queue, stream, now),
            None => return,
        };
        if pending.is_empty() {
            return;
        }
        if let Some(queue) = self.queue_by_arn_mut(now, region, arn) {
            queue.absorb(pending, tracked_ceiling);
        }
    }

    /// ONE receive out of the queue `arn` names at `now`: the records it serves on `path`, or
    /// why it serves none.
    ///
    /// The whole rule for both of this service's receives, so the two operations differ only in
    /// the path they ask for and in the modelled error each words a failure with: the queue is
    /// resolved, a queue that does not deliver in its current state serves nothing, the queue
    /// draws whatever its stream holds past its cursor (the receive path only — a record
    /// reaches the dead-letter path by being received, never by arriving), and the selection
    /// itself is [`Queue::receive`].
    ///
    /// `visibility_timeout_seconds` is the request's, when it named one: "If you do not specify
    /// this value, the queue's visibility timeout is used"
    /// (`ReceiveQueueRecordsInput$VisibilityTimeoutSeconds`).
    ///
    /// `asked` carries what the receive asks for and the bounds it is held to
    /// ([`QueueReceive`]): the records it draws, the count it returns and the ceiling the
    /// queue's tracked set stays inside, all resolved by the operation that drives them.
    pub fn receive_from_queue(
        &mut self,
        now: SystemTime,
        region: &str,
        arn: &str,
        asked: QueueReceive,
    ) -> Result<Vec<QueueRecord>, QueueReceiveFailure> {
        // A queue whose removal has fallen due is no resource of this account any more, and its
        // receipt handles "stop being valid" with it (`DeleteQueue`).
        self.drop_removed_queues(now);
        let queue = self
            .queue_by_arn(now, region, arn)
            .ok_or(QueueReceiveFailure::NotFound)?;
        if !queue.delivers(now) {
            return Err(QueueReceiveFailure::NotDelivering {
                name: queue.name.clone(),
                status: queue.status(now).to_string(),
            });
        }
        if asked.path == QueueRecordPath::Queue {
            self.draw_into_queue(now, region, arn, asked.tracked_ceiling);
        }
        let queue = self
            .queue_by_arn_mut(now, region, arn)
            .ok_or(QueueReceiveFailure::NotFound)?;
        let timeout = asked
            .visibility_timeout_seconds
            .map(visibility_timeout)
            .unwrap_or_else(|| queue.visibility_timeout());
        Ok(queue.receive(now, asked.path, asked.limit, timeout))
    }
}

/// The records of `stream` that `queue` has not drawn yet at `now`, oldest first: everything
/// past the queue's own cursor in each shard that arrived at or after the position delivery
/// begins at ([`Queue::effective_start`]) and that the stream still retains.
///
/// Reads both records without writing either, so the caller can take the queue's lock for the
/// write alone ([`Queue::absorb`]).
pub fn pending_for(queue: &Queue, stream: &Stream, now: SystemTime) -> Vec<QueueDelivery> {
    let trim_horizon = stream.trim_horizon(now);
    let start = queue.effective_start(trim_horizon);
    let mut pending: Vec<QueueDelivery> = Vec::new();
    for shard in &stream.shards {
        let drawn = queue.cursor.get(&shard.shard_id);
        for record in shard.retained(trim_horizon) {
            // Past the cursor: a record at or before it is already tracked, or was tracked and
            // has been acknowledged, so it is never drawn twice.
            if drawn.is_some_and(|cursor| sequence_number_le(&record.sequence_number, cursor)) {
                continue;
            }
            if record.arrived_at < start {
                continue;
            }
            pending.push(QueueDelivery {
                record_id: record_id(&stream.arn, &shard.shard_id, &record.sequence_number),
                shard_id: shard.shard_id.clone(),
                sequence_number: record.sequence_number.clone(),
                partition_key: record.partition_key.clone(),
                data: record.data.clone(),
                arrived_at: record.arrived_at,
                // Nothing has received the record out of this queue yet, so it carries no
                // first-receive instant, no count and no handle, and is visible from the
                // instant it arrived in the stream.
                first_received_at: None,
                receive_count: 0,
                receipt_handle: None,
                visible_at: record.arrived_at,
                deliveries: 0,
            });
        }
    }
    // "Records are not necessarily received in the order they were written to the data stream"
    // (`ReceiveQueueRecords`) — a stream orders records inside a shard, not across shards — so
    // the order the queue draws them in is by arrival, which is the order one shard's records
    // already carry.
    pending.sort_by_key(|delivery| epoch_millis(delivery.arrived_at));
    pending
}
