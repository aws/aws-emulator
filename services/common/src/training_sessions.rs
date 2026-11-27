// SPDX-License-Identifier: CC0-1.0

//! The TRAINING SESSION DIRECTORY: which SageMaker training sessions the control plane
//! holds, so the data plane that serves their operations can resolve a name.
//!
//! A training session is CREATED by the SageMaker control plane
//! (`CreateTrainingSession`) and USED by the SageMaker Training Session Runtime data
//! plane (`ExecuteTrainingOperation`, `ExecuteSamplingOperation`,
//! `SampleTrainingSession`, `SampleTrainingSessionWithResponseStream`), whose own API
//! models no create at all: a runtime request names a session and nothing else. So the
//! data plane can only tell a session of this account from a name that was never created
//! because the control plane RECORDED it.
//!
//! This is that record: ONE directory in the shared engine, written by the control plane
//! when it creates a session, when it stops one (and cleared when the session is deleted),
//! read by the data plane on every call. Nothing here names a wire shape — the directory
//! holds the session's name, the ARN the control plane minted for it, where it is in its
//! LIFECYCLE and the base model it adapts, and each reader projects that onto the members
//! ITS own model declares.
//!
//! The lifecycle is here because a session only accepts work in some of its states, and
//! the state is the control plane's to decide: a status the control plane reaches on a
//! clock-owed instant is published as the status the session is in NOW plus the transition
//! the clock owes next ([`TrainingSessionLifecycle`]), so a reader materialises the same
//! status the control plane would without writing anything while it reads.
//!
//! Like [`crate::feature_store_groups`], a name this directory does not hold is a FACT
//! and not a guess: a session exists only because `CreateTrainingSession` minted it, so
//! the data plane refuses a name with no registration rather than materialising one. The
//! directory is BOUNDED all the same ([`MAX_REGISTERED_TRAINING_SESSIONS`]) so a caller
//! looping on create cannot grow it without end; the oldest registration is dropped
//! first, which keeps the sessions a live workflow just created resolvable.
//!
//! It is the companion of [`crate::training_session_usage`], which accounts what a
//! session has RECEIVED and CONSUMED: that one answers "how much", this one "does it
//! exist at all", and a zero account is a real value for a session nothing has been
//! submitted against — which is why existence cannot be read off it.

use std::collections::{HashMap, VecDeque};
use std::time::SystemTime;

/// How many training sessions the directory holds at once.
///
/// An EMULATOR ceiling, not an AWS quota: this directory lives in memory for the lifetime
/// of the process, so it needs a maximum count. It is an order of magnitude above the
/// count the SageMaker control plane's own `CreateTrainingSession` admits (its
/// `TRAINING_SESSION_QUOTA`, which that handler refuses past with
/// `ResourceLimitExceeded`), so while that bound stands no session the control plane still
/// holds can be evicted from here. Were it ever to admit more, eviction drops the OLDEST
/// registration first — never a newer one — so the sessions a live workflow just created
/// stay resolvable and only the longest-standing ones could become unresolvable to the
/// data plane.
pub const MAX_REGISTERED_TRAINING_SESSIONS: usize = 10_000;

/// One lifecycle transition the clock owes a training session: the status the session
/// moves to, and the instant that move falls due.
#[derive(Debug, Clone)]
pub struct OwedStatusTransition {
    /// A `TrainingSessionStatus` value.
    pub status: String,
    /// The instant the session reaches it, whether or not anybody looks.
    pub due_at: SystemTime,
}

/// Where a training session is in its lifecycle, as the control plane published it.
///
/// A session accepts new operations only in some of its states, so the status is what the
/// data plane asks the directory about beside existence. Two of the control plane's own
/// transitions are owed by the CLOCK rather than by a call — the compute coming up after a
/// create, and a stop quiescing — so a published lifecycle carries the status the session
/// is in now and the transition the clock owes next: [`Self::status_at`] applies it, which
/// makes the reading a pure function of the published record and the instant asked about.
#[derive(Debug, Clone)]
pub struct TrainingSessionLifecycle {
    /// The `TrainingSessionStatus` the session was in when this record was published.
    status: String,
    /// The transition the clock owes it next, and `None` when the clock owes none.
    owed: Option<OwedStatusTransition>,
}

impl TrainingSessionLifecycle {
    /// A session in `status` that the clock owes no transition out of.
    #[must_use]
    pub fn settled(status: impl Into<String>) -> Self {
        Self {
            status: status.into(),
            owed: None,
        }
    }

    /// A session in `status` that the clock moves to `owed` at `due_at`.
    #[must_use]
    pub fn owing(status: impl Into<String>, owed: impl Into<String>, due_at: SystemTime) -> Self {
        Self {
            status: status.into(),
            owed: Some(OwedStatusTransition {
                status: owed.into(),
                due_at,
            }),
        }
    }

