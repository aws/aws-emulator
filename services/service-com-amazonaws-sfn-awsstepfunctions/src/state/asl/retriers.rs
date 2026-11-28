// SPDX-License-Identifier: CC0-1.0

//! The `Retry` and `Catch` of a state, matched by the name a failure is reported under:
//! which retrier answers for an error and how long the attempt after it waits, and which
//! of a state's catchers — or of the concurrent states around it — takes the execution on.
//!
//! One reading of those rules for the whole interpreter ([`super`]): the walk asks them for
//! the work it performs itself, and the store asks the same ones for a task a worker or the
//! clock closed, so neither half of a state's error handling answers for a failure the other
//! would not.

use super::*;

/// A retrier's numeric member, when it is a finite, non-negative number.
///
/// A member that is absent, is not a number, or is negative or non-finite leaves the
/// retrier on its documented default rather than reaching the arithmetic below.
pub(super) fn finite_seconds(retrier: &Map<String, Value>, member: &str) -> Option<f64> {
    retrier
        .get(member)
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite() && *number >= 0.0)
}

/// How long a retrier waits before the attempt after `taken` ones:
/// "IntervalSeconds" scaled by "BackoffRate" once per attempt already taken, capped
/// by `MaxDelaySeconds` where the retrier declares one, and — where it asks for full
/// jitter — drawn uniformly from zero up to that capped figure
/// ([`jittered_seconds`]).
///
/// The figure that arithmetic reaches is TRUNCATED to whole seconds: `BackoffRate` is
/// "a number which is >= 1.0", so the product of an integer interval and it is a
/// fraction of a second as often as not, and the service waits the whole seconds of it
/// — the fraction is dropped rather than rounded up, so a wait is never longer than the
/// figure the retrier's own members reached. The band full jitter then draws from is
/// that truncated figure, since what the strategy spreads is the wait the retrier owes.
///
/// The product is of two numbers the definition chose, so it can overflow to infinity
/// (or to NaN, when a zero interval meets an infinite backoff). A wait no longer than
/// the longest one this service waits for is what such a retrier gets: the figure
/// this returns is always finite and in range, which is what makes converting it to a
/// `Duration` total. An attempt count past `i32::MAX` — which `MaxAttempts` and the
/// step budget both make unreachable — scales to that same maximum rather than to the
/// bare interval.
pub(super) fn retry_delay_seconds(retrier: &Map<String, Value>, taken: u64) -> f64 {
    let interval =
        finite_seconds(retrier, "IntervalSeconds").unwrap_or(DEFAULT_RETRY_INTERVAL_SECONDS);
    let backoff = finite_seconds(retrier, "BackoffRate").unwrap_or(DEFAULT_RETRY_BACKOFF_RATE);
    let mut delay = interval * backoff.powi(i32::try_from(taken).unwrap_or(i32::MAX));
    if let Some(cap) = finite_seconds(retrier, "MaxDelaySeconds") {
        delay = delay.min(cap);
    }
    let computed = if delay.is_finite() {
        delay.clamp(0.0, MAX_WAIT_SECONDS as f64)
    } else {
        MAX_WAIT_SECONDS as f64
    };
    // The whole seconds of the figure the retrier's own members reached: a computed
    // wait of 7.5 seconds is waited as 7, and one shorter than a second is not waited
    // at all.
    let computed = computed.trunc().max(0.0);
    if jitters_fully(retrier) {
        jittered_seconds(computed)
    } else {
        computed
    }
}

/// Whether `retrier` asks for full jitter on the waits between its attempts, which is
/// what its `JitterStrategy` names ([`JITTER_STRATEGY_FULL`]).
///
/// A retrier that declares no strategy — or any other one — keeps the computed wait,
/// because `NONE` is the member's documented default.
pub(super) fn jitters_fully(retrier: &Map<String, Value>) -> bool {
    retrier
        .get("JitterStrategy")
        .and_then(Value::as_str)
        .is_some_and(|strategy| strategy == JITTER_STRATEGY_FULL)
}

