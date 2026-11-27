// SPDX-License-Identifier: CC0-1.0

//! The training-session slice of this service's state: the [`TrainingSession`] record
//! `CreateTrainingSession` writes and `DescribeTrainingSession` replays, its ARN kind,
//! statuses, store bound, clock-owed span and the sentence a reader states an unknown
//! session with.
//!
//! A training session is the CONTROL-PLANE resource behind the SageMaker Training
//! Session Runtime data plane: this slice holds what the session IS — the LoRA
//! configuration it fine-tunes with, the base model it adapts, where its checkpoints are
//! written, the role it runs as and the status its compute has reached. The training and
//! sampling operations submitted against it are the runtime service's own records, so
//! nothing of them is kept here; what the session has ACCOUNTED for that traffic — the
//! sequence number it last received and the tokens billed for it — is the shared account
//! both services read ([`crate::state::training_session_usage`]).

use super::*;

use crate::ops::types::{LoraConfig, TrainingSessionModelConfig, TrainingSessionOutputConfig};

/// The ARN resource kind a training session carries, i.e. the `<kind>` in
/// [`resource_arn`]. Fixed by `TrainingSessionArn`'s modelled pattern
/// (`^arn:(aws[a-z\-]*):sagemaker:[a-z0-9\-]+:[0-9]{12}:training-session/`), and named
/// once here so the create mints and the readers match the one kind.
pub const TRAINING_SESSION_ARN_KIND: &str = "training-session";

/// See [`MODEL_QUOTA`]. Bounds the training-session store the same way, and is what
/// `CreateTrainingSession` answers `ResourceLimitExceeded` past.
pub const TRAINING_SESSION_QUOTA: usize = 1_000;

/// How long after `CreateTrainingSession` the clock owes the `Created` -> `Running`
/// transition on a training session.
///
/// The operation "provisions compute resources for fine-tuning a foundation model"
/// (`CreateTrainingSession`'s doc), which is not instantaneous: the session is reported
/// `Created` until that compute is up and `Running` afterwards. AWS publishes no duration
/// for it, so the span is CHOSEN — the same three seconds this service gives a Studio
/// user profile ([`USER_PROFILE_IN_SERVICE_AFTER`]) — and it is never zero, so a caller that
/// describes the session straight after creating it still observes `Created`. The due
/// instant is stored on the record and materialised by whichever reporting call first
/// runs at or after it; nothing sleeps.
pub const TRAINING_SESSION_RUNNING_AFTER: Duration = Duration::from_secs(3);

/// How long after `StopTrainingSession` the clock owes the `Stopping` -> `Stopped`
/// transition on a training session.
///
/// The operation "stops a running training session and sets the session status to
/// Stopping. After the session stops, the status changes to Stopped"
/// (`StopTrainingSession`'s doc), so stopping is a TRANSIENT state the session passes
/// through on its way to the terminal one. AWS publishes no duration for the passage, so
/// the span is CHOSEN — the same three seconds the create's own transition takes
/// ([`TRAINING_SESSION_RUNNING_AFTER`]) — and it is never zero, so a caller that describes
/// the session straight after stopping it still observes `Stopping`. The due instant is
/// stored on the record and materialised by whichever reporting call first runs at or
/// after it; nothing sleeps.
pub const TRAINING_SESSION_STOPPED_AFTER: Duration = Duration::from_secs(3);

/// The `TrainingSessionStatus` members a created, running, stopped and failed session
/// carries, from the model's own enum
/// (`smithy_query(query="shape",shape="TrainingSessionStatus")`). The enum also carries the
/// blocked, scheduled and deletion-in-progress states, which no operation of this service
/// reaches.
pub const TRAINING_SESSION_CREATED: &str = "Created";
/// See [`TRAINING_SESSION_CREATED`].
pub const TRAINING_SESSION_RUNNING: &str = "Running";
/// See [`TRAINING_SESSION_CREATED`].
pub const TRAINING_SESSION_STOPPING: &str = "Stopping";
/// See [`TRAINING_SESSION_CREATED`].
pub const TRAINING_SESSION_STOPPED: &str = "Stopped";
/// See [`TRAINING_SESSION_CREATED`].
pub const TRAINING_SESSION_FAILED: &str = "Failed";
/// See [`TRAINING_SESSION_CREATED`].
pub const TRAINING_SESSION_EXPIRED: &str = "Expired";
/// See [`TRAINING_SESSION_CREATED`].
pub const TRAINING_SESSION_DELETION_FAILED: &str = "DeletionFailed";

/// Whether a session in `status` can be DELETED: "You must stop a running session before
/// deleting it" (`DeleteTrainingSession`'s doc), so a session is deletable once it is in a
/// state where nothing of it is still running — a terminal state of the lifecycle — and has
/// to be stopped first in every other.
#[must_use]
pub fn training_session_is_deletable(status: &str) -> bool {
    matches!(
        status,
        TRAINING_SESSION_STOPPED
            | TRAINING_SESSION_FAILED
            | TRAINING_SESSION_EXPIRED
            | TRAINING_SESSION_DELETION_FAILED
    )
}

