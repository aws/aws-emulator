// SPDX-License-Identifier: CC0-1.0

//! The CLOCK an emulated service's clock-owed transitions are decided against.
//!
//! A service that reports a resource in a transient status (`creating`, `modifying`,
//! `pending`) records the instant the transition to its settled status falls due, and
//! materialises that transition on the next call that reads or writes the resource. Both
//! halves — the due instant it records and the comparison that materialises it — read the
//! clock THIS module holds for the emulator, never the process clock directly, so one
//! emulator's whole set of owed transitions can be held open or crossed deliberately.
//!
//! Running by default: [`now`] then answers `Instant::now()`, so every span passes for a
//! caller of the served emulator exactly as it did before this clock existed. [`stop`] stops
//! it where it stands and [`advance`] moves a stopped clock on, which is how a caller that
//! has to observe BOTH sides of one span crosses it: a service's own integration tests stop
//! the clock so a transient status stays observable for as long as they need, then advance it
//! by a known amount so the settled status is reached by the clock rather than by a wait.
//! Nothing a REQUEST can reach moves it.
//!
//! Held as the emulator's own engine state, like every other store, so the clock belongs to
//! one emulator and two emulators in one test process cannot move each other's time.

use ::std::time::{Duration, Instant};

/// The clock one emulator measures its owed transitions against.
///
/// Reached through the free functions of this module rather than by locking it: a reader
/// wants the instant, and [`now`] answers it whether or not this store was ever created.
#[derive(Debug, Default)]
pub struct Clock {
    /// Where a STOPPED clock stands, and `None` while the clock follows the process clock.
    stopped_at: Option<Instant>,
}

impl Clock {
    /// What this clock reads: the instant a due instant is measured from, and compared
    /// against.
    #[must_use]
    pub fn now(&self) -> Instant {
        self.stopped_at.unwrap_or_else(Instant::now)
    }

    /// Stop the clock where it stands, and answer that instant.
    ///
    /// Idempotent: a clock that is already stopped keeps the instant it stands at, so
    /// stopping it twice cannot lose time an earlier [`Clock::advance`] added.
    pub fn stop(&mut self) -> Instant {
        *self.stopped_at.get_or_insert_with(Instant::now)
    }

    /// Move the clock on by `span`, stopping it first if it is still running, and answer the
    /// instant it now reads.
    ///
    /// Forward only, so a due instant this clock has already passed can never come back; an
    /// advance that would run off the end of the monotonic clock's range (which no span a
    /// service records comes near) leaves the clock where it stands rather than failing.
    pub fn advance(&mut self, span: Duration) -> Instant {
        let standing = self.stop();
        let moved = standing.checked_add(span).unwrap_or(standing);
        self.stopped_at = Some(moved);
        moved
    }
}

/// What the clock of the emulator behind `engine` reads.
///
/// The one way time enters a service that uses this module: a handler records the due
/// instants of the transitions it starts, and compares the ones it already owes, against
/// this instant rather than against the process clock. An engine this clock was never
/// created in reads the process clock, so a service needs no setup to use it.
///
/// Takes the clock's own lock for that read alone and releases it, and the clock store
/// touches nothing else, so it is safe to read while another store's guard is held.
#[must_use]
pub fn now<Eng: Send + Sync + 'static>(engine: &Eng) -> Instant {
    clock(engine).map_or_else(Instant::now, |clock| clock.lock().now())
}

/// Stop this emulator's clock where it stands, so no transition it owes falls due until
/// [`advance`] brings it there; answers the instant the clock now stands at, and `None` on an
/// engine that holds no store to stop.
pub fn stop<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<Instant> {
    clock(engine).map(|clock| clock.lock().stop())
}

/// Move this emulator's clock on by `span`, stopping it first if it is still running — which
/// is what crosses a span a transition is owed after without waiting for it.
///
/// Because the clock is stopped at an instant no earlier than every due instant already
/// recorded, an advance by a whole span always crosses a due instant an earlier call
/// recorded, whatever the host was doing in between.
pub fn advance<Eng: Send + Sync + 'static>(engine: &Eng, span: Duration) -> Option<Instant> {
    clock(engine).map(|clock| clock.lock().advance(span))
}