    /// The `TrainingSessionStatus` the session is in at `now`: the owed transition applied
    /// once the clock has reached its due instant, and the published status before that.
    ///
    /// Nothing is stored, so what one reader observes never depends on whether another
    /// read first.
    #[must_use]
    pub fn status_at(&self, now: SystemTime) -> &str {
        match &self.owed {
            Some(owed) if now >= owed.due_at => owed.status.as_str(),
            _ => self.status.as_str(),
        }
    }
}

/// One training session the control plane created, as the directory records it.
#[derive(Debug, Clone)]
pub struct TrainingSessionRecord {
    /// The `TrainingSessionName` the session was created with, which is also this
    /// record's key in the directory.
    pub name: String,
    /// The `TrainingSessionArn` the control plane minted for it.
    pub arn: String,
    /// Where the session is in its lifecycle, as the control plane published it.
    pub lifecycle: TrainingSessionLifecycle,
    /// The base model the session adapts, as the create recorded it
    /// (`TrainingSessionModelConfig$BaseModelArn`): it is the model the data plane's
    /// inference and sampling operations are served by, so what that model can hold is a
    /// fact about the session rather than about either plane.
    pub base_model_arn: String,
}

/// The directory itself: `trainingSessionName` -> the session the control plane holds.
///
/// Registered in the shared engine under THIS type, so the control plane that creates a
/// session and the data plane that serves its operations read one directory instead of
/// each keeping its own copy.
#[derive(Debug, Default)]
pub struct TrainingSessionDirectory {
    /// The records, by training session name.
    sessions: HashMap<String, TrainingSessionRecord>,
    /// Registration order, so the ceiling drops the oldest entry first.
    order: VecDeque<String>,
}

impl TrainingSessionDirectory {
    /// Record `session` under its own name, as the control plane's create does.
    ///
    /// Re-registering a name REPLACES its record — the control plane correcting what it
    /// told the directory — and does not move its place in the eviction order.
    pub fn register(&mut self, session: TrainingSessionRecord) {
        let name = session.name.clone();
        if self.sessions.insert(name.clone(), session).is_none() {
            self.order.push_back(name);
            while self.order.len() > MAX_REGISTERED_TRAINING_SESSIONS {
                if let Some(oldest) = self.order.pop_front() {
                    self.sessions.remove(&oldest);
                }
            }
        }
    }

    /// Drop the record of the session `name`: the session is gone, so the directory no
    /// longer claims it exists.
    ///
    /// Belongs to the control plane's `DeleteTrainingSession`, which releases the session
    /// and everything it published about it; registering a name again replaces its record
    /// in any case.
    pub fn forget(&mut self, name: &str) {
        if self.sessions.remove(name).is_some() {
            self.order.retain(|held| held != name);
        }
    }

