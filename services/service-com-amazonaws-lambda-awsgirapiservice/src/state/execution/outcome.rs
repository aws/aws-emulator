// SPDX-License-Identifier: CC0-1.0

//! What one RUN of a handler is allowed to spend, and what it came to.
//!
//! An invocation's answer is the same shape whichever language the handler is written in —
//! the value it returned, the error it raised, the bound it ran past — so these are the
//! types both engines ([`super::python`], [`super::node`]) report in and the one
//! [`super::run_invocation`] reads. Keeping them here is what lets the operation's answer
//! be built once for every runtime rather than once per engine.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// What one invocation is allowed to spend.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    /// How long the handler may run for.
    pub wall_clock: Duration,
    /// How many traced events the handler may execute.
    pub steps: u64,
    /// How much memory the run may add to this process.
    pub memory_bytes: u64,
    /// How many bytes of the handler's output are kept.
    pub log_bytes: usize,
    /// How large the handler's response may be.
    pub response_bytes: usize,
    /// How deep the handler may recurse.
    pub recursion_limit: usize,
}

/// Which bound a stopped run ran past.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The run outlasted its wall clock.
    Deadline,
    /// The run executed more traced events than its budget.
    Steps,
    /// The run grew this process past its ceiling.
    Memory,
    /// The handler's response is larger than one may be.
    Response,
    /// The run took more of THIS HOST's own time than a run bounded against its clock may
    /// ([`super::run_clock::Clock::within`]).
    ///
    /// Not the run's own deadline: it is what bounds the host a run whose own time this
    /// host's clock does not move runs on, so a run stopped here is one this emulator gave up
    /// on rather than one whose own span ran out.
    HostCeiling,
}

impl Stop {
    /// What the stop is reported as, as the reason of the run's failure.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::Deadline => "the invocation ran past its timeout",
            Self::Steps => "the invocation ran past its step budget",
            Self::Memory => "the invocation ran past its memory ceiling",
            Self::Response => "the response is larger than the invocation may return",
            Self::HostCeiling => "the invocation took more host time than one run may take",
        }
    }
}

/// What the handler came to.
#[derive(Debug, Clone)]
pub enum Completion {
    /// The handler returned, and this is the JSON its value was serialized to.
    Returned(String),
    /// A RESPONSE-STREAMING handler finished, and these are the writes it made to its
    /// response stream, in the order it made them.
    ///
    /// Such a handler's answer is what it WROTE rather than what it returned, so the
    /// boundaries here are the handler's own writes — which is what a streaming caller
    /// reads as the stream's chunks.
    Streamed {
        /// The writes the handler made, in order.
        chunks: Vec<String>,
        /// The failure the handler came to once it had already written, when it failed;
        /// see [`StreamFailure`].
        failure: Option<StreamFailure>,
    },
    /// The handler (or the runtime resolving it) raised.
    Raised {
        /// The exception's type name, or the `Runtime.*` class of a runtime failure.
        error_type: String,
        /// The exception's message.
        error_message: String,
        /// The handler's own stack, one formatted frame per entry.
        stack_trace: Vec<String>,
    },
    /// The run was stopped by one of its bounds.
    Stopped(Stop),
    /// The interpreter itself could not be built or driven. Not a guest failure.
    Internal(String),
}

/// The failure a RESPONSE-STREAMING handler came to AFTER it had already written to its
/// response stream.
///
/// A stream that has carried bytes cannot be withdrawn, so the bytes already written stay
/// the answer and the failure is reported BESIDE them — which is what lets a streaming
/// caller tell a complete response from one cut short. A handler that failed before its
/// first write wrote nothing and carries no failure here: it delivered no bytes, and its
/// error is in the run's log alone.
#[derive(Debug, Clone)]
pub struct StreamFailure {
    /// The exception's type name.
    pub error_type: String,
    /// The exception's message.
    pub error_message: String,
    /// The handler's own stack, one formatted frame per entry.
    pub stack_trace: Vec<String>,
}

/// One finished run.
#[derive(Debug, Clone)]
pub struct Finished {
    /// What the handler came to.
    pub completion: Completion,
    /// Everything the handler wrote to its output streams, capped.
    pub log: String,
    /// How long the run took.
    pub elapsed: Duration,
}

/// What the handler wrote to its output streams, capped at the run's log budget.
#[derive(Debug)]
pub(super) struct Capture {
    /// The text kept so far.
    pub(super) text: String,
    /// How many bytes are kept at most.
    limit: usize,
}

impl Capture {
    /// A capture that keeps at most `limit` bytes.
    pub(super) fn new(limit: usize) -> Self {
        Self {
            text: String::new(),
            limit,
        }
    }

    /// Append `text`, stopping at the cap. Past it the capture keeps what it already has.
    pub(super) fn write(&mut self, text: &str) {
        if self.text.len() >= self.limit {
            return;
        }
        let room = self.limit - self.text.len();
        if text.len() <= room {
            self.text.push_str(text);
            return;
        }
        // Cut on a character boundary, so the kept log is still text.
        let mut end = room;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&text[..end]);
    }
}

/// Read the capture, which a panic in guest-driven code cannot poison out of use: what it
/// already holds is still the only output there is.
pub(super) fn captured(capture: &Arc<Mutex<Capture>>) -> MutexGuard<'_, Capture> {
    capture.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The log a run writes is kept up to the budget's cap and no further — one row per
    /// size class around it.
    #[test]
    fn the_capture_keeps_at_most_its_budget() {
        let cases: &[(usize, usize, usize)] = &[
            (16, 0, 0),
            (16, 8, 8),
            (16, 16, 16),
            (16, 17, 16),
            (16, 4_096, 16),
        ];
        for (limit, written, expected) in cases {
            let capture = Arc::new(Mutex::new(Capture::new(*limit)));
            captured(&capture).write(&"L".repeat(*written));
            let actual = captured(&capture).text.len();
            assert_eq!(
                actual, *expected,
                "case a {limit}-byte cap with {written} bytes written: expected {expected} \
                 bytes kept, got {actual}"
            );
        }
    }

    /// A multi-byte character is never cut in half by the cap: what is kept is still text.
    #[test]
    fn the_capture_cuts_on_a_character_boundary() {
        for limit in [0_usize, 1, 2, 3, 4, 5] {
            let capture = Arc::new(Mutex::new(Capture::new(limit)));
            // Three bytes per character, so every cap but a multiple of three has to cut
            // short of it.
            captured(&capture).write("☃☃");
            let kept = captured(&capture).text.clone();
            assert!(
                kept.chars().all(|character| character == '☃'),
                "case a {limit}-byte cap: expected whole characters, got {kept:?}"
            );
            assert!(
                kept.len() <= limit,
                "case a {limit}-byte cap: expected at most {limit} bytes, got {}",
                kept.len()
            );
        }
    }
}
