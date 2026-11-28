// SPDX-License-Identifier: CC0-1.0

//! The ADMISSION side of capacity: what is left of the throughput a provisioned table is
//! provisioned for, the per-partition-key ceiling beneath it, and the ceiling either of
//! them accumulates credit up to.
//!
//! What a request CONSUMED — the units a read and a write of a given size cost, and how
//! `ConsumedCapacity` reports them — is `super::capacity_accounting`; this file is the
//! other half, and `super` re-exports it so every caller still names it
//! `crate::state::…`.

use super::*;

/// What is left of the capacity a PROVISIONED table is provisioned for, and when that
/// figure was last advanced.
///
/// "Provisioned throughput is the maximum amount of capacity that an application can
/// consume from a table or index" per second (DynamoDB Developer Guide, "Read/write
/// capacity mode"), and "DynamoDB currently retains up to 300 seconds (5 minutes) of
/// unused read and write capacity" for a burst — so the budget refills at the
/// provisioned rate and never holds more than [`BURST_CAPACITY_WINDOW`] of it. The
/// rate is PASSED IN rather than stored, because the capacity in force is the table's
/// own and an `UpdateTable` moves it.
///
/// Only the figure and the instant are kept, so nothing runs on a timer: how much has
/// refilled is derived from the clock when the next write asks for it.
#[derive(Debug, Clone)]
pub struct CapacityBudget {
    /// What was left at [`Self::measured_at`], or `None` for a budget that has been idle
    /// for its whole window and so holds the ceiling, whatever rate is in force by the
    /// time it is first spent from.
    remaining: Option<f64>,
    /// When `remaining` was measured.
    measured_at: SystemTime,
}

impl CapacityBudget {
    /// The budget a resource created at `now` starts life with: NOTHING banked, filling
    /// from that instant at the rate in force.
    ///
    /// What a burst draws on is capacity that WENT UNUSED — "DynamoDB currently retains up
    /// to 300 seconds (5 minutes) of unused read and write capacity" (DynamoDB Developer
    /// Guide, "Using burst capacity effectively") — and a resource that has only just been
    /// created has had no capacity to leave unused yet. So the credit behind the first
    /// request is the rate over the time the resource has existed, which reaches the whole
    /// [`BURST_CAPACITY_WINDOW`] ceiling only after a resource has sat unused that long.
    pub fn new(now: SystemTime) -> Self {
        Self {
            remaining: Some(0.0),
            measured_at: now,
        }
    }

    /// The budget of something that has been IDLE for at least a whole burst window, so it
    /// holds the ceiling: what a per-key budget minted on the first request that addresses
    /// that key starts from, since the key consumed nothing over the window behind it
    /// ([`KeyWriteBudgets`]).
    pub fn idle(now: SystemTime) -> Self {
        Self {
            remaining: None,
            measured_at: now,
        }
    }

    /// Take `units` from the budget a rate of `units_per_second` refills, reporting
    /// whether the budget covered them.
    ///
    /// A request the budget cannot cover takes NOTHING from it: capacity is consumed by
    /// the work that was done, so a throttled write leaves the budget where it was.
    pub fn take(&mut self, units_per_second: i64, units: f64, now: SystemTime) -> bool {
        self.take_within(units_per_second, BURST_CAPACITY_WINDOW, units, now)
    }

    /// The same, over a stated burst window — what the per-partition-key ceiling is
    /// measured over ([`PARTITION_BURST_WINDOW`]), so one implementation serves both
    /// budgets a write is admitted from.
    pub fn take_within(
        &mut self,
        units_per_second: i64,
        window: Duration,
        units: f64,
        now: SystemTime,
    ) -> bool {
        let available = self.available(units_per_second, window, now);
        self.measured_at = now;
        let covered = available >= units;
        self.remaining = Some(if covered { available - units } else { available });
        covered
    }

    /// Whether the budget would cover `units` at `now`, WITHOUT taking them: what lets a
    /// write be held to two budgets at once and debit neither when either refuses it.
    pub fn covers_within(
        &self,
        units_per_second: i64,
        window: Duration,
        units: f64,
        now: SystemTime,
    ) -> bool {
        self.available(units_per_second, window, now) >= units
    }

