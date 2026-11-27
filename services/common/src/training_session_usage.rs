// SPDX-License-Identifier: CC0-1.0

//! The TRAINING-SESSION ACCOUNT: what a SageMaker training session has RECEIVED and
//! CONSUMED, written by the data plane that serves its operations and read by the control
//! plane that reports the session.
//!
//! A training session is created by the SageMaker control plane
//! (`CreateTrainingSession`), and the training and sampling operations submitted against
//! it are served by the SageMaker Training Session Runtime data plane. The control plane's
//! own read of a session (`DescribeTrainingSession`) nevertheless reports two ACCOUNTED
//! quantities of that traffic — the sequence number of the last training operation the
//! session received, and the token usage billed for it — which only the data plane
//! observes. The engine keys state by TYPE, so a copy of its own inside either service's
//! state would be invisible to the other: this is the ONE account both read.
//!
//! Like [`crate::agentic_rft_jobs`], nothing here names a wire shape. The account holds
//! counters; each reader projects them onto the members ITS own model declares.
//!
//! The quantities are COUNTED, never guessed: an account is opened zeroed when the control
//! plane creates the session (so a name reused after a delete starts from nothing), and
//! the data plane publishes what its own records add up to. A session nothing has submitted
//! against therefore reports zero — the value it has accounted — rather than no value at
//! all.
//!
//! The directory is BOUNDED ([`MAX_ACCOUNTED_SESSIONS`]) so a caller looping on fresh
//! session names cannot grow it without end; the oldest account is dropped first, which
//! keeps the sessions a live workflow just created accounted.

use std::collections::{HashMap, VecDeque};

/// How many training-session accounts the directory holds at once.
///
/// An EMULATOR ceiling, not an AWS quota: this directory lives in memory for the lifetime
/// of the process, so it needs a maximum count. It is an order of magnitude above the
/// count the SageMaker control plane's own create admits, so while that bound stands no
/// session the control plane still holds can lose its account; were it ever to admit more,
/// eviction drops the OLDEST account first, never a newer one.
pub const MAX_ACCOUNTED_SESSIONS: usize = 10_000;

/// What one training session has accounted so far.
///
/// Every field is a running total or a cursor the SERVICE keeps, so the zero value is the
/// state of a session nothing has been submitted against — a real value, not an absence.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionUsage {
    /// The sequence number of the last training operation the session RECEIVED, and `0`
    /// for a session that has received none: sequence numbers start at 1, so zero is the
    /// cursor of a session no training operation has reached.
    pub last_received_sequence_number: i32,
    /// Prefill tokens consumed by the session's sampling operations.
    pub prefill_tokens: i64,
    /// Tokens decoded (sampled) by the session's sampling operations.
    pub sample_tokens: i64,
    /// Tokens consumed by the session's training operations.
    pub training_tokens: i64,
}

/// The account directory itself: `trainingSessionName` -> what that session has accounted.
///
/// Registered in the shared engine under THIS type, so the control plane that creates and
/// reports a session and the data plane that serves its operations read one account
/// instead of each keeping its own.
#[derive(Debug, Default)]
pub struct TrainingSessionUsageDirectory {
    /// The accounts, by `trainingSessionName` — the only identifier every operation of
    /// this resource takes.
    sessions: HashMap<String, SessionUsage>,
    /// The order accounts were opened in, so the ceiling drops the oldest first.
    order: VecDeque<String>,
}

impl TrainingSessionUsageDirectory {
    /// Open a ZEROED account for the session `name`, as the control plane's create does.
    ///
    /// A name created again — after its session was deleted, or over an account the data
    /// plane opened for traffic submitted before the session existed — starts from
    /// nothing: the new session has received no operation and consumed no token, so the
    /// previous session's totals are not its own.
    pub fn open(&mut self, name: &str) {
        self.publish(name, SessionUsage::default());
    }

    /// Record what the session `name` has accounted, as the data plane that serves its
    /// operations counts it.
    ///
    /// The whole account is replaced rather than added to: the publisher derives these
    /// totals from its own records of the session, so what it publishes is the account,
    /// and publishing it twice leaves the same value.
    pub fn publish(&mut self, name: &str, usage: SessionUsage) {
        if self.sessions.insert(name.to_owned(), usage).is_none() {
            self.order.push_back(name.to_owned());
            while self.order.len() > MAX_ACCOUNTED_SESSIONS {
                if let Some(oldest) = self.order.pop_front() {
                    self.sessions.remove(&oldest);
                }
            }
        }
    }

