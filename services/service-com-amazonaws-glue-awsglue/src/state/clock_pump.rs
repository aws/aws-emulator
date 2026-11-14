// SPDX-License-Identifier: CC0-1.0

//! The ONE in-process clock pump this store runs: it waits for the next instant the CLOCK owes
//! a transition at, materialises what is then due, and publishes the EventBridge events those
//! transitions produced.
//!
//! # Why a pump exists at all
//!
//! Every owed transition this service holds is materialised by whichever later call reads the
//! record ([`ComAmazonawsGlueAwsglueState::settle_due_job_runs`],
//! [`ComAmazonawsGlueAwsglueState::settle_due_crawls`]), which is what makes a read report the
//! state the clock had already reached. That is enough for a client that keeps polling, and
//! not enough for one that does not: Glue is an event SOURCE, and a client that wires a job
//! run or a crawl to downstream work through EventBridge makes no further Glue call at all, so
//! an event published only by the next reader would never be published. The pump is what lets
//! those events leave with no request waiting on any of them.
//!
//! # What it does and does not decide
//!
//! It does NOT decide WHEN anything moves: only the stored due instants do, so a caller
//! reading before one still observes the transient state and two reads at the same instant
//! still answer identically. The pump applies exactly the same settles a reading door applies,
//! and the records those settles produce carry the DUE instant rather than the instant the
//! pump happened to wake up. It performs no work of its own either — no script is run here;
//! the door that performs a script records what it came to.
//!
//! Each wake-up settles both families this store owes transitions to and then drains both
//! event queues ([`publish_pending_job_run_events`], [`publish_pending_crawler_events`]), with
//! NO store guard held across either publication: EventBridge is reached through the
//! framework's in-process callback, and a target the event sets off may call back into this
//! service, so holding the store across it would deadlock the very call the event set off.
//!
//! At most one pump runs per store — the store carries the claim and its generation — it is
//! bounded by [`MAX_CLOCK_WAKE_UPS`] wake-ups, and it stands down as soon as the store owes
//! nothing and has nothing queued. With no async runtime to spawn onto there is no pump, and
//! the transitions are materialised by the next reader instead.

use ::std::time::{Duration, SystemTime};

use super::*;

/// The shortest one wake-up of the pump waits.
///
/// A due instant already in the past is materialised on the NEXT wake-up, not on this one's
/// instant, so the pump always hands the runtime back and a wake-up that materialises nothing
/// cannot turn the loop into a spin. It is far below the spans a transition is owed after
/// ([`JOB_RUN_SETTLES_AFTER`], [`CRAWL_SETTLES_AFTER`]), so it never delays one noticeably.
const MIN_CLOCK_WAIT: Duration = Duration::from_millis(5);

/// The longest one wake-up of the pump waits before it looks at the store again.
///
/// Every state a record owes falls due within a span or two of the one before it, so a wait is
/// never near this value; it is a backstop, so a due instant far in the future cannot park the
/// pump indefinitely.
const MAX_CLOCK_WAIT: Duration = Duration::from_secs(5);

/// How many wake-ups one pump makes before it stands down.
///
/// A backstop, not a budget: the transitions the store owes are already finite (a run owes a
/// handful and a crawl owes one, and the jobs, the runs one job keeps and the crawlers are all
/// bounded), and every wake-up either materialises a transition or clears the due instant that
/// woke it. The cap guarantees termination even if a later change left a due instant that
/// materialising does not clear, and the next call starts a fresh pump.
const MAX_CLOCK_WAKE_UPS: usize = 10_000;

impl ComAmazonawsGlueAwsglueState {
    /// What the pump waits for next, or `None` — having RELEASED its claim — when this store
    /// owes nothing more and has nothing queued.
    ///
    /// Observing "nothing is owed" and standing down happen in ONE critical section, which is
    /// what makes the hand-over race-free: a door records its change (and a start stores its
    /// record, and so its due instant) under this same guard BEFORE it tries to claim the
    /// pump, so either this call sees that instant and the running pump keeps it, or the door's
    /// claim finds the flag already cleared and starts a pump of its own. A change recorded but
    /// not yet published keeps the pump awake too, so nothing queued waits for the next call.
    fn next_due_or_release_pump(&mut self, now: SystemTime, generation: u64) -> Option<SystemTime> {
        if self.has_pending_job_run_events() || self.has_pending_crawler_events() {
            return Some(now);
        }
        let next = [self.next_job_run_due_at(), self.next_crawl_due_at()]
            .into_iter()
            .flatten()
            .min();
        match next {
            Some(due) => Some(due),
            None => {
                self.release_clock_pump(generation);
                None
            }
        }
    }