/// This emulator's clock store, or `None` when `engine` is not the shared in-memory engine
/// (which only a test harness of this workspace can arrange).
fn clock<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<::engine::ServiceState<Clock>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<Clock>)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spans a case advances by: nothing at all, the finest the clock keeps, the span a
    /// service's own provisioning transition is owed, and one far past any of them.
    const SPANS: [(&str, Duration); 4] = [
        ("no span at all", Duration::ZERO),
        ("one nanosecond", Duration::from_nanos(1)),
        ("a provisioning span", Duration::from_millis(500)),
        ("an hour", Duration::from_secs(3_600)),
    ];

    #[test]
    fn a_running_clock_never_goes_backwards() {
        let clock = Clock::default();
        let first = clock.now();
        let second = clock.now();
        assert!(
            second >= first,
            "case a running clock read twice: expected the second reading not to precede the \
             first, got {first:?} then {second:?}"
        );
    }

    #[test]
    fn stopping_is_idempotent_and_the_clock_then_stands_still() {
        let mut clock = Clock::default();
        let stopped = clock.stop();
        assert_eq!(
            clock.stop(),
            stopped,
            "case stopped twice: expected the clock to keep the instant it stands at, got {:?} \
             instead of {stopped:?}",
            clock.stop()
        );
        for read in 1..=3 {
            assert_eq!(
                clock.now(),
                stopped,
                "case read {read} of a stopped clock: expected it to stand at {stopped:?}, got \
                 {:?}",
                clock.now()
            );
        }
    }

    #[test]
    fn advancing_moves_a_stopped_clock_forward_by_the_span() {
        for (case, span) in SPANS {
            let mut clock = Clock::default();
            let stopped = clock.stop();
            let moved = clock.advance(span);
            assert_eq!(
                moved,
                stopped + span,
                "case {case}: expected the clock to stand {span:?} past {stopped:?}, got {moved:?}"
            );
            assert_eq!(
                clock.now(),
                moved,
                "case {case}: expected a read to answer the instant the advance left, got {:?} \
                 instead of {moved:?}",
                clock.now()
            );
            // Advances ACCUMULATE: a second one crosses one more span rather than
            // re-crossing the first.
            assert_eq!(
                clock.advance(span),
                moved + span,
                "case {case} advanced twice: expected two spans past {stopped:?}, got {:?}",
                clock.now()
            );
        }
    }

    #[test]
    fn advancing_a_running_clock_stops_it_first() {
        let mut clock = Clock::default();
        let moved = clock.advance(Duration::from_millis(500));
        for read in 1..=3 {
            assert_eq!(
                clock.now(),
                moved,
                "case read {read} after advancing a running clock: expected it to stand at \
                 {moved:?}, got {:?}",
                clock.now()
            );
        }
    }

    #[test]
    fn an_advance_past_the_end_of_the_range_leaves_the_clock_where_it_stands() {
        let mut clock = Clock::default();
        let stopped = clock.stop();
        assert_eq!(
            clock.advance(Duration::MAX),
            stopped,
            "case an advance past the end of the monotonic range: expected the clock to stay at \
             {stopped:?}, got {:?}",
            clock.now()
        );
    }

    #[test]
    fn the_clock_of_an_engine_is_stopped_and_advanced_through_that_engine() {
        let engine = ::engine::InMemoryEngine::new();
        let running = now(&engine);
        let stopped = stop(&engine).expect("the in-memory engine holds a clock store");
        assert!(
            stopped >= running,
            "case stopping an engine's clock: expected the instant it stands at not to precede a \
             reading before it, got {running:?} then {stopped:?}"
        );
        for read in 1..=3 {
            assert_eq!(
                now(&engine),
                stopped,
                "case read {read} of a stopped engine clock: expected {stopped:?}, got {:?}",
                now(&engine)
            );
        }
        for (case, span) in SPANS {
            let moved = advance(&engine, span).expect("the in-memory engine holds a clock store");
            assert_eq!(
                now(&engine),
                moved,
                "case {case}: expected the engine's clock to read the instant the advance left \
                 ({moved:?}), got {:?}",
                now(&engine)
            );
        }
    }

    #[test]
    fn an_engine_without_a_clock_store_reads_the_process_clock_and_cannot_be_stopped() {
        // Not the shared in-memory engine, which is the only engine that holds stores.
        struct ForeignEngine;
        let engine = ForeignEngine;
        assert_eq!(
            stop(&engine),
            None,
            "case stopping a foreign engine: expected no clock to stop, got {:?}",
            stop(&engine)
        );
        assert_eq!(
            advance(&engine, Duration::from_millis(500)),
            None,
            "case advancing a foreign engine: expected no clock to advance, got {:?}",
            advance(&engine, Duration::from_millis(500))
        );
        let first = now(&engine);
        let second = now(&engine);
        assert!(
            second >= first,
            "case a foreign engine read twice: expected the process clock, which never goes \
             backwards, got {first:?} then {second:?}"
        );
    }
}