/// Why a delete of a session the caller has not stopped is refused: the sentence
/// `DeleteTrainingSession` documents, over the session this account holds and the status it
/// is in.
#[must_use]
pub fn training_session_must_stop_first(arn: &str, status: &str) -> String {
    format!(
        "Unable to delete training session {arn} because it is in {status} status. You must \
         stop a running session before deleting it."
    )
}

/// The kind label [`could_not_find`] names a training session by.
pub const TRAINING_SESSION_KIND_LABEL: &str = "training session";

/// A stored instant as the `Timestamp` a training session REPORTS it as: the whole second
/// the instant falls in.
///
/// `TrainingSessionDetail`'s `CreationTime`, `LastModifiedTime` and `EndTime` are plain
/// `Timestamp` members with no `@timestampFormat` of their own
/// (`smithy_query(query="shape",shape="TrainingSessionDetail")`), so they travel in the
/// awsJson1_1 default form — an epoch-seconds NUMBER — and the instants SageMaker reports
/// for a training session carry no sub-second part. The host clock these instants are read
/// off ticks at whatever resolution the platform offers (microseconds or nanoseconds), so
/// the reading a clock offers and the timestamp a member reports are not the same
/// resolution, and the ONE conversion between them lives here rather than in the
/// projection of each member: every instant of this record goes through it, so no two
/// members of one session report an instant at two resolutions.
///
/// Truncation, never rounding: the second an instant falls in is the one it is reported
/// in, so a reported timestamp never names an instant that has not happened yet and the
/// order of two instants is never reversed by reporting them. Converting an
/// already-reported value again changes nothing. The INSTANT itself is the host clock's
/// and is not a requirement; its resolution is.
#[must_use]
pub fn training_session_instant(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs(timestamp(at).secs())
}

/// How an operation that cannot resolve the training session a request names states it:
/// the [`could_not_find`] sentence over the NAME the caller sent.
///
/// A training session is addressed by `TrainingSessionName` and by nothing else — every
/// operation of the family takes that one member — so the identifier the refusal quotes
/// is the one the request carried, never an ARN synthesised from it. Stated once here so
/// the readers of this resource cannot word the same fact differently.
#[must_use]
pub fn training_session_not_found(name: &str) -> String {
    could_not_find(TRAINING_SESSION_KIND_LABEL, name)
}

/// A training session created by `CreateTrainingSession`, as `DescribeTrainingSession`
/// replays it.
///
/// `running_due_at` is the instant the clock owes the `Created` -> `Running` transition;
/// it is stored, never slept on, and materialised by whichever reporting call first runs
/// at or after it ([`Self::settle_status`]).
///
/// `stopped_due_at` is the same for the `Stopping` -> `Stopped` transition a stop asks
/// for, and `None` for a session nobody has stopped; `end_time` is the instant the session
/// ENDED, which only a session that has reached a terminal status has.
///
/// `model_package_group_arn` and `vpc_config` stay `None` when the create named neither:
/// the model documents no default for either, so the session reports no value rather than
/// one the request never asked for.
#[derive(Debug, Clone)]
pub struct TrainingSession {
    pub name: String,
    pub arn: String,
    pub status: String,
    pub model_package_group_arn: Option<String>,
    pub role_arn: String,
    pub lora_config: LoraConfig,
    pub model_config: TrainingSessionModelConfig,
    pub output_config: TrainingSessionOutputConfig,
    pub vpc_config: Option<VpcConfig>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub running_due_at: SystemTime,
    pub stopped_due_at: Option<SystemTime>,
    pub end_time: Option<SystemTime>,
}

