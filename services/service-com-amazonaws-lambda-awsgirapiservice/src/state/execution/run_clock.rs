// SPDX-License-Identifier: CC0-1.0

//! The clock a RUN's time-based bounds are read against.
//!
//! Which bound stopped a run is the HOST's own reading and never the guest's, and the
//! DEADLINE is the one bound whose reading is a span of TIME: a run that reaches any other
//! bound slowly enough is answered by the deadline instead, so on a slow or loaded host the
//! bound that answers is decided by how fast the host happens to be running rather than by
//! what the run did. Which clock those readings come from is therefore a property of the run
//! rather than a call to the host buried inside each check.
//!
//! Every invocation this emulator serves is bounded against this host's own clock
//! ([`Clock::host`]), which is what [`super::python::run`] and [`super::node::run`] pass: no
//! request chooses a run's clock, and no configuration selects one. A run driven to establish
//! that ONE named bound is what stops it is bounded against a clock whose span this host's
//! speed cannot spend: one that stands STILL ([`Clock::still`]), against which a deadline
//! never falls, so only a bound that is not a span of time can answer the run; or one its own
//! READINGS drive ([`Clock::driven`]), against which a deadline falls after a number of
//! readings — the run's traced events and the host's own samples of it — rather than after a
//! span of this host's time.
//!
//! A clock the host drives therefore leaves this HOST's own time unbounded, which is why a
//! clock that reaches a request path carries a CEILING on it ([`Clock::within`]): the run is
//! stopped once it has taken more of this host's time than that, whatever its own clock
//! reads, so a run whose own time this host's clock does not move is still bounded on this
//! host — by a bound of its own ([`super::outcome::Stop::HostCeiling`]) rather than by the
//! deadline, which keeps what the run REPORTS a matter of what it did.
//!
//! A WAIT the run asked for is spent through the clock too ([`Clock::wait`]), because waiting
//! is how time passes: on this host's own clock the thread really waits, and on a clock the
//! host drives the span is advanced on the clock instead — so the wait costs the RUN the span
//! it asked for and none of this host's time.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where a run's time-based bounds read the current instant from.
///
/// Shared by both engines ([`super::python::Bounds`], [`super::node::Bounds`]): the bound a
/// run is stopped on is read the same way whichever language the handler is written in.
#[derive(Clone, Debug)]
pub struct Clock {
    /// The instant this clock reads before a reading has driven it, or `None` for this
    /// host's own clock, whose readings nothing but time drives.
    base: Option<Instant>,
    /// How far ONE reading drives this clock.
    tick: Duration,
    /// How many readings it has answered, which is what drives it. Shared between the run's
    /// own thread and the host's watch over it, which read the same clock.
    readings: Arc<AtomicU64>,
    /// How much of this clock's time the WAITS the run asked for have spent ([`Clock::wait`]),
    /// in nanoseconds. Shared with every holder of the clock, like the readings above, and
    /// read by none of them: a clock this host's own time moves does its waiting instead.
    waited: Arc<AtomicU64>,
    /// How much of THIS HOST's own time a run bounded against this clock may take
    /// ([`Clock::within`]), or `None` for a clock with no ceiling of its own — this host's
    /// own, whose readings are this host's time already, and a clock used only to drive a run
    /// from a test.
    host_ceiling: Option<Duration>,
    /// When this clock was built, in THIS HOST's own time, which is what the ceiling above is
    /// measured from.
    built_at: Instant,
}

impl Clock {
    /// This HOST's own clock, which every invocation this emulator serves is bounded
    /// against: a reading of it is the instant it is taken at, and nothing but time moves
    /// it.
    #[must_use]
    pub fn host() -> Self {
        Self {
            base: None,
            tick: Duration::ZERO,
            readings: Arc::new(AtomicU64::new(0)),
            waited: Arc::new(AtomicU64::new(0)),
            host_ceiling: None,
            built_at: Instant::now(),
        }
    }

