// SPDX-License-Identifier: CC0-1.0

//! The MAP RUN records this service stores: the resource a Distributed Map state starts,
//! the items and the terminal status the clock owes it, and what a Map Run reports about
//! the thresholds it ran under.
//!
//! They live beside the rest of this service's state ([`super`]) rather than in it, so one
//! resource's record and the rules that read it are in one place.

use super::*;

/// One stored Map Run: the resource a Distributed Map state starts, which
/// `ListMapRuns` reports for the execution that started it.
///
/// A Map Run is NOT keyed by its `(execution, label)` pair in the store, because it
/// is reached by its own ARN; `map_runs_by_execution` is what lists one execution's
/// Map Runs in the order their states were entered.
#[derive(Debug, Clone)]
pub struct MapRun {
    pub arn: String,
    /// The execution whose Distributed Map state started it.
    pub execution_arn: String,
    /// The state machine that execution ran.
    pub state_machine_arn: String,
    /// The label that identifies the Map state ([`map_run_identity::map_state_label`]):
    /// the field of the ARN that tells two Map Runs of one execution apart.
    pub label: String,
    pub status: String,
    pub start_date: SystemTime,
    pub stop_date: Option<SystemTime>,
    /// The terminal status the walk computed and the instant the clock owes it at;
    /// `None` once it has been applied, or once a stop overtook it.
    pub pending: Option<PendingMapRunOutcome>,
    pub max_concurrency: i32,
    pub tolerated_failure_count: i64,
    pub tolerated_failure_percentage: f32,
    pub total_items: i64,
    /// How many of the items the clock has already finished produced a result, and
    /// how many failed: the counts GROW as each item comes due, so what a caller
    /// reads is what the Map Run has done so far rather than what it will have done.
    pub succeeded_items: i64,
    pub failed_items: i64,
    /// The items the clock has NOT finished yet, in the order they finish. Drained by
    /// [`ComAmazonawsSfnAwsstepfunctionsState::settle`] into the counts above, which
    /// is also where the failure budget is weighed.
    pub items: Vec<MapRunItem>,
    /// How many times the parent execution has been redriven since this Map Run was
    /// created, which re-ran the Map state this record belongs to.
    pub redrive_count: i32,
    pub redrive_date: Option<SystemTime>,
    /// The bytes the store counts for this record against [`MAX_STORE_BYTES`], set by
    /// [`ComAmazonawsSfnAwsstepfunctionsState::record_map_runs`] exactly as
    /// [`Execution::retained_bytes`] is.
    pub retained_bytes: usize,
}

/// The terminal status [`asl::run`] computed for a Map Run, owed to the clock: it
/// becomes the Map Run's reported status at `due_at` and not before.
#[derive(Debug, Clone)]
pub struct PendingMapRunOutcome {
    pub due_at: SystemTime,
    pub status: String,
}

/// One item of a Map Run, as the clock owes it: the instant its iteration finished
/// and whether it failed.
///
/// This is [`asl::MapItemOutcome`] with the walk's offset turned into an instant. A
/// Map Run counts an item from this instant on, so the counts it reports are what its
/// items have done SO FAR — the item ran in the walk, but nothing it produced is
/// reported before the clock reaches the instant it produced it at.
#[derive(Debug, Clone, Copy)]
pub struct MapRunItem {
    pub due_at: SystemTime,
    pub failed: bool,
}

/// What a Map Run reports as its `maxConcurrency` for a declared limit of `limit`:
/// the limit itself, and the largest value the modelled integer carries for a limit
/// of `0`.
///
/// `MaxConcurrency`'s own default is `0`, which the Amazon States Language defines as
/// NO limit, and a Map Run reports its limit through a modelled integer: an unlimited
/// Map Run therefore reports the largest value that integer carries, rather than a
/// `0` a client would read as "none at a time". RECORDED: `DescribeMapRun` on a `Map`
/// state that declares no `MaxConcurrency` answers `2147483647`
/// (`sfn.describe-map-run-default-fields-and-execution-counts`).
///
/// One rule for both the `Map` state that declares the limit ([`asl`]) and the
/// `UpdateMapRun` that changes it, so one Map Run never reports "no limit" two ways.
pub fn reported_max_concurrency(limit: i64) -> i32 {
    if limit <= 0 {
        i32::MAX
    } else {
        i32::try_from(limit).unwrap_or(i32::MAX)
    }
}