impl TrainingSession {
    /// Materialise the transition the clock already owes — `Created` -> `Running` for a
    /// session whose compute has come up, and `Stopping` -> `Stopped` for one a stop asked
    /// to quiesce — the way [`MlflowTrackingServer::settle_status`] does for a tracking
    /// server: each was due at its stored instant whether or not anybody described the
    /// session, so the observer makes no difference to the result, and `LastModifiedTime`
    /// moves to the instant the transition was due rather than to "now".
    ///
    /// Answers whether a transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == TRAINING_SESSION_CREATED && now >= self.running_due_at {
            self.status = TRAINING_SESSION_RUNNING.to_string();
            self.last_modified_time = self.running_due_at;
            return true;
        }
        if self.status == TRAINING_SESSION_STOPPING {
            if let Some(due_at) = self.stopped_due_at {
                if now >= due_at {
                    self.status = TRAINING_SESSION_STOPPED.to_string();
                    // The session ENDED when it finished stopping, which is the instant the
                    // transition was due, so `EndTime` and `LastModifiedTime` name it
                    // rather than the instant of whichever call observed it.
                    self.end_time = Some(due_at);
                    self.last_modified_time = due_at;
                    self.stopped_due_at = None;
                    return true;
                }
            }
        }
        false
    }

    /// Ask this session to stop: it "sets the session status to Stopping", and the clock
    /// owes the terminal `Stopped` at [`TRAINING_SESSION_STOPPED_AFTER`]
    /// ([`Self::settle_status`]).
    ///
    /// Answers whether this call started the stop. A session that is already stopping, or
    /// that has already stopped, is left exactly as it is: the stop it is serving is the
    /// one the first call asked for, so repeating the request neither restarts the passage
    /// nor moves the instant the session ended. A session still provisioning stops too —
    /// what the stop ends is the session, not only the compute that came up for it.
    pub fn request_stop(&mut self, now: SystemTime) -> bool {
        if self.status != TRAINING_SESSION_CREATED && self.status != TRAINING_SESSION_RUNNING {
            return false;
        }
        self.status = TRAINING_SESSION_STOPPING.to_string();
        self.last_modified_time = now;
        self.stopped_due_at = Some(now + TRAINING_SESSION_STOPPED_AFTER);
        self.end_time = None;
        true
    }
}

/// What the shared directory publishes for `session`: the record the DATA plane resolves a
/// training session name against.
///
/// The data plane models no create, so this record is the only thing that makes a session
/// exist for it; beside the identity it carries the two facts about the session that are
/// this plane's to decide and the other plane's to honour — where the session is in its
/// lifecycle, which decides whether it accepts new operations, and the base model it
/// adapts, which is the model its operations are served by. A status this service reaches
/// on a clock-owed instant is published as the status the session is in NOW plus the
/// transition the clock owes, so neither plane has to be called for the other to observe
/// the same status at the same instant.
#[must_use]
pub fn published_training_session(
    session: &TrainingSession,
) -> ::service_common::training_sessions::TrainingSessionRecord {
    use ::service_common::training_sessions::TrainingSessionLifecycle;
    let lifecycle = match session.status.as_str() {
        TRAINING_SESSION_CREATED => TrainingSessionLifecycle::owing(
            TRAINING_SESSION_CREATED,
            TRAINING_SESSION_RUNNING,
            session.running_due_at,
        ),
        TRAINING_SESSION_STOPPING => match session.stopped_due_at {
            Some(due_at) => TrainingSessionLifecycle::owing(
                TRAINING_SESSION_STOPPING,
                TRAINING_SESSION_STOPPED,
                due_at,
            ),
            None => TrainingSessionLifecycle::settled(TRAINING_SESSION_STOPPING),
        },
        settled => TrainingSessionLifecycle::settled(settled),
    };
    ::service_common::training_sessions::TrainingSessionRecord {
        name: session.name.clone(),
        arn: session.arn.clone(),
        lifecycle,
        base_model_arn: session.model_config.base_model_arn.clone(),
    }
}

/// Publish `session` into the directory the data plane resolves a training session name
/// against ([`published_training_session`]).
///
/// Called wherever this service CREATES a session or MOVES its status, which is where what
/// the directory claims about it changes. The directory is an aid to another service and
/// never part of this operation's own answer, so an engine holding none leaves it
/// unwritten rather than failing the call.
pub fn publish_training_session<Eng: Send + Sync + 'static>(engine: &Eng, session: &TrainingSession) {
    if let Some(directory) = training_session_directory(engine) {
        directory.lock().register(published_training_session(session));
    }
}