    /// A clock that stands STILL: every reading answers the instant it was built at.
    ///
    /// A deadline read against it never falls, however long the run takes of this host's
    /// time, so the bound that stops a run judged against it is one of the bounds that is
    /// not a span of time — the step budget, the memory ceiling, the response cap. It is for
    /// DRIVING a run, from this crate's own tests: no invocation this emulator serves is
    /// bounded against it, because the operation path passes [`Self::host`].
    #[must_use]
    pub fn still() -> Self {
        Self {
            base: Some(Instant::now()),
            tick: Duration::ZERO,
            readings: Arc::new(AtomicU64::new(0)),
            waited: Arc::new(AtomicU64::new(0)),
            host_ceiling: None,
            built_at: Instant::now(),
        }
    }

    /// A clock its own READINGS drive: each reading answers `tick` after the one before it.
    ///
    /// A span of such a clock is a NUMBER of readings rather than a span of this host's
    /// time, so a deadline read against it falls after the run has delivered that many
    /// traced events — the same number on an idle host and on a loaded one. None of this
    /// host's time moves it, so a run bounded against one on a REQUEST path states what it
    /// may take of this host besides ([`Self::within`]).
    #[must_use]
    pub fn driven(tick: Duration) -> Self {
        Self {
            base: Some(Instant::now()),
            tick,
            readings: Arc::new(AtomicU64::new(0)),
            waited: Arc::new(AtomicU64::new(0)),
            host_ceiling: None,
            built_at: Instant::now(),
        }
    }

    /// The same clock, with a CEILING on how much of THIS HOST's own time a run bounded
    /// against it may take.
    ///
    /// A clock the host drives leaves this host's time unbounded: the deadline it reads
    /// against cannot be spent by the host's own clock, and nor can the host's watch over the
    /// run spend it. So a run on a request path states the host time it may take HERE, and is
    /// stopped once it has taken it ([`Self::past_host_ceiling`]) — which is what keeps the
    /// CPU one run of caller-supplied code may take bounded, whether it is spending traced
    /// events or sitting in one long native call that delivers none.
    ///
    /// It is a bound of its own and not the run's deadline, so what a run REPORTS still says
    /// what the run DID: a run stopped here is one this emulator gave up on
    /// ([`super::outcome::Stop::HostCeiling`]), not one its own span ran out on.
    #[must_use]
    pub fn within(mut self, host_ceiling: Duration) -> Self {
        self.host_ceiling = Some(host_ceiling);
        self
    }

    /// The instant this clock reads NOW, which on a driven clock is also the reading that
    /// drives it.
    #[must_use]
    pub fn now(&self) -> Instant {
        let Some(base) = self.base else {
            return Instant::now();
        };
        let readings = self.readings.fetch_add(1, Ordering::Relaxed);
        let driven = self
            .tick
            .saturating_mul(u32::try_from(readings).unwrap_or(u32::MAX));
        // The waits the run asked for are time this clock has passed as much as its readings
        // are ([`Self::wait`]).
        base + driven.saturating_add(Duration::from_nanos(self.waited.load(Ordering::Relaxed)))
    }

    /// Spend `span` of this clock's own time, which is what a WAIT the run asked for costs
    /// it.
    ///
    /// Time passes on this HOST's own clock by waiting, so a run bounded against it — every
    /// invocation this emulator serves — really waits, and its own deadline then falls where
    /// that wait took it. None of this host's time moves a clock the host drives
    /// ([`Self::still`], [`Self::driven`]), so the span is ADVANCED on that clock instead: the
    /// wait costs the run the span it asked for, the run's deadline falls after the waits it
    /// really asked for, and how fast this host happens to be running spends neither.
    ///
    /// The caller decides how long a run may be made to wait for — the span handed here is
    /// already clamped to what the run has left — and no guest code reaches this.
    pub fn wait(&self, span: Duration) {
        if self.base.is_none() {
            ::std::thread::sleep(span);
            return;
        }
        let nanos = u64::try_from(span.as_nanos()).unwrap_or(u64::MAX);
        let _ = self
            .waited
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |waited| {
                Some(waited.saturating_add(nanos))
            });
    }

    /// Whether a run bounded against this clock has taken more of THIS HOST's own time than
    /// it may ([`Self::within`]).
    ///
    /// Read by the HOST — from the run's own thread at every traced event and from the watch
    /// over that run, which is what notices a run sitting in a long native call — and read
    /// from this host's own clock whatever this one reads, so no guest code and no clock of
    /// the run's own moves it. A clock that states no ceiling answers `false`: this host's own
    /// clock bounds its runs by their deadline already.
    #[must_use]
    pub fn past_host_ceiling(&self) -> bool {
        self.host_ceiling
            .is_some_and(|ceiling| self.built_at.elapsed() >= ceiling)
    }
}