/// Whether a Map Run whose items turned out this way stayed INSIDE the failure
/// thresholds it ran under.
///
/// `toleratedFailureCount` is "the maximum number of failed items before the Map Run
/// fails" and `toleratedFailurePercentage` "the maximum percentage of failed items"
/// before it does, so either threshold on its own is enough to tolerate the
/// failures: a run is past what it tolerates only when more items failed than the
/// count AND than that share of them. A percentage of `0` — the member's own default
/// — is no share at all, so it tolerates nothing by itself.
///
/// The interpreter decides a `Map` state's outcome with this ([`asl`]), which is also
/// what the Map Run's status reports.
pub fn within_tolerated_failures(
    failed: i64,
    total: i64,
    tolerated_count: i64,
    tolerated_percentage: f64,
) -> bool {
    failed <= tolerated_count
        || (tolerated_percentage > 0.0
            && (failed as f64) * 100.0 <= tolerated_percentage * (total as f64))
}

/// Why an `UpdateMapRun` was not applied; see
/// [`ComAmazonawsSfnAwsstepfunctionsState::update_map_run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapRunUpdateRejection {
    /// The store holds no Map Run under that ARN — the modelled `ResourceNotFound`.
    NotFound,
    /// The Map Run has already reached a terminal status, so it has no in-progress
    /// configuration to change — the modelled `ValidationException` carrying
    /// [`VALIDATION_REASON_COMPLETED_MAP_RUN`].
    Completed,
}

impl MapRun {
    /// Whether this Map Run has reached a terminal status.
    pub fn is_terminal(&self) -> bool {
        self.status != MAP_RUN_STATUS_RUNNING
    }

    /// The bytes this record would retain.
    pub fn measure(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.arn.len()
            + self.execution_arn.len()
            + self.state_machine_arn.len()
            + self.label.len()
            + self.status.len()
            + self.items.len().saturating_mul(std::mem::size_of::<MapRunItem>())
            + self
                .pending
                .as_ref()
                .map_or(0, |pending| pending.status.len())
    }

    /// Count every item the clock has already finished, and leave `RUNNING` at the
    /// failure that BREACHES the failure budget this record carries.
    ///
    /// Both are owed whether or not anybody called. An item is counted under the
    /// status its iteration reached from the instant it finished, so a Map Run reports
    /// what its items have done so far; and the thresholds a Map Run is running under
    /// are a failure budget, weighed against the failures counted so far
    /// ([`within_tolerated_failures`]) each time one is counted — the thresholds the
    /// record carries NOW, which is what `UpdateMapRun` writes. A Map Run whose
    /// failures are inside that budget keeps running; one whose budget a counted
    /// failure breaches is FAILED from the instant of that failure, rather than only
    /// once every one of its items has finished.
    ///
    /// The budget decides WHEN this Map Run stopped running, and the status it stops
    /// at is the one the walk computed for it: the execution around it reached its own
    /// outcome through the same `Map` state, so the two never come to contradict each
    /// other.
    pub(super) fn advance(&mut self, now: SystemTime) {
        let fails_on_budget = self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.status == MAP_RUN_STATUS_FAILED);
        let mut breached_at: Option<SystemTime> = None;
        let mut counted = 0usize;
        while let Some(item) = self.items.get(counted).copied() {
            if item.due_at > now {
                break;
            }
            counted += 1;
            if !item.failed {
                self.succeeded_items = self.succeeded_items.saturating_add(1);
                continue;
            }
            self.failed_items = self.failed_items.saturating_add(1);
            if breached_at.is_none()
                && fails_on_budget
                && !within_tolerated_failures(
                    self.failed_items,
                    self.total_items,
                    self.tolerated_failure_count,
                    f64::from(self.tolerated_failure_percentage),
                )
            {
                breached_at = Some(item.due_at);
            }
        }
        self.items.drain(..counted);
        if let Some(breached_at) = breached_at.filter(|_| !self.is_terminal()) {
            self.status = MAP_RUN_STATUS_FAILED.to_string();
            self.stop_date = Some(breached_at);
            // The clock owes this Map Run no further status: the budget decided it
            // here, at the failure that breached it.
            self.pending = None;
            return;
        }
        // A Map Run no failure took past its budget reaches the status the walk
        // computed, once its items are all counted and the clock makes it due.
        if self.items.is_empty()
            && self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.due_at <= now)
        {
            self.apply_pending();
        }
    }

    /// Apply the terminal status the clock now owes.
    ///
    /// `stopDate` is the instant the transition was DUE, not the instant somebody
    /// happened to look, so two callers reading at different times see one value.
    pub(super) fn apply_pending(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        self.status = pending.status;
        self.stop_date = Some(pending.due_at);
    }
}