    /// Whether the budget has ANY capacity left at `now`.
    ///
    /// This is what ADMISSION asks, rather than whether the whole draw is covered:
    /// provisioned throughput is "the maximum amount of capacity that an application can
    /// consume from a table or index" per SECOND (DynamoDB Developer Guide, "Read/write
    /// capacity mode") — a sustained RATE, not a per-request ceiling. A request arriving
    /// while the resource still has capacity is served and charged what it consumed, which
    /// can leave the budget in deficit ([`Self::take_if_any`]); the deficit repays at the
    /// rate in force, so the rate this budget admits over time is exactly the rate it is
    /// provisioned for, while one request larger than the whole credit is not turned down
    /// for being large.
    pub fn has_capacity(&self, units_per_second: i64, window: Duration, now: SystemTime) -> bool {
        self.available(units_per_second, window, now) > 0.0
    }

    /// Take `units` while the budget has any capacity at all, reporting whether it did.
    ///
    /// The draw is charged IN FULL whatever was left ([`Self::take_borrowing`]), so nothing
    /// is served from capacity that is never accounted; a request the budget had nothing
    /// left for takes nothing and leaves the budget exactly where it was, because a
    /// throttled request did no work.
    pub fn take_if_any(
        &mut self,
        units_per_second: i64,
        window: Duration,
        units: f64,
        now: SystemTime,
    ) -> bool {
        if !self.has_capacity(units_per_second, window, now) {
            return false;
        }
        self.take_borrowing(units_per_second, window, units, now);
        true
    }

    /// Whether the budget can serve `units` by BORROWING against its own refill: the
    /// draw may leave the budget in deficit, but no deeper than `allowance`.
    ///
    /// Reports it without taking anything, so a write held to two budgets at once
    /// debits neither when either of them refuses it.
    pub fn covers_borrowing(
        &self,
        units_per_second: i64,
        window: Duration,
        units: f64,
        now: SystemTime,
        allowance: f64,
    ) -> bool {
        self.available(units_per_second, window, now) - units >= -allowance
    }

    /// Take `units` the budget may be SHORT of, leaving it in deficit by the shortfall.
    ///
    /// The units are charged in full, so nothing is served from capacity that is never
    /// accounted: the deficit repays at the rate in force, which leaves the rate this
    /// budget bounds exactly what it was. What borrowing changes is only how far ahead
    /// of its own refill one draw may be served (see
    /// [`Table::admit_item_write_with_progress`]), and
    /// [`CapacityBudget::covers_borrowing`] is what bounds that.
    pub fn take_borrowing(
        &mut self,
        units_per_second: i64,
        window: Duration,
        units: f64,
        now: SystemTime,
    ) {
        let available = self.available(units_per_second, window, now);
        self.measured_at = now;
        self.remaining = Some(available - units);
    }

    /// What has refilled by `now`: the figure last measured plus the rate over the time
    /// since, held at the ceiling the burst window allows.
    fn available(&self, units_per_second: i64, window: Duration, now: SystemTime) -> f64 {
        let rate = units_as_f64(units_per_second);
        let ceiling = burst_ceiling(units_per_second, window);
        let elapsed = now
            .duration_since(self.measured_at)
            .unwrap_or_default()
            .as_secs_f64();
        match self.remaining {
            None => ceiling,
            Some(remaining) => (remaining + elapsed * rate).min(ceiling),
        }
    }

    /// Whether this budget has gone a whole `window` without being spent from, which
    /// means it has refilled to the ceiling and holds no more information than a budget
    /// that was never spent from at all.
    fn refilled(&self, window: Duration, now: SystemTime) -> bool {
        self.remaining.is_none()
            || now
                .duration_since(self.measured_at)
                .is_ok_and(|idle| idle >= window)
    }
}

/// The MOST a budget refilled at `units_per_second` can ever hold: the rate over the
/// whole window it accumulates credit over.
///
/// It is the ceiling [`CapacityBudget::available`] holds a refill at, so it is also the
/// largest single draw that budget can ever cover however long the caller waits — which
/// is what tells a write that is merely waiting for capacity from one no amount of
/// waiting admits.
pub fn burst_ceiling(units_per_second: i64, window: Duration) -> f64 {
    units_as_f64(units_per_second) * window.as_secs_f64()
}