impl Default for Clock {
    /// The host's own clock: a run with nothing said about its clock is bounded against the
    /// clock every invocation is bounded against.
    fn default() -> Self {
        Self::host()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many readings each row below takes, which is more than one so a clock that moves
    /// per reading is told apart from one that does not.
    const READINGS: u32 = 8;

    /// What one reading of each kind of clock answers — one row per kind, each read the same
    /// number of times, so the rows differ only along which clock they read.
    #[test]
    fn a_reading_moves_the_clock_its_own_kind_says_it_does() {
        // The tick a driven row is built with, and the one a still row's readings must not
        // move it by.
        let tick = Duration::from_millis(7);
        let cases: &[(&str, Clock, Option<Duration>)] = &[
            ("the host's own clock", Clock::host(), None),
            ("a clock that stands still", Clock::still(), Some(Duration::ZERO)),
            ("a clock its readings drive", Clock::driven(tick), Some(tick)),
        ];
        for (case, clock, per_reading) in cases {
            let first = clock.now();
            let mut taken = first;
            for reading in 1..=READINGS {
                let next = clock.now();
                assert!(
                    next >= taken,
                    "case {case}: reading {reading} went backwards, {next:?} after {taken:?}"
                );
                if let Some(per_reading) = per_reading {
                    let expected = first + per_reading.saturating_mul(reading);
                    assert_eq!(
                        next, expected,
                        "case {case}: expected reading {reading} at {expected:?}, got {next:?}"
                    );
                }
                taken = next;
            }
        }
    }

    /// A clock that stands still answers the SAME instant to every reader of it, which is
    /// what a run and the host's own watch over that run both read.
    #[test]
    fn a_still_clock_reads_the_same_instant_to_every_reader() {
        let clock = Clock::still();
        let taken = clock.now();
        let shared = clock.clone();
        let read_elsewhere = ::std::thread::spawn(move || shared.now())
            .join()
            .expect("the reading thread ended");
        assert_eq!(
            read_elsewhere, taken,
            "expected every reader of a still clock to read {taken:?}, got {read_elsewhere:?}"
        );
    }

    /// A clock states what a run bounded against it may take of THIS HOST's own time, and
    /// answers whether the run has taken it — one row per kind of clock and per ceiling,
    /// including the kinds that state none.
    #[test]
    fn a_clock_answers_whether_its_run_has_taken_the_host_time_it_may() {
        // (case, the clock, the span a run of it asks to wait for, whether the ceiling is
        // taken). A ceiling of nothing is already spent when the clock is built, and one of
        // an hour is not spent inside any run of this test. The row about THIS HOST's own
        // clock asks to wait for nothing, because a wait of that clock's time is a real one.
        let cases: &[(&str, Clock, Duration, bool)] = &[
            (
                "this host's own clock, which states no ceiling",
                Clock::host(),
                Duration::ZERO,
                false,
            ),
            (
                "a still clock with no ceiling stated",
                Clock::still(),
                Duration::from_secs(600),
                false,
            ),
            (
                "a driven clock with no ceiling stated",
                Clock::driven(Duration::from_millis(1)),
                Duration::from_secs(600),
                false,
            ),
            (
                "a driven clock whose ceiling is already spent",
                Clock::driven(Duration::from_millis(1)).within(Duration::ZERO),
                Duration::from_secs(600),
                true,
            ),
            (
                "a driven clock with an hour of host time to spend",
                Clock::driven(Duration::from_millis(1)).within(Duration::from_secs(3_600)),
                Duration::from_secs(600),
                false,
            ),
            (
                "a still clock whose ceiling is already spent",
                Clock::still().within(Duration::ZERO),
                Duration::from_secs(600),
                true,
            ),
        ];
        for (case, clock, asked, past) in cases {
            // The waits and readings a run takes are its OWN time: on a clock the host
            // drives they spend no host time, so they move this answer for no row.
            clock.wait(*asked);
            let _ = clock.now();
            assert_eq!(
                clock.past_host_ceiling(),
                *past,
                "case {case}: expected past_host_ceiling to answer {past}"
            );
        }
    }

    /// A WAIT spends the time of a clock the host drives by ADVANCING it, so what the run
    /// asked to wait for is time that clock has passed — one row per kind of such clock, and
    /// one per shape of what is asked for.
    #[test]
    fn a_wait_spends_the_time_of_a_clock_the_host_drives() {
        // (case, the clock, the spans asked for in turn, what they add up to)
        let cases: &[(&str, Clock, &[Duration], Duration)] = &[
            (
                "one span on a clock that stands still",
                Clock::still(),
                &[Duration::from_secs(600)],
                Duration::from_secs(600),
            ),
            (
                "many small spans on a clock that stands still",
                Clock::still(),
                &[
                    Duration::from_millis(250),
                    Duration::from_millis(250),
                    Duration::from_millis(500),
                ],
                Duration::from_secs(1),
            ),
            (
                "nothing asked for at all",
                Clock::still(),
                &[Duration::ZERO],
                Duration::ZERO,
            ),
            (
                "one span on a clock its readings drive",
                // A tick of nothing, so what the readings below add is nothing and the
                // reading answers the waits alone.
                Clock::driven(Duration::ZERO),
                &[Duration::from_secs(30)],
                Duration::from_secs(30),
            ),
        ];
        for (case, clock, asked, total) in cases {
            let first = clock.now();
            for span in *asked {
                clock.wait(*span);
            }
            let moved = clock.now().saturating_duration_since(first);
            assert_eq!(
                moved, *total,
                "case {case}: expected the waits to move the clock by {total:?}, got {moved:?}"
            );
        }
    }

    /// A wait is spent on the clock of EVERY holder of it, which is what the run's own thread
    /// and the host's watch over that run read.
    #[test]
    fn a_wait_is_spent_on_the_clock_every_reader_holds() {
        let clock = Clock::still();
        let first = clock.now();
        let span = Duration::from_secs(5);
        let shared = clock.clone();
        ::std::thread::spawn(move || shared.wait(span))
            .join()
            .expect("the waiting thread ended");
        let moved = clock.now().saturating_duration_since(first);
        assert_eq!(
            moved, span,
            "expected the wait taken elsewhere to move this clock by {span:?}, got {moved:?}"
        );
    }

    /// A driven clock is driven by EVERY reader of it: the readings a second holder takes
    /// move the clock the first holder reads, because a run and the watch over it spend one
    /// clock between them.
    #[test]
    fn a_driven_clock_is_moved_by_every_reader() {
        let tick = Duration::from_millis(3);
        let clock = Clock::driven(tick);
        let first = clock.now();
        let shared = clock.clone();
        ::std::thread::spawn(move || {
            let _ = shared.now();
        })
        .join()
        .expect("the reading thread ended");
        let after = clock.now();
        let expected = first + tick.saturating_mul(2);
        assert_eq!(
            after, expected,
            "expected the reading taken elsewhere to drive the clock to {expected:?}, got \
             {after:?}"
        );
    }
}