/// One wait drawn uniformly from `[0, computed]` seconds, which is what a retrier
/// asking for full jitter waits instead of `computed` itself.
///
/// Jitter only ever SHORTENS a wait: the draw is bounded above by the figure the
/// retrier's own arithmetic reached, so a capped delay stays capped. A `computed` of
/// zero — or one that is somehow no usable number — draws nothing, since there is no
/// span to spread the wait over.
pub(super) fn jittered_seconds(computed: f64) -> f64 {
    if !computed.is_finite() || computed <= 0.0 {
        return 0.0;
    }
    // The 53 bits a double carries exactly, as a fraction in [0, 1).
    let fraction = (jitter_draw() >> 11) as f64 / (1_u64 << 53) as f64;
    (computed * fraction).clamp(0.0, computed)
}

/// The counter behind [`jittered_seconds`]' draws: what makes two draws in the same
/// nanosecond differ, exactly as it does for every id the store mints.
pub(super) static JITTER_DRAWS: AtomicU64 = AtomicU64::new(0);

/// One draw from the generator every value this service samples comes from
/// ([`super::super::splitmix64`]), seeded from the process clock mixed with the count of
/// draws already made. The draw spreads waits apart rather than keeping anything
/// secret, so the mixer's own non-cryptographic quality is what it needs.
pub(super) fn jitter_draw() -> u64 {
    let drawn = JITTER_DRAWS.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    let mut seed = super::super::now_nanos() ^ drawn.rotate_left(29);
    super::super::splitmix64(&mut seed)
}

/// The state named `state_name` ANYWHERE in `document`, as the Amazon States Language
/// scopes a state name: "State names must be unique within the scope of the whole state
/// machine", so one name names one state however deeply the state lists nest.
///
/// The search descends into the nested state machines a state declares, through the one
/// statement of which members carry one ([`NESTED_MACHINE_MEMBERS`]) — a member naming ONE
/// machine carries it as an object and `Branches` carries a list of them — so a reader
/// asking what a state DECLARES finds the state a task of a `Parallel` branch belongs to
/// exactly as it finds one at the top level. Without that, a task scheduled inside a
/// branch would be judged against no `Retry` and no `Catch` at all.
///
/// `pub(in crate::state)`, not private to this module: it is the ONE such reader of this
/// crate, which the series that names the work a state's event belongs to reads through
/// too ([`super::observability`]).
///
/// `left` is how many levels further it may descend, which bounds the recursion by the
/// nesting a definition may be RUN at ([`MAX_NESTING_DEPTH`]): the document is a caller's,
/// so a definition nesting past what any walk of it would run is not descended into here
/// either — no state past that depth is reachable at run time anyway.
///
/// The FIRST state of that name is the answer, which the uniqueness rule above is what
/// makes unambiguous: within one state list the names are the keys of a JSON object, so
/// they cannot repeat, and across nested lists it is the definition's own obligation. A
/// definition that breaks it has one of its same-named states read here rather than both.
pub(in crate::state) fn declared_state<'a>(
    document: &'a Value,
    state_name: &str,
    left: usize,
) -> Option<&'a Map<String, Value>> {
    let states = document.get("States").and_then(Value::as_object)?;
    if let Some(state) = states.get(state_name).and_then(Value::as_object) {
        return Some(state);
    }
    let left = left.checked_sub(1)?;
    for state in states.values() {
        let nested = NESTED_MACHINE_MEMBERS
            .iter()
            .filter_map(|member| state.get(*member))
            .flat_map(|value| match value.as_array() {
                Some(machines) => machines.iter().collect::<Vec<_>>(),
                None => vec![value],
            });
        for machine in nested {
            if let Some(found) = declared_state(machine, state_name, left) {
                return Some(found);
            }
        }
    }
    None
}

