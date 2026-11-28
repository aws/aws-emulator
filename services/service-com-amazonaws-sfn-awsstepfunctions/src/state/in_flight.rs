// SPDX-License-Identifier: CC0-1.0

//! What an execution still has IN FLIGHT: the work its run has not been reported as
//! finishing, which is the work a stop catches.
//!
//! # The rule this module implements
//!
//! A run that has LEFT the state it ends at has nothing left to do — the execution's own
//! outcome is what happens next, at that same instant — so a service that has not reported
//! an outcome yet has not reported that state as left either: the exit of the state a run
//! ends at is reported WITH the outcome ([`reportable_before_outcome`]), and until then
//! that state is work in flight. Every other event the run wrote is reported from its own
//! instant on ([`super::Execution::advance_history`]), so what a caller reads is still a
//! prefix of the whole history.
//!
//! What STOPS such a run therefore catches the states it was still inside, and the history
//! of a stopped execution is TRUNCATED at them: each was entered and is never left, and
//! the stop writes no record of its own about them ([`super::store::Store::stop_execution`])
//! — the end of the EXECUTION is the one abort such a history holds. What tells a state a
//! stop caught from one that finished is the exit the history does not hold, which is what
//! a reader of the history — and of the trace built from it ([`super::xray_tracing`]) —
//! attributes the stop to.

use super::{HistoryEvent, STATE_EXITED_SUFFIX};

/// How many of the events an owed outcome carries are reported BEFORE that outcome is.
///
/// Every one of them but two. The event the attempt ENDS on is the outcome itself — it is
/// stamped at the instant the outcome comes due, so it is never reported sooner. And the
/// EXIT of the state the run ends at is reported with it: a run that has left that state
/// has done all its work, so reporting the exit any sooner would report a run that has
/// finished and reached no outcome, which is not a state an execution is ever in.
///
/// Zero for an attempt that wrote nothing but its own outcome.
pub(super) fn reportable_before_outcome(events: &[HistoryEvent]) -> usize {
    // The last event is the one the attempt ends on: the walk stamps it at `due_at` and
    // records it last ([`super::plan`]).
    let Some((_outcome, walked)) = events.split_last() else {
        return 0;
    };
    let left_the_state_it_ends_at = walked
        .last()
        .is_some_and(|event| event.event_type.ends_with(STATE_EXITED_SUFFIX));
    walked.len() - usize::from(left_the_state_it_ends_at)
}