/// What is left of the per-partition-key write capacity of one table: one budget per
/// partition key value that has been written to recently.
///
/// Admission-control bookkeeping ONLY — no response reports it — and bounded: entries
/// that have refilled are dropped once the map reaches [`MAX_TRACKED_WRITE_KEYS`], so a
/// table that is written across many keys does not accumulate a budget per key it ever
/// held.
#[derive(Debug, Clone, Default)]
pub struct KeyWriteBudgets {
    budgets: BTreeMap<AttributeValueKey, CapacityBudget>,
}

impl KeyWriteBudgets {
    /// Whether the key's own ceiling covers `units` at `now`, without taking them.
    pub(super) fn covers(
        &self,
        partition: &AttributeValueKey,
        units: f64,
        now: SystemTime,
    ) -> bool {
        self.budgets.get(partition).is_none_or(|budget| {
            budget.covers_within(
                PARTITION_WRITE_UNITS_PER_SECOND,
                PARTITION_BURST_WINDOW,
                units,
                now,
            )
        })
    }

    /// Take `units` from that key's budget, reporting whether it covered them.
    ///
    /// The map is held at [`MAX_TRACKED_WRITE_KEYS`] entries BEFORE a new one is added,
    /// so the bookkeeping one table carries is a constant of this store and not a
    /// function of how many keys have been written: the budgets that have refilled go
    /// first (they hold no more than a key never written to), and if every entry is
    /// still live the least recently spent from gives way, because it is the closest to
    /// having refilled.
    pub(super) fn take(
        &mut self,
        partition: &AttributeValueKey,
        units: f64,
        now: SystemTime,
    ) -> bool {
        self.make_room_for(partition, now);
        self.budgets
            .entry(partition.clone())
            .or_insert_with(|| CapacityBudget::idle(now))
            .take_within(
                PARTITION_WRITE_UNITS_PER_SECOND,
                PARTITION_BURST_WINDOW,
                units,
                now,
            )
    }

    /// Whether that key's budget can serve `units` by borrowing against its own refill,
    /// no deeper than `allowance` ([`CapacityBudget::covers_borrowing`]).
    pub(super) fn covers_borrowing(
        &self,
        partition: &AttributeValueKey,
        units: f64,
        now: SystemTime,
        allowance: f64,
    ) -> bool {
        self.budgets.get(partition).is_none_or(|budget| {
            budget.covers_borrowing(
                PARTITION_WRITE_UNITS_PER_SECOND,
                PARTITION_BURST_WINDOW,
                units,
                now,
                allowance,
            )
        })
    }

    /// Take `units` from that key's budget, leaving it in deficit by whatever it was
    /// short of ([`CapacityBudget::take_borrowing`]).
    pub(super) fn take_borrowing(
        &mut self,
        partition: &AttributeValueKey,
        units: f64,
        now: SystemTime,
    ) {
        self.make_room_for(partition, now);
        self.budgets
            .entry(partition.clone())
            .or_insert_with(|| CapacityBudget::idle(now))
            .take_borrowing(
                PARTITION_WRITE_UNITS_PER_SECOND,
                PARTITION_BURST_WINDOW,
                units,
                now,
            );
    }

    /// Hold the map at [`MAX_TRACKED_WRITE_KEYS`] entries before an entry for
    /// `partition` is added to it.
    fn make_room_for(&mut self, partition: &AttributeValueKey, now: SystemTime) {
        if self.budgets.len() >= MAX_TRACKED_WRITE_KEYS && !self.budgets.contains_key(partition) {
            self.budgets
                .retain(|_, budget| !budget.refilled(PARTITION_BURST_WINDOW, now));
            while self.budgets.len() >= MAX_TRACKED_WRITE_KEYS {
                let Some(oldest) = self
                    .budgets
                    .iter()
                    .min_by_key(|(_, budget)| budget.measured_at)
                    .map(|(key, _)| key.clone())
                else {
                    break;
                };
                self.budgets.remove(&oldest);
            }
        }
    }
}