    /// The session `name` names, or `None` when no session of that name was created.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&TrainingSessionRecord> {
        self.sessions.get(name)
    }

    /// Whether the control plane holds a session called `name` — the first question the
    /// data plane asks before it serves a call against it.
    #[must_use]
    pub fn holds(&self, name: &str) -> bool {
        self.sessions.contains_key(name)
    }

    /// The `TrainingSessionStatus` the session `name` is in at `now`, or `None` when no
    /// session of that name was created — the second question, because a session accepts
    /// new operations only in some of its states.
    #[must_use]
    pub fn status_at(&self, name: &str, now: SystemTime) -> Option<&str> {
        Some(self.sessions.get(name)?.lifecycle.status_at(now))
    }

    /// How many training sessions the directory holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Whether the directory holds no training session at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    /// One registration, named, of a session whose compute is already up.
    fn session(name: &str) -> TrainingSessionRecord {
        TrainingSessionRecord {
            name: name.to_owned(),
            arn: format!("arn:aws:sagemaker:us-east-1:000000000000:training-session/{name}"),
            lifecycle: TrainingSessionLifecycle::settled("Running"),
            base_model_arn: format!(
                "arn:aws:sagemaker:us-east-1:aws:hub-content/SageMakerPublicHub/Model/{name}-base/1.0.0"
            ),
        }
    }

    /// RULE: a published lifecycle answers the status the session is in at the instant it
    /// is asked about — the status it was published in until the clock reaches the owed
    /// transition's due instant, and the owed status from that instant on.
    #[test]
    fn a_published_lifecycle_applies_the_transition_the_clock_owes() {
        let due_at = SystemTime::UNIX_EPOCH + Duration::from_secs(300);
        // (case, the lifecycle as published, the instant asked about, the status owed)
        let rows = [
            (
                "a settled status, long before the due instant",
                TrainingSessionLifecycle::settled("Stopped"),
                SystemTime::UNIX_EPOCH,
                "Stopped",
            ),
            (
                "a settled status, long after it",
                TrainingSessionLifecycle::settled("Stopped"),
                due_at + Duration::from_secs(3_600),
                "Stopped",
            ),
            (
                "an owed transition, one second before it falls due",
                TrainingSessionLifecycle::owing("Created", "Running", due_at),
                due_at - Duration::from_secs(1),
                "Created",
            ),
            (
                "an owed transition, at the instant it falls due",
                TrainingSessionLifecycle::owing("Created", "Running", due_at),
                due_at,
                "Running",
            ),
            (
                "an owed transition, well past it",
                TrainingSessionLifecycle::owing("Stopping", "Stopped", due_at),
                due_at + Duration::from_secs(3_600),
                "Stopped",
            ),
        ];
        for (case, lifecycle, at, expected) in rows {
            let actual = lifecycle.status_at(at);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the status {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the directory answers the status of a session it holds and nothing for a name
    /// it does not hold, so a reader cannot mistake "never created" for a state.
    #[test]
    fn only_a_registered_session_has_a_status() {
        let mut directory = TrainingSessionDirectory::default();
        let due_at = SystemTime::UNIX_EPOCH + Duration::from_secs(60);
        directory.register(TrainingSessionRecord {
            lifecycle: TrainingSessionLifecycle::owing("Created", "Running", due_at),
            ..session("provisioning")
        });

        assert_eq!(
            directory.status_at("provisioning", SystemTime::UNIX_EPOCH),
            Some("Created"),
            "case a session still provisioning: expected the status it was published in"
        );
        assert_eq!(
            directory.status_at("provisioning", due_at),
            Some("Running"),
            "case the compute coming up: expected the status the clock owed"
        );
        assert_eq!(
            directory.status_at("never-created", due_at),
            None,
            "case a name nothing created: expected no status at all"
        );
    }

    /// RULE: the directory holds exactly the sessions that were registered, so a name
    /// nothing registered resolves to nothing.
    #[test]
    fn only_a_registered_session_resolves() {
        let mut directory = TrainingSessionDirectory::default();
        directory.register(session("created"));

        // (case, the name a reader asks about, whether the directory holds it)
        let rows = [
            ("a session the control plane created", "created", true),
            ("a name nothing created", "never-created", false),
            ("a name differing only in case", "Created", false),
            ("the empty name", "", false),
        ];
        for (case, name, expected) in rows {
            let actual = directory.holds(name);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected holds({name:?}) to be {expected}, got {actual}"
            );
            assert_eq!(
                directory.get(name).is_some(),
                expected,
                "case {case:?}: expected get({name:?}).is_some() to be {expected}"
            );
        }
        assert_eq!(
            directory.get("created").map(|held| held.arn.as_str()),
            Some(session("created").arn.as_str()),
            "case a registered session: expected the ARN the control plane minted"
        );
    }

    /// RULE: a forgotten session no longer resolves, every other registration stands, and
    /// forgetting a name the directory does not hold changes nothing.
    #[test]
    fn forgetting_one_session_leaves_the_others() {
        let mut directory = TrainingSessionDirectory::default();
        directory.register(session("kept"));
        directory.register(session("dropped"));

        directory.forget("dropped");
        directory.forget("never-created");

        assert_eq!(
            (
                directory.holds("kept"),
                directory.holds("dropped"),
                directory.len()
            ),
            (true, false, 1),
            "case one session forgotten: expected only it to go, got {} session(s)",
            directory.len()
        );
    }

    /// RULE: the ceiling bounds the directory and drops the OLDEST registration first; a
    /// re-registration is not a new entry, so it moves nothing out.
    #[test]
    fn the_ceiling_drops_the_oldest_registration_first() {
        let mut directory = TrainingSessionDirectory::default();
        assert!(
            directory.is_empty(),
            "case a fresh directory: expected it to hold nothing"
        );
        for index in 0..MAX_REGISTERED_TRAINING_SESSIONS {
            directory.register(session(&format!("session{index}")));
        }
        directory.register(session("session0"));
        assert_eq!(
            directory.len(),
            MAX_REGISTERED_TRAINING_SESSIONS,
            "case a re-registration at the ceiling: expected \
             {MAX_REGISTERED_TRAINING_SESSIONS} sessions, got {}",
            directory.len()
        );

        directory.register(session(&format!(
            "session{MAX_REGISTERED_TRAINING_SESSIONS}"
        )));
        assert_eq!(
            directory.len(),
            MAX_REGISTERED_TRAINING_SESSIONS,
            "case one past the ceiling: expected the ceiling held, got {}",
            directory.len()
        );
        assert!(
            !directory.holds("session0"),
            "case one past the ceiling: expected the oldest registration dropped"
        );
        assert!(
            directory.holds("session1"),
            "case one past the ceiling: expected the newer registrations kept"
        );
    }
}