    /// Claim the single pump for this store: the claim's GENERATION when the caller may start
    /// one, `None` when one is already running.
    ///
    /// The generation is what makes a release safe: a pump releases its own claim and never a
    /// later pump's (see [`Self::release_clock_pump`]).
    fn start_clock_pump(&mut self) -> Option<u64> {
        if self.clock_pump_running {
            return None;
        }
        self.clock_pump_running = true;
        self.clock_pump_generation = self.clock_pump_generation.wrapping_add(1);
        Some(self.clock_pump_generation)
    }

    /// Release the claim `generation` took — and ONLY that one.
    ///
    /// A pump that ends on the wake-up cap must not clear a claim a pump started since is
    /// holding, or two pumps would run against one store.
    fn release_clock_pump(&mut self, generation: u64) {
        if self.clock_pump_generation == generation {
            self.clock_pump_running = false;
        }
    }

    /// Keep the running pump's task handle, aborting the one it replaces.
    ///
    /// The replaced handle is a pump that has already ended — by standing down, or on its
    /// wake-up cap — so the abort is normally a no-op; it is what guarantees the invariant
    /// anyway, that no earlier pump task is still running beside the one that now holds the
    /// claim.
    fn set_clock_pump(&mut self, handle: ::tokio::task::JoinHandle<()>) {
        if let Some(previous) = self.clock_pump.replace(handle) {
            previous.abort();
        }
    }
}

/// Start this store's clock pump, unless one is already running.
///
/// Called by the doors that leave the store owing a transition — a job run's start or stop, a
/// crawl's start — with no store guard held. It returns at once: the waiting, the settling and
/// the publication are the spawned task's, so no request waits on the clock.
pub fn drive_clock(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    state: &::engine::ServiceState<ComAmazonawsGlueAwsglueState>,
) {
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = state.lock().start_clock_pump() else {
        return;
    };
    let callback = callback.clone();
    let pumped = ::engine::ServiceState::clone(state);
    let state = ::engine::ServiceState::clone(state);
    let handle = runtime.spawn(async move {
        // `true` only if the wake-up cap below is what ended the loop; a pump that ran out of
        // work has already released its claim inside the one critical section that observed it
        // ([`ComAmazonawsGlueAwsglueState::next_due_or_release_pump`]).
        let mut capped = true;
        for _ in 0..MAX_CLOCK_WAKE_UPS {
            let now = SystemTime::now();
            let Some(due) = state.lock().next_due_or_release_pump(now, generation) else {
                capped = false;
                break;
            };
            // Every wake-up waits, even for an instant already past: that is what keeps the
            // backstop above from ever costing a CPU-bound spin, and it hands the runtime back
            // between wake-ups.
            let wait = due
                .duration_since(now)
                .unwrap_or_default()
                .clamp(MIN_CLOCK_WAIT, MAX_CLOCK_WAIT);
            ::tokio::time::sleep(wait).await;
            {
                let settled_at = SystemTime::now();
                let mut store = state.lock();
                store.settle_due_job_runs(settled_at);
                store.settle_due_crawls(settled_at);
            }
            publish_pending_job_run_events(&callback, &state).await;
            publish_pending_crawler_events(&callback, &state).await;
        }
        if capped {
            // Only THIS pump's claim, so a pump started since is left holding its own.
            state.lock().release_clock_pump(generation);
        }
    });
    // The handle is kept so the pump it replaces is aborted rather than merely forgotten: that
    // is what guarantees no earlier pump task runs beside the one holding the claim.
    pumped.lock().set_clock_pump(handle);
}
