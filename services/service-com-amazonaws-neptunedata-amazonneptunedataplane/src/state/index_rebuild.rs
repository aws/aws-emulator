// SPDX-License-Identifier: CC0-1.0

//! The INDEX REBUILDS this engine holds, and what the storage endpoints answer from them.
//!
//! An index rebuild is a job the storage layer runs: `StartIndexRebuild` admits one,
//! `GetIndexRebuildStatus` reports how far it has got, and `CancelIndexRebuild` stops one
//! that is still running. A rebuild is reorganisation of the storage this engine already
//! holds, so the records below carry WHAT was asked for (the scope, the concurrency and the
//! write-buffer size the request settled on) and WHEN each transition of the job falls due;
//! every status, percentage and instant the three endpoints report is computed from one of
//! those records.
//!
//! The job is not finished the instant the start returns, so the record stores the instants
//! its transitions fall due and a read derives the status from the clock — nothing sleeps
//! for them, and no read has to write a status for a later read to see it.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{ConcurrentModificationException, InvalidParameterException};

/// The values `RebuildStatus` carries that a rebuild of this engine passes through, each the
/// status of one stage of the job.
///
/// Source: smithy_query(query="shape", shape="RebuildStatus") — `PREPARING`, `REBUILDING`,
/// `COMPLETED`, `FAILED`, `CANCELLED`, `CANCELLING`. `FAILED` is deliberately absent: the
/// only work a rebuild of this engine does is reorganising storage this engine already
/// holds, and nothing in that can fail, so no record can reach it and no reason is reported
/// under `failureReason`.
pub const REBUILD_STATUS_PREPARING: &str = "PREPARING";
pub const REBUILD_STATUS_REBUILDING: &str = "REBUILDING";
pub const REBUILD_STATUS_COMPLETED: &str = "COMPLETED";
pub const REBUILD_STATUS_CANCELLING: &str = "CANCELLING";
pub const REBUILD_STATUS_CANCELLED: &str = "CANCELLED";

/// `StartIndexRebuildInput$scope`'s documented default — the member's own documentation
/// states "`ALL` - Rebuild all (quads and dictionaries). This is the default if not
/// specified".
pub const DEFAULT_REBUILD_SCOPE: &str = "ALL";

/// `StartIndexRebuildInput$writeBufferSize`'s documented default — the member's own
/// documentation states "`LOW` - Suitable for databases with light to moderate write
/// activity. This is the default".
pub const DEFAULT_WRITE_BUFFER_SIZE: &str = "LOW";

/// `StartIndexRebuildInput$concurrency`'s documented default — the member's own
/// documentation states "If not specified, defaults to 1".
pub const DEFAULT_REBUILD_CONCURRENCY: i32 = 1;

/// The lower bound `RebuildConcurrency` declares, which is also the least a rebuild can run
/// with — the boundary validator refuses a request below it before any handler runs.
/// Source: smithy_query(query="shape", shape="RebuildConcurrency") — range min 1.
pub const MIN_REBUILD_CONCURRENCY: i32 = 1;

/// How long the storage layer spends PREPARING before the rebuild itself starts, and how
/// long the rebuild then runs.
///
/// A rebuild is work, so it is NOT finished the instant the start returns: these are the
/// spans the clock owes its two transitions, and a record stores the instants they fall due
/// rather than anything sleeping for them. A caller reading the job straight after starting
/// it therefore sees it preparing, as it would against Neptune.
pub const REBUILD_PREPARATION: Duration = Duration::from_secs(3);
pub const REBUILD_DURATION: Duration = Duration::from_secs(3);

/// How long a rebuild stays CANCELLING before it is CANCELLED: the span the clock owes the
/// transition a cancel starts.
pub const REBUILD_CANCELLATION: Duration = Duration::from_secs(3);

/// How many rebuild records this engine keeps.
///
/// Every start is a caller-driven write to the registry, so the registry has a ceiling: a
/// finished rebuild is kept so a later status read can still report the id its submitter was
/// given, but only the most recent ones, so a stream of rebuilds cannot grow this engine's
/// memory without end.
pub const REBUILD_HISTORY: usize = 64;

/// The input members of the rebuild endpoints, by their wire names, which are the names a
/// refusal has to name for the caller to fix the request from it.
pub const REBUILD_ID_MEMBER: &str = "rebuildId";