    /// Drop the account of the session `name`: the session is gone, so nothing it received
    /// or consumed is accounted any longer.
    ///
    /// Belongs to the control plane's `DeleteTrainingSession`, which releases the session
    /// and the account with it. Opening an account zeroes it in any case, so a name created
    /// again never inherits the totals of the session that held it.
    pub fn forget(&mut self, name: &str) {
        if self.sessions.remove(name).is_some() {
            self.order.retain(|held| held != name);
        }
    }

    /// What the session `name` has accounted: the published totals, and the ZERO account
    /// for a session no account was ever opened or published for.
    ///
    /// Zero is what a session with no accounted traffic has accounted, so a reader needs
    /// no second rule for a session whose account is not held: both answer the same
    /// counters.
    #[must_use]
    pub fn usage(&self, name: &str) -> SessionUsage {
        self.sessions.get(name).copied().unwrap_or_default()
    }

    /// How many accounts the directory holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Whether the directory holds no account at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One published account, named.
    fn usage(sequence: i32, prefill: i64, sample: i64, training: i64) -> SessionUsage {
        SessionUsage {
            last_received_sequence_number: sequence,
            prefill_tokens: prefill,
            sample_tokens: sample,
            training_tokens: training,
        }
    }

    /// RULE: an account answers the totals last published for that session, and the zero
    /// account for a session nothing was published for.
    #[test]
    fn an_account_answers_what_was_published_for_its_own_session() {
        let mut directory = TrainingSessionUsageDirectory::default();
        directory.open("opened");
        directory.publish("busy", usage(7, 11, 13, 17));

        // (case, the session a reader asks about, the account it must answer)
        let rows = [
            ("a session opened and nothing published for", "opened", usage(0, 0, 0, 0)),
            ("a session with published totals", "busy", usage(7, 11, 13, 17)),
            ("a session no account was opened for", "unknown", usage(0, 0, 0, 0)),
        ];
        for (case, name, expected) in rows {
            let actual = directory.usage(name);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?} for {name}, got {actual:?}"
            );
        }
    }

    /// RULE: opening an account zeroes it, so a session name used again is accounted from
    /// nothing rather than inheriting what the previous session accounted.
    #[test]
    fn opening_an_account_again_zeroes_it() {
        let mut directory = TrainingSessionUsageDirectory::default();
        directory.publish("reused", usage(4, 5, 6, 7));
        directory.open("reused");

        assert_eq!(
            directory.usage("reused"),
            SessionUsage::default(),
            "case a reopened account: expected the zero account, got {:?}",
            directory.usage("reused")
        );
        assert_eq!(
            directory.len(),
            1,
            "case a reopened account: expected one account, got {}",
            directory.len()
        );
    }

    /// RULE: forgetting one session's account leaves every other session's standing, and
    /// forgetting an account the directory does not hold changes nothing.
    #[test]
    fn forgetting_an_account_drops_it_and_leaves_the_others() {
        let mut directory = TrainingSessionUsageDirectory::default();
        directory.publish("kept", usage(1, 0, 0, 0));
        directory.publish("dropped", usage(2, 0, 0, 0));

        directory.forget("dropped");
        directory.forget("never-created");

        assert_eq!(
            (directory.usage("kept"), directory.usage("dropped"), directory.len()),
            (usage(1, 0, 0, 0), SessionUsage::default(), 1),
            "case one account forgotten: expected only it to go, got {} account(s)",
            directory.len()
        );
    }

    /// RULE: the ceiling bounds the directory and drops the OLDEST account first; a
    /// republication is not a new account, so it moves nothing out.
    #[test]
    fn the_ceiling_drops_the_oldest_account_first() {
        let mut directory = TrainingSessionUsageDirectory::default();
        for index in 0..MAX_ACCOUNTED_SESSIONS {
            directory.publish(&format!("session{index}"), usage(1, 0, 0, 0));
        }
        directory.publish("session0", usage(2, 0, 0, 0));
        assert_eq!(
            directory.len(),
            MAX_ACCOUNTED_SESSIONS,
            "case a republication at the ceiling: expected {MAX_ACCOUNTED_SESSIONS} \
             accounts, got {}",
            directory.len()
        );

        directory.publish(&format!("session{MAX_ACCOUNTED_SESSIONS}"), usage(1, 0, 0, 0));
        assert_eq!(
            directory.len(),
            MAX_ACCOUNTED_SESSIONS,
            "case one past the ceiling: expected the ceiling held, got {}",
            directory.len()
        );
        assert_eq!(
            directory.usage("session0"),
            SessionUsage::default(),
            "case one past the ceiling: expected the oldest account dropped"
        );
        assert_eq!(
            directory.usage("session1"),
            usage(1, 0, 0, 0),
            "case one past the ceiling: expected the newer accounts kept"
        );
    }
}