/// Which retrier of the `Task` state `state_name` of `definition` retries an activity
/// task that failed, and how long after that failure its next attempt is due.
///
/// `errors` is every error name that failure answers to — one name for what a worker
/// reported, and both names the language states for a closure of the task's clock
/// ([`task_closure_errors`]).
///
/// `taken` is how many attempts each retrier of that state has already spent on the
/// task, in the order the state declares them. The answer is the FIRST retrier whose
/// `ErrorEquals` names one of those errors, exactly as a retried walk's is, so one rule
/// decides every retry this service makes.
///
/// `None` when the definition declares no such state, when the state declares no
/// `Retry`, when no retrier names the error, or when the one that does has spent its
/// `MaxAttempts`: the failure is then the state's own.
pub fn activity_retry_any(
    definition: &str,
    state_name: &str,
    errors: &[&str],
    taken: &[u64],
) -> Option<(usize, Duration)> {
    let document = serde_json::from_str::<Value>(definition).ok()?;
    let state = declared_state(&document, state_name, MAX_NESTING_DEPTH)?;
    let retriers = state.get("Retry").and_then(Value::as_array)?;
    for (index, retrier) in retriers.iter().enumerate() {
        let Some(retrier) = retrier.as_object() else {
            continue;
        };
        if !errors_match_any(retrier, errors) {
            continue;
        }
        let max_attempts = retrier
            .get("MaxAttempts")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_RETRY_MAX_ATTEMPTS);
        let spent = taken.get(index).copied().unwrap_or(0);
        if spent >= max_attempts {
            return None;
        }
        return Some((
            index,
            // The figure is already finite and inside [`MAX_WAIT_SECONDS`], whatever a
            // definition writes in the retrier's own members; the conversion is
            // nevertheless written so it CANNOT panic, because a caller-sized figure
            // must not be able to take the process down — a delay that somehow left
            // that range waits no time at all, exactly as the walk's own retry loop
            // converts it.
            Duration::try_from_secs_f64(retry_delay_seconds(retrier, spent))
                .unwrap_or(Duration::ZERO),
        ));
    }
    None
}

/// Which retrier retries a task that failed with ONE error name: the whole of what a
/// worker reports, and the one name every failure inside a walk carries
/// ([`activity_retry_any`] is the same rule over the several names one closure of a
/// task's clock answers to).
pub fn activity_retry(
    definition: &str,
    state_name: &str,
    error: &str,
    taken: &[u64],
) -> Option<(usize, Duration)> {
    activity_retry_any(definition, state_name, &[error], taken)
}

/// Whether any state on the path from `state_name` OUT to the top level of `definition`
/// catches a failure answering to any of `errors`: the state itself, then each concurrent
/// state it is nested inside, outermost last ([`ParkedFrame`]).
///
/// A failure no catcher of the state's own answers is the failure of the concurrent state
/// that encloses it — "if any branch fails, the entire Parallel state is considered to have
/// failed" — so a `Catch` on THAT state answers for it. The question a caller deciding
/// whether a task's answer needs a WALK has to ask is therefore about the whole path and
/// not about the state alone: a task inside a `Parallel` whose own `Catch` names the error
/// is a task whose answer the execution carries on from.
///
/// `frames` is the path, as the suspended task carries it
/// ([`ScheduledActivity::frames`]); empty for a task at the top level, where this is
/// [`state_catches_error`] on the state itself.
pub fn path_catches_error(
    definition: &str,
    state_name: &str,
    frames: &[ParkedFrame],
    errors: &[&str],
) -> bool {
    state_catches_error(definition, state_name, errors)
        || frames.iter().any(|frame| {
            // An enclosing state's own `Retry` re-runs its whole machine, which only a
            // walk can do, so either half of its answer needs the continuation.
            state_answers_error(definition, &frame.state_name, errors, &[])
        })
}