/// The number of vCPUs the writer instance of this engine has, which is the ceiling a
/// requested concurrency is capped at: `StartIndexRebuildInput$concurrency` is documented as
/// "between 1 and the number of vCPUs available on the writer instance. If the value exceeds
/// the available vCPUs, it is capped at the maximum".
///
/// The writer instance this plane answers for is this emulator's own process, so the count
/// is the parallelism the host really affords it rather than a number stated here; a host
/// that will not say reports the one vCPU every instance has.
pub fn writer_vcpus() -> i32 {
    let available = ::std::thread::available_parallelism().map_or(1, ::std::num::NonZeroUsize::get);
    i32::try_from(available).unwrap_or(i32::MAX)
}

/// The concurrency a rebuild asked for `requested` really runs with: the request's own
/// value, capped at the writer instance's vCPU count, which is "the effective (capped) value
/// returned in the response".
///
/// The model's own lower bound ([`MIN_REBUILD_CONCURRENCY`]) is what keeps the floor: a
/// request below it never reaches a handler, and the ceiling is at least that, so the value
/// this hands back is one a rebuild can run with.
pub fn effective_concurrency(requested: i32) -> i32 {
    requested.clamp(MIN_REBUILD_CONCURRENCY, writer_vcpus())
}

/// The `InvalidParameterException` payload for a `rebuildId` the registry does not hold.
///
/// `detailedMessage` names the member and the value that was wrong, so the caller can fix the
/// request from it; the error CLASS a client branches on is written from the shape's own name
/// (see `crate::ops::types::error_message_wire`).
pub fn invalid_rebuild_id(rebuild_id: &str) -> InvalidParameterException {
    InvalidParameterException {
        detailed_message: format!(
            "Supplied {REBUILD_ID_MEMBER} {} is invalid",
            super::echoed(rebuild_id)
        ),
        request_id: super::new_request_id(),
    }
}

/// The `InvalidParameterException` payload for a cancel of a rebuild that is no longer
/// running: the operation "cancels an in-progress index rebuild operation", and a job that
/// has reached a terminal status has nothing left to stop, so the message names the id and
/// the status it is in.
pub fn rebuild_not_in_progress(rebuild_id: &str, status: &str) -> InvalidParameterException {
    InvalidParameterException {
        detailed_message: format!(
            "Supplied {REBUILD_ID_MEMBER} {} is {status}, and only an index rebuild in \
             progress can be cancelled",
            super::echoed(rebuild_id)
        ),
        request_id: super::new_request_id(),
    }
}

/// The `ConcurrentModificationException` payload for a start made while another rebuild is
/// still running: one storage layer reorganises its indexes once at a time, so the rebuild
/// already in flight is the process concurrently modifying the data this request asked to
/// rebuild. The message names that rebuild and the status it is in.
pub fn rebuild_already_in_progress(
    rebuild_id: &str,
    status: &str,
) -> ConcurrentModificationException {
    ConcurrentModificationException {
        detailed_message: format!(
            "An index rebuild is already in progress: {REBUILD_ID_MEMBER} {} is {status}",
            super::echoed(rebuild_id)
        ),
        request_id: super::new_request_id(),
    }
}

/// One index rebuild this engine holds, as `StartIndexRebuild` admitted it.
#[derive(Debug, Clone)]
pub struct IndexRebuildRecord {
    /// The id the rebuild is reached by — `GetIndexRebuildStatus`'s and
    /// `CancelIndexRebuild`'s `rebuildId` label, and the key this record is stored under.
    pub rebuild_id: String,
    /// The scope the rebuild runs over, as the request settled it.
    pub scope: String,
    /// The concurrency the rebuild really runs with: what the request asked for, capped at
    /// the writer instance's vCPU count.
    pub concurrency: i32,
    /// The write-buffer size the rebuild runs with, as the request settled it.
    pub write_buffer_size: String,
    /// The idempotency token the start presented, when it presented one: a retry carrying
    /// the same token is the same request, so it reaches this same rebuild rather than
    /// starting a second one.
    pub client_token: Option<String>,
    /// When the rebuild was admitted, which is when it started preparing.
    pub started_at: SystemTime,
    /// When preparation ends and the rebuild itself begins.
    pub rebuilding_at: SystemTime,
    /// When the rebuild finishes, if nothing cancels it first.
    pub completes_at: SystemTime,
    /// When a cancel was accepted, and when the cancellation it started completes. `None`
    /// while no cancel has been accepted for this rebuild.
    pub cancel: Option<RebuildCancellation>,
}

