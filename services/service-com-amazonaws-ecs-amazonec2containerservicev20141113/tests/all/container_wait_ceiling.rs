// SPDX-License-Identifier: CC0-1.0

//! How long ONE wait a container's program names may spend of this emulator's own wall clock.
//!
//! RULE under test: **a wait a container's program names is spent — the program has not ended
//! while one of its waits is outstanding, so neither has the container — but the wall clock one
//! wait may spend on the caller's own seconds is bounded by this emulator and never by the
//! request: the bound is ORDERED by the seconds the wait named, never below the span this
//! emulator gives a standalone task's work (`state::CONTAINER_WORK_SPAN`), never above a cap
//! stated in those spans, and never above the wait itself.** That is what lets a client observe
//! such a task `RUNNING` and still reach the verdict behind it — the task's terminal status, the
//! dependents held until the container ends, the next job an AWS Batch queue admits once this
//! one releases the capacity or the consumable quantity it holds — inside a budget of its own,
//! for SEVERAL such containers run one after another and not only for the first.
//!
//! The reading itself is `state::wait_ceiling`, which is a pure function of the seconds the wait
//! named, so this suite asserts it directly: the end-to-end consequences are pinned where the
//! waits are really performed ([`crate::container_program_waits`] for a task that is still
//! running while its program waits, [`crate::container_stated_waits`] for what the program does
//! once a brief wait is over, and
//! `core/router/tests/all/batch_consumable_resource_caps_running_fargate_jobs.rs` for the
//! serialised client this bound exists for).
//!
//! The rows span what the rule generalises over: a wait SHORTER than the work span, one exactly
//! at it, one just above it, the tens-of-seconds band a batch workload is written in, and one
//! far beyond the cap — plus the two invariants over a sweep of the whole range (never longer
//! than the wait named, never longer than the cap, and never shorter for a longer wait).

use std::time::Duration;

use service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::{
    wait_ceiling, CONTAINER_WORK_SPAN,
};

/// The cap the reading holds every wait under: the documented number of work spans.
///
/// Written out here rather than imported, so a change to the cap is a change this suite reports
/// instead of one it follows.
const CAP_WORK_SPANS: u32 = 8;

/// One row: the seconds a wait names, and the wall clock the reading gives it.
struct Row {
    case: &'static str,
    /// What the program's `sleep` named, in milliseconds.
    stated_millis: u64,
    /// What one wait of it may spend, in milliseconds.
    spends_millis: u64,
}

/// The table. The expectations are computed from the documented reading — a third of the seconds
/// named, held between one work span and the cap, and never above the wait itself — not read
/// back off the implementation.
const ROWS: &[Row] = &[
    Row {
        // A wait of nothing is no wait: there is nothing to bound.
        case: "a wait of no seconds at all",
        stated_millis: 0,
        spends_millis: 0,
    },
    Row {
        // Below the floor: a wait of a second costs a second, because the bound is a CEILING
        // and never a stretch.
        case: "a wait of one second",
        stated_millis: 1_000,
        spends_millis: 1_000,
    },
    Row {
        case: "a wait of exactly one work span",
        stated_millis: 3_000,
        spends_millis: 3_000,
    },
    Row {
        // Above the floor but below three work spans: still the floor, never more than the wait.
        case: "a wait of six seconds",
        stated_millis: 6_000,
        spends_millis: 3_000,
    },
    Row {
        case: "a wait of thirty seconds",
        stated_millis: 30_000,
        spends_millis: 10_000,
    },
    Row {
        // The band a dwelling batch workload is written in. The seconds are divisible by the
        // divisor, so the expectation is an exact number of milliseconds rather than a
        // repeating one.
        case: "a wait of forty-two seconds",
        stated_millis: 42_000,
        spends_millis: 14_000,
    },
    Row {
        case: "a wait of forty-five seconds",
        stated_millis: 45_000,
        spends_millis: 15_000,
    },
    Row {
        case: "a wait of a minute",
        stated_millis: 60_000,
        spends_millis: 20_000,
    },
    Row {
        // At the cap: three times the cap in seconds named.
        case: "a wait of two minutes",
        stated_millis: 120_000,
        spends_millis: 24_000,
    },
    Row {
        case: "a wait of an hour",
        stated_millis: 3_600_000,
        spends_millis: 24_000,
    },
    Row {
        case: "a wait of a day",
        stated_millis: 86_400_000,
        spends_millis: 24_000,
    },
];

/// RULE: the wall clock one named wait may spend is this emulator's reading of the seconds it
/// named — ordered by them, floored at a standalone task's work span, capped, and never above
/// the wait itself.
#[test]
fn one_named_wait_spends_the_span_this_emulator_reads_its_seconds_as() {
    let floor = CONTAINER_WORK_SPAN;
    let cap = CONTAINER_WORK_SPAN * CAP_WORK_SPANS;
    for row in ROWS {
        let case = row.case;
        let stated = Duration::from_millis(row.stated_millis);
        let spends = wait_ceiling(stated);
        assert_eq!(
            spends,
            Duration::from_millis(row.spends_millis),
            "case {case:?}: expected one wait of {stated:?} to spend \
             {}ms, got {spends:?}",
            row.spends_millis
        );
        assert!(
            spends <= stated,
            "case {case:?}: a wait may never spend more than it named, got {spends:?} for \
             {stated:?}"
        );
        assert!(
            spends <= cap,
            "case {case:?}: no wait may spend more than the cap {cap:?}, got {spends:?}"
        );
        assert!(
            spends >= floor.min(stated),
            "case {case:?}: a wait at or above one work span spends at least {floor:?}, got \
             {spends:?}"
        );
    }
}

/// RULE, as an invariant over the whole range rather than over the table alone: the reading
/// never answers more than the wait named or more than the cap, and never answers LESS for a
/// longer wait — which is what makes a container written to hold a slot for longer hold it for
/// longer.
#[test]
fn the_reading_is_bounded_and_never_falls_as_the_wait_grows() {
    let cap = CONTAINER_WORK_SPAN * CAP_WORK_SPANS;
    // A sweep from nothing to well past the cap, in steps far finer than the work span, so a
    // reading that fell anywhere inside the ordered band is caught.
    let mut previous = Duration::ZERO;
    for step in 0..=1_000_u64 {
        let stated = Duration::from_millis(step * 200);
        let spends = wait_ceiling(stated);
        assert!(
            spends <= stated,
            "case a wait of {stated:?}: may never spend more than it named, got {spends:?}"
        );
        assert!(
            spends <= cap,
            "case a wait of {stated:?}: may never spend more than the cap {cap:?}, got {spends:?}"
        );
        assert!(
            spends >= previous,
            "case a wait of {stated:?}: a longer wait may never spend less, got {spends:?} \
             after {previous:?}"
        );
        previous = spends;
    }
    assert_eq!(
        previous, cap,
        "the sweep must reach the cap {cap:?}, got {previous:?}"
    );
}