/// Drop the training session `name` from what this service published about it: the shared
/// directory the data plane resolves a name through, and the account it reads the session's
/// billed totals from.
///
/// Called where this service DELETES a session. A session that no longer exists must stop
/// resolving for the data plane too, and nothing it received or consumed is accounted any
/// longer; as with publishing, an engine holding neither store leaves it alone rather than
/// failing the call.
pub fn forget_training_session<Eng: Send + Sync + 'static>(engine: &Eng, name: &str) {
    if let Some(directory) = training_session_directory(engine) {
        directory.lock().forget(name);
    }
    if let Some(account) = training_session_usage(engine) {
        account.lock().forget(name);
    }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another training session fits, i.e. the store is below
    /// [`TRAINING_SESSION_QUOTA`].
    pub fn training_sessions_below_quota(&self) -> bool {
        self.training_sessions.len() < TRAINING_SESSION_QUOTA
    }

    /// The training session `name` names, with the transition the clock already made due
    /// applied, or `None` when this account holds none under that name.
    ///
    /// For a REPORTING operation: a name no create has written answers `None` rather than
    /// being materialised here. Answers the session beside whether the transition happened
    /// on this call, so the caller can record the state change it materialised.
    pub fn advance_training_session(
        &mut self,
        name: &str,
        now: SystemTime,
    ) -> Option<(&TrainingSession, bool)> {
        let session = self.training_sessions.get_mut(name)?;
        let settled = session.settle_status(now);
        Some((session, settled))
    }

    /// Release the training session `name` from this account, with the transition the clock
    /// already made due applied first, or `None` when this account holds no session under
    /// that name.
    ///
    /// Answers the session as it stood when it was released, so the caller can record the
    /// state change and drop what this service published about it. A session that has not
    /// been stopped is NOT released: it is answered as `Err` carrying the status it is in,
    /// which the caller refuses with ([`training_session_must_stop_first`]).
    pub fn delete_training_session(
        &mut self,
        name: &str,
        now: SystemTime,
    ) -> Option<Result<TrainingSession, TrainingSession>> {
        let session = self.training_sessions.get_mut(name)?;
        session.settle_status(now);
        if !training_session_is_deletable(&session.status) {
            return Some(Err(session.clone()));
        }
        Some(Ok(self.training_sessions.remove(name)?))
    }

    /// Move the training session `name` into `Stopping`, with the clock owing the terminal
    /// `Stopped`, or `None` when this account holds no session under that name.
    ///
    /// Answers the session as it now stands beside whether this call started the stop, so
    /// the caller records the state change it made, publishes the session's new lifecycle
    /// and reports nothing for a request that changed nothing. The transition the clock
    /// already owed is materialised first, so a session whose compute had come up stops
    /// from `Running` rather than from the status a read had not caught up with yet.
    pub fn stop_training_session(
        &mut self,
        name: &str,
        now: SystemTime,
    ) -> Option<(TrainingSession, bool)> {
        let session = self.training_sessions.get_mut(name)?;
        session.settle_status(now);
        let started = session.request_stop(now);
        Some((session.clone(), started))
    }

    /// Whether `arn` names a model package group of THIS account in `region` that the
    /// account does not hold — the one reading of a `ModelPackageGroupArn` reference this
    /// service resolves.
    ///
    /// A group ARN of another partition, Region or account names no record here, so it is
    /// not something this emulator can answer "does not exist" about: only an ARN this
    /// service itself mints ([`resource_arn`]) is resolved, and anything else is left
    /// alone.
    pub fn model_package_group_arn_is_unresolvable(&self, region: &str, arn: &str) -> bool {
        let Some(resource) = arn_resource(arn, region) else {
            return false;
        };
        let Some(name) = resource.strip_prefix(&format!("{MODEL_PACKAGE_GROUP_ARN_KIND}/")) else {
            return false;
        };
        !self.model_package_groups.contains_key(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a training session reports an instant at the whole second it falls in, and
    /// the second it reports is the instant's own.
    #[test]
    fn a_reported_instant_carries_whole_seconds() {
        // The table spans what the rule generalises over: the epoch itself, a whole
        // second, and the sub-second ticks a host clock offers but no timestamp of this
        // record carries — a millisecond, a microsecond and a nanosecond past a second,
        // and the last nanosecond below the next one.
        let cases = [
            ("the epoch", Duration::from_secs(0), 0),
            (
                "a whole second",
                Duration::from_secs(1_700_000_000),
                1_700_000_000,
            ),
            (
                "a millisecond past a second",
                Duration::from_millis(1_700_000_000_123),
                1_700_000_000,
            ),
            (
                "a microsecond past a second",
                Duration::from_micros(1_700_000_000_000_123),
                1_700_000_000,
            ),
            (
                "a nanosecond past a second",
                Duration::from_nanos(1_700_000_000_000_000_123),
                1_700_000_000,
            ),
            (
                "the last nanosecond below the next second",
                Duration::from_nanos(1_700_000_000_999_999_999),
                1_700_000_000,
            ),
        ];
        for (case, since_epoch, expected_secs) in cases {
            let reported = training_session_instant(SystemTime::UNIX_EPOCH + since_epoch);
            assert_eq!(
                (reported.secs(), reported.subsec_nanos()),
                (expected_secs, 0),
                "case {case:?}: expected {expected_secs} whole epoch seconds, got {reported:?}"
            );
        }
    }

    /// RULE: reporting an already-reported instant again changes nothing, so a value
    /// derived from one a session already reported keeps it.
    #[test]
    fn reporting_a_reported_instant_is_stable() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_nanos(1_700_000_000_987_654_321);
        let once = training_session_instant(at);
        let twice =
            training_session_instant(SystemTime::UNIX_EPOCH + Duration::from_secs(once.secs() as u64));
        assert_eq!(
            (twice.secs(), twice.subsec_nanos()),
            (once.secs(), once.subsec_nanos()),
            "expected a reported instant to be stable under a second reporting, got \
             {twice:?} instead of {once:?}"
        );
    }
}