/// The cancellation of one rebuild: when it was asked for, and when it falls due.
#[derive(Debug, Clone, Copy)]
pub struct RebuildCancellation {
    /// When the cancel was accepted. The work stopped here, so this is the instant the
    /// progress the status reports is frozen at.
    pub requested_at: SystemTime,
    /// When the rebuild becomes `CANCELLED`; until then it is `CANCELLING`.
    pub cancelled_at: SystemTime,
}

impl IndexRebuildRecord {
    /// A record for a rebuild admitted at `started_at`, with the two transitions the clock
    /// owes it already due at the instants they fall.
    pub fn admitted(
        rebuild_id: impl Into<String>,
        scope: impl Into<String>,
        concurrency: i32,
        write_buffer_size: impl Into<String>,
        client_token: Option<String>,
        started_at: SystemTime,
    ) -> Self {
        let rebuilding_at = started_at + REBUILD_PREPARATION;
        Self {
            rebuild_id: rebuild_id.into(),
            scope: scope.into(),
            concurrency,
            write_buffer_size: write_buffer_size.into(),
            client_token,
            started_at,
            rebuilding_at,
            completes_at: rebuilding_at + REBUILD_DURATION,
            cancel: None,
        }
    }

    /// The rebuild's status as of `now`: preparing until the preparation span has passed,
    /// rebuilding until the rebuild span has, completed afterwards — and, once a cancel has
    /// been accepted, cancelling until that span has passed and cancelled afterwards.
    pub fn status(&self, now: SystemTime) -> &'static str {
        match self.cancel {
            Some(cancel) if now < cancel.cancelled_at => REBUILD_STATUS_CANCELLING,
            Some(_) => REBUILD_STATUS_CANCELLED,
            None if now < self.rebuilding_at => REBUILD_STATUS_PREPARING,
            None if now < self.completes_at => REBUILD_STATUS_REBUILDING,
            None => REBUILD_STATUS_COMPLETED,
        }
    }

    /// Whether the rebuild has reached a status it will not leave — which is what makes a
    /// cancel of it nothing to stop, and what lets another rebuild start.
    ///
    /// A rebuild being cancelled is NOT terminal: the cancellation is still in flight, so
    /// the storage layer is still busy with it.
    pub fn is_terminal(&self, now: SystemTime) -> bool {
        matches!(
            self.status(now),
            REBUILD_STATUS_COMPLETED | REBUILD_STATUS_CANCELLED
        )
    }

    /// `GetIndexRebuildStatusOutput$progressPercentage`: how much of the rebuild has been
    /// done, as "an integer from 0 to 100".
    ///
    /// Nothing is rebuilt while the job is preparing, the share done grows across the
    /// rebuild span, and a completed rebuild is wholly done. A cancelled one reports what it
    /// had done when the cancel stopped it, so the percentage does not go on growing after
    /// the work has stopped.
    pub fn progress_percentage(&self, now: SystemTime) -> i32 {
        let measured_at = match self.cancel {
            Some(cancel) => cancel.requested_at.min(now),
            None => now,
        };
        if measured_at < self.rebuilding_at {
            return 0;
        }
        if measured_at >= self.completes_at {
            return 100;
        }
        let done = measured_at
            .duration_since(self.rebuilding_at)
            .unwrap_or_default()
            .as_secs_f64();
        let whole = REBUILD_DURATION.as_secs_f64();
        if whole <= 0.0 {
            return 100;
        }
        // A rebuild still running has not finished, so the share it reports stops one short
        // of whole: 100 is what a job that is done reports.
        ((done / whole) * 100.0).floor().clamp(0.0, 99.0) as i32
    }

    /// `GetIndexRebuildStatusOutput$endTime`: when the rebuild ended, "only present when the
    /// operation has completed".
    pub fn end_time(&self, now: SystemTime) -> Option<SystemTime> {
        match self.cancel {
            Some(cancel) if now >= cancel.cancelled_at => Some(cancel.cancelled_at),
            Some(_) => None,
            None if now >= self.completes_at => Some(self.completes_at),
            None => None,
        }
    }

    /// `GetIndexRebuildStatusOutput$estimatedEndTime`: when the rebuild is expected to end.
    /// `None` once it has ended, because then the instant it really ended is what the answer
    /// carries.
    pub fn estimated_end_time(&self, now: SystemTime) -> Option<SystemTime> {
        match self.cancel {
            Some(cancel) if now < cancel.cancelled_at => Some(cancel.cancelled_at),
            Some(_) => None,
            None if now < self.completes_at => Some(self.completes_at),
            None => None,
        }
    }

    /// Accept a cancel of this rebuild as of `now`, and report whether this call is what
    /// started the cancellation.
    ///
    /// `false` means a cancel had already been accepted, so the cancellation already in
    /// flight stands: that is what makes a second cancel of one id answer exactly like the
    /// first. The record is KEPT either way, so a status read goes on reporting the id its
    /// submitter was given.
    pub fn request_cancel(&mut self, now: SystemTime) -> bool {
        if self.cancel.is_some() {
            return false;
        }
        self.cancel = Some(RebuildCancellation {
            requested_at: now,
            cancelled_at: now + REBUILD_CANCELLATION,
        });
        true
    }
}

