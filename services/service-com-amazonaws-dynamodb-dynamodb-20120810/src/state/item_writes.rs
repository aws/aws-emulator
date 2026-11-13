// SPDX-License-Identifier: CC0-1.0

//! The store's ITEM-WRITE sites, and the store-wide figure they are admitted against.
//!
//! One item write and one item delete, which every data-plane operation of this service
//! lands through — the single-item writes, the batch and transactional ones and the PartiQL
//! statements — so numbers are canonicalized, capacity is accounted, a change is queued for
//! the feeds that publish it, the replicas of a global table are written and the version the
//! write replaced is kept for the table's continuous backups in ONE place rather than once
//! per handler. They live beside [`super`] because they are the store's growth sites: it is
//! where this emulator's own host-protection ceilings are enforced, which is also why
//! [`ComAmazonawsDynamodbDynamodb20120810State::stored_bytes`], the figure those ceilings are
//! measured against, is here with them. `super` re-exports nothing of it: these are inherent
//! methods of the store, so every caller still reaches them on the state itself.

use super::*;

impl ComAmazonawsDynamodbDynamodb20120810State {
    /// The bytes every table together holds — the store-wide figure admission
    /// control measures against. O(tables), because each table keeps its own total.
    ///
    /// A table's live items AND the item versions its continuous backups retain
    /// ([`PointInTimeRecovery::retained_bytes`]): both are item data this process holds
    /// because a caller wrote it, so one ceiling bounds the two together.
    pub fn stored_bytes(&self) -> i64 {
        self.regions
            .values()
            .flat_map(HashMap::values)
            .map(|table| {
                table
                    .stored_bytes
                    .saturating_add(table.point_in_time_recovery.retained_bytes())
            })
            .fold(0, i64::saturating_add)
    }