/// Whether the state `state_name` of `definition` ANSWERS a failure ITSELF: a `Retry`
/// whose retrier names it with an attempt left, or a `Catch` whose catcher names it.
///
/// "Task states […] can have a field named Catch. […] When a state reports an error and
/// it matches a catcher, the state's output is the error output" — so such a failure is
/// not the execution's: the state makes its work's next attempt, or the execution
/// carries on at the catcher's `Next`. A state that answers neither way fails the
/// execution with the error.
///
/// `errors` is every error name the failure answers to and `taken` how many attempts each
/// retrier of that state has already spent, both read exactly as [`activity_retry`] reads
/// them, and the matching is the one rule every retrier and catcher of this service is
/// decided by ([`errors_match`]) — so this answers the same question the walk itself
/// would, without walking.
pub fn state_answers_error(
    definition: &str,
    state_name: &str,
    errors: &[&str],
    taken: &[u64],
) -> bool {
    activity_retry_any(definition, state_name, errors, taken).is_some()
        || state_catches_error(definition, state_name, errors)
}

/// Whether the state `state_name` of `definition` CATCHES a failure: a `Catch` whose
/// catcher names any of `errors`.
///
/// The `Catch` half of [`state_answers_error`] on its own, for the caller that has
/// already answered the `Retry` half itself: an activity task's next attempt is a task
/// the store queues ([`super::ComAmazonawsSfnAwsstepfunctionsState::retry_failed_activity_task`]),
/// so only a catcher's branch is left needing a walk.
pub fn state_catches_error(definition: &str, state_name: &str, errors: &[&str]) -> bool {
    let Ok(document) = serde_json::from_str::<Value>(definition) else {
        return false;
    };
    let Some(state) = declared_state(&document, state_name, MAX_NESTING_DEPTH) else {
        return false;
    };
    retry_entries(state, "Catch")
        .iter()
        .filter_map(Value::as_object)
        .any(|catcher| errors_match_any(catcher, errors))
}

/// Whether a retrier's or catcher's `ErrorEquals` names ANY of `errors`: the names one
/// failure answers to, which is more than one where the language states more than one for
/// it ([`task_closure_errors`]).
pub(crate) fn errors_match_any(entry: &Map<String, Value>, errors: &[&str]) -> bool {
    errors.iter().any(|error| errors_match(entry, error))
}

/// Whether a retrier's or catcher's `ErrorEquals` names `error`.
///
/// "States.ALL — A wildcard that matches any known error name", with one documented
/// exception: "A States.Runtime error isn't retriable, and will always cause the
/// execution to fail. A retry or catch on States.ALL won't catch States.Runtime
/// errors." A definition that names `States.Runtime` itself does not catch it
/// either, because it is not an error the execution can recover from.
///
/// [`TASK_FAILED_ERROR`] is the OTHER wildcard the language documents: "States.TaskFailed
/// […] When used in a retry or catch, States.TaskFailed acts as a wildcard that matches
/// any known error name except States.Timeout" — so a retrier naming it takes over the
/// failure a task's own work reported, whatever that work named it, which is how a
/// definition retries a service call or a function that failed without naming the
/// service's error itself. The closure of a task's own clock is the one thing it does not
/// take over, under EITHER of the names the language states for that closure
/// ([`task_closure_errors`]): a heartbeat window that lapsed is a `States.Timeout` too.
pub(super) fn errors_match(entry: &Map<String, Value>, error: &str) -> bool {
    if error == RUNTIME_ERROR {
        return false;
    }
    let timeout = error == TIMEOUT_ERROR || error == HEARTBEAT_TIMEOUT_ERROR;
    entry
        .get("ErrorEquals")
        .and_then(Value::as_array)
        .is_some_and(|names| {
            names.iter().filter_map(Value::as_str).any(|name| {
                name == error || name == ALL_ERRORS || (name == TASK_FAILED_ERROR && !timeout)
            })
        })
}