/// Every index rebuild this engine has admitted, keyed by rebuild id.
///
/// A `BTreeMap` is what gives the registry a stable order over the ids, so the bound below
/// forgets the same records however the registry is walked. It derefs to that map for
/// READING only: [`Self::admit`] is the one way a record enters the registry, so the ceiling
/// [`REBUILD_HISTORY`] states cannot be bypassed by inserting through the map.
#[derive(Debug, Default)]
pub struct IndexRebuildRegistry {
    rebuilds: BTreeMap<String, IndexRebuildRecord>,
}

impl ::std::ops::Deref for IndexRebuildRegistry {
    type Target = BTreeMap<String, IndexRebuildRecord>;

    fn deref(&self) -> &Self::Target {
        &self.rebuilds
    }
}

impl IndexRebuildRegistry {
    /// The rebuild that has not reached a terminal status as of `now`, if the registry holds
    /// one — the job the storage layer is busy with, which is what makes a second start a
    /// concurrent modification.
    pub fn in_flight(&self, now: SystemTime) -> Option<&IndexRebuildRecord> {
        self.rebuilds
            .values()
            .find(|rebuild| !rebuild.is_terminal(now))
    }

    /// The rebuild a start presenting `client_token` already admitted, if any: an
    /// idempotency token names the request, so a retry carrying the same one is answered
    /// with the rebuild the first call started.
    pub fn by_client_token(&self, client_token: &str) -> Option<&IndexRebuildRecord> {
        self.rebuilds
            .values()
            .find(|rebuild| rebuild.client_token.as_deref() == Some(client_token))
    }

    /// The record `rebuild_id` names, to WRITE: the one a cancel marks. Reaching a held
    /// record is all this hands out, so no caller can add one past the registry's ceiling.
    pub fn record_mut(&mut self, rebuild_id: &str) -> Option<&mut IndexRebuildRecord> {
        self.rebuilds.get_mut(rebuild_id)
    }

    /// Admit `record`, and apply the registry's bound. The ONE way a record enters the
    /// registry, so the bound below dominates every write to it.
    pub fn admit(&mut self, record: IndexRebuildRecord, now: SystemTime) {
        self.rebuilds.insert(record.rebuild_id.clone(), record);
        self.forget_oldest_finished(now);
    }

    /// Drop the finished rebuilds past [`REBUILD_HISTORY`], oldest first.
    ///
    /// Only a rebuild that has reached a terminal status is forgotten: one the storage layer
    /// is still busy with has to go on being reachable by a status read and a cancel, however
    /// long the registry grows.
    fn forget_oldest_finished(&mut self, now: SystemTime) {
        let mut finished: Vec<(SystemTime, String)> = self
            .rebuilds
            .values()
            .filter(|rebuild| rebuild.is_terminal(now))
            .map(|rebuild| (rebuild.started_at, rebuild.rebuild_id.clone()))
            .collect();
        if finished.len() <= REBUILD_HISTORY {
            return;
        }
        // Oldest first, ties broken by rebuild id, so the order is total.
        finished.sort();
        let surplus = finished.len() - REBUILD_HISTORY;
        for (_, rebuild_id) in finished.iter().take(surplus) {
            self.rebuilds.remove(rebuild_id);
        }
    }
}

/// A fresh rebuild id: the `rebuildId` `StartIndexRebuild` hands back and the two other
/// storage endpoints name a rebuild by.
///
/// Version 4, like every other id this plane mints, so two rebuilds never share an id and
/// the id is derived from nothing the caller sent.
pub fn new_rebuild_id() -> String {
    ::uuid::Uuid::new_v4().to_string()
}