    /// Write one item into `name`, replacing whatever that key held.
    ///
    /// Numbers are canonicalized HERE, the store's only item-write site, so what is
    /// stored is the number each `N` denotes rather than the lexeme a writer sent, and
    /// every read path reports that one form without normalising anything itself
    /// ([`canonicalize_item`]). It runs before this method's own budget checks and
    /// after the caller has resolved the table — every handler reaches this through
    /// [`Self::active_table`]/[`Table::key_of_item`] first — so a write naming a table
    /// that is absent or not `ACTIVE` is still answered with that refusal rather than
    /// with a number's.
    ///
    /// This is the store's only growth site, so it is where this emulator's own
    /// HOST-PROTECTION ceilings are enforced ([`MAX_ITEMS_PER_TABLE`],
    /// [`MAX_TABLE_SIZE_BYTES`], [`MAX_STORE_SIZE_BYTES`]): the clamp precedes the insert,
    /// so no write path can grow the process past them. They are not AWS quotas — the
    /// service bounds ONE item ("the maximum item size in DynamoDB is 400 KB",
    /// [`MAX_ITEM_SIZE_BYTES`], checked where the item is built) and not what a table
    /// holds — and they sit far above the data any workload stores, so a legal item is
    /// written however much the table already carries. Replacing an existing item frees
    /// that item's bytes, so a steady overwrite loop is never refused.
    pub fn put_item(
        &mut self,
        region: &str,
        name: &str,
        now: SystemTime,
        key: ItemKey,
        mut item: Item,
    ) -> Result<Option<Item>, RequestError> {
        canonicalize_item(&mut item)?;
        let size = item_size(&item);
        let stored = self.stored_bytes();
        // The key this write lands on, kept for the replication below: `key` itself is
        // moved into the table's map.
        let replicated_key = key.clone();
        let table = self.active_table_mut(region, name, now)?;
        // Whether this write INSERTS is whether the key is held, not whether the item it
        // holds sizes to zero — an item can legitimately size to nothing.
        let replaced = table.items.get(&key);
        let replaced_size = replaced.map_or(0, item_size);
        let table_bytes = table
            .stored_bytes
            .saturating_sub(replaced_size)
            .saturating_add(size);
        // A write to a global table lands in every Region of its group, so it costs the
        // store its own size ONCE PER REGIONAL TABLE: the host-protection ceiling is
        // measured against that whole cost, which is what keeps the replication below
        // inside the same bound as the write that drives it. The factor is bounded by
        // [`MAX_REPLICA_REGIONS`].
        let copies = i64::try_from(table.replicas.len().saturating_add(1)).unwrap_or(1);
        if (replaced.is_none() && table.items.len() >= MAX_ITEMS_PER_TABLE)
            || table_bytes > MAX_TABLE_SIZE_BYTES
            || stored
                .saturating_sub(replaced_size.saturating_mul(copies))
                .saturating_add(size.saturating_mul(copies))
                > MAX_STORE_SIZE_BYTES
        {
            return Err(store_exhausted(name));
        }
        table.stored_bytes = table_bytes;
        // "DynamoDB Streams captures a time-ordered sequence of item-level
        // modifications in any DynamoDB table": a write that LANDS on a table that
        // publishes its changes owes one record, whether it stores a new item or replaces
        // one. A table PUBLISHES through its own stream, through a Kinesis streaming
        // destination that is taking changes, or both. The images the record carries are
        // the only reason to keep a copy of the item, so a table that publishes nowhere
        // clones nothing.
        let publishes_to = table.enabled_stream_arn();
        let publishes = publishes_to.is_some()
            || table.delivers_to_kinesis(now)
            // A table that is COLLECTING key activity owes a record too: a write of an item
            // is an access of its key, and this queue is where a landed write's changes are
            // handed to the stores that answer for them
            // ([`contributor_insights::record_item_changes`]).
            || table.contributor_insights.collecting();
        let keys = if publishes {
            table.stream_keys(&item)
        } else {
            Item::default()
        };
        let new_image = publishes.then(|| item.clone());
        let published_by = publishes.then(|| table.name.clone());
        let replaced = table.items.insert(key, item);
        // The version this write replaced, for the table's continuous backups.
        table.record_item_version(now, &replicated_key, replaced.as_ref());
        let change = published_by.map(|table_name| PendingStreamRecord {
            region: region.to_string(),
            table_name,
            stream_arn: publishes_to,
            keys,
            old_image: replaced.clone(),
            new_image,
            by_caller: true,
        });
        self.queue_stream_record(change);
        // A global table's data is the same in every Region of its group, so the item
        // this write landed lands in each replica too ([`Self::replicate_item_change`]).
        // Read back out of the stored table rather than kept from above, so one write
        // path cannot replicate something else than it stored.
        if let Some(stored) = self
            .table(region, name)
            .ok()
            .and_then(|table| table.items.get(&replicated_key).cloned())
        {
            self.replicate_item_change(region, name, now, &replicated_key, Some(&stored));
        }
        Ok(replaced)
    }

    /// Remove one item from `name`, returning it, and give its bytes back to the
    /// table's budget.
    pub fn delete_item(
        &mut self,
        region: &str,
        name: &str,
        now: SystemTime,
        key: &ItemKey,
    ) -> Result<Option<Item>, RequestError> {
        let table = self.active_table_mut(region, name, now)?;
        let removed = table.items.remove(key);
        // The version this delete removed, for the table's continuous backups.
        table.record_item_version(now, key, removed.as_ref());
        table.stored_bytes = table
            .stored_bytes
            .saturating_sub(removed.as_ref().map_or(0, item_size));
        // A delete that removed NOTHING changed no item, so nothing publishes a record
        // for it: a stream is a sequence of modifications, and a delete of an absent key
        // is none.
        let stream_arn = table.enabled_stream_arn();
        let publishes = stream_arn.is_some()
            || table.delivers_to_kinesis(now)
            // See the same predicate in `put_item`: a collecting table's landed writes are
            // attributed from this queue.
            || table.contributor_insights.collecting();
        let change = removed
            .as_ref()
            .filter(|_| publishes)
            .map(|before| PendingStreamRecord {
                region: region.to_string(),
                table_name: table.name.clone(),
                stream_arn,
                keys: table.stream_keys(before),
                old_image: Some(before.clone()),
                new_image: None,
                by_caller: true,
            });
        self.queue_stream_record(change);
        // The same item change the local table applied is applied to every replica of it,
        // so a key deleted in one Region of a global table is deleted in all of them.
        if removed.is_some() {
            self.replicate_item_change(region, name, now, key, None);
        }
        Ok(removed)
    }
}
