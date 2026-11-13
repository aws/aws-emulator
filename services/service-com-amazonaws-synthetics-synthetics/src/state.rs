// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-synthetics-synthetics service.
//!
//! One record per canary, keyed by canary name — the identity CloudWatch Synthetics
//! itself uses on the wire (`GET /canary/{Name}`), so a name is unique per account and
//! region. A [`BTreeMap`] rather than a hash map because `DescribeCanaries` pages over
//! this collection and a tokenised page boundary needs a total, stable order.
//!
//! Everything a handler returns is projected from one of these records
//! ([`StoredCanary::to_canary`]); no handler builds a `Canary` from its own request.

use std::collections::hash_map::RandomState;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{BuildHasher as _, Hasher as _};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ArtifactConfigOutput, BrowserConfig, Canary, CanaryCodeOutput, CanaryDryRunConfigOutput,
    CanaryRun, CanaryRunConfigOutput, CanaryRunStatus, CanaryRunTimeline, CanaryScheduleOutput,
    CanaryStatus, CanaryTimeline, Dependency, DryRunConfigOutput, Group, GroupSummary,
    RetryConfigOutput, RuntimeVersion, S3EncryptionConfig, VpcConfigOutput,
};

/// The WORK one canary run performs, and the verdict that work produces: the run's
/// artifacts written to the canary's artifact location, under the permissions its execution
/// role carries.
pub mod canary_run;

/// Mint a fresh identifier for a canary or a canary run.
///
/// `Canary$Id` and `CanaryRun$Id` are modelled as `UUID`
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`), which is exactly a
/// lowercase-hyphenated version-4 UUID, so that is what this mints: sixteen bytes of
/// SHA-256 (the `sha2` crate, not a hand-written hash) over a seed that is unique per
/// call, laid out with the version-4 and RFC 4122 variant nibbles and hex-encoded (the
/// `hex` crate, not a hand-written codec) into the five modelled groups.
///
/// The seed is a process-unique counter, the current instant and one operating-system
/// seeded value per process, so two ids minted by one process differ by construction and
/// ids minted by different processes differ too. NOTHING about the request enters the
/// seed: an id is not derived from client input.
pub fn new_uuid() -> String {
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&fresh_digest()[..16]);
    // Version 4 in the high nibble of byte 6 and the RFC 4122 variant in the top bits of
    // byte 8, the layout a random UUID carries on the wire.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let digits = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &digits[0..8],
        &digits[8..12],
        &digits[12..16],
        &digits[16..20],
        &digits[20..32]
    )
}

/// Sixteen-plus bytes of fresh, request-independent entropy: SHA-256 (the `sha2` crate,
/// not a hand-written hash) over a seed that is unique per call.
///
/// The seed is a process-unique counter, the current instant and one operating-system
/// seeded value per process, so two values drawn by one process differ by construction and
/// values drawn by different processes differ too. NOTHING about the request enters the
/// seed: an identifier is never derived from client input. Shared by every identifier this
/// service mints ([`new_uuid`], [`new_group_id`]) so there is one entropy source.
fn fresh_digest() -> [u8; 32] {
    use sha2::Digest as _;

    /// Distinguishes two ids minted in the same instant by the same process.
    static MINTED: AtomicU64 = AtomicU64::new(0);
    /// One value per process, drawn from the operating system: `RandomState`'s keys are
    /// OS-seeded the first time one is built, which is the entropy std exposes.
    static PROCESS_SEED: OnceLock<u64> = OnceLock::new();

    let process_seed = *PROCESS_SEED.get_or_init(|| {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u32(::std::process::id());
        hasher.finish()
    });
    let minted = MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_nanos())
        .unwrap_or_default();

    let mut digest = sha2::Sha256::new();
    digest.update(process_seed.to_le_bytes());
    digest.update(minted.to_le_bytes());
    digest.update(nanos.to_le_bytes());
    digest.finalize().into()
}

/// How many bytes of digest a minted group id carries, which hex-encoding doubles into
/// twelve characters. `Group$Id` is modelled only as a string (`length 1..1024`), but
/// `Group$Arn`'s pattern pins the resource segment it appears in to `group:[0-9a-z]+`, and
/// the real service's ids are a short fixed-width token; twelve characters is that width.
const GROUP_ID_BYTES: usize = 6;

/// Mint a fresh group id.
///
/// `Group$Arn` is modelled as
/// `^arn:(aws[a-zA-Z-]*)?:synthetics:…:\d{12}:group:[0-9a-z]+$`, so a group's id is the
/// ARN's resource segment and must be lowercase alphanumeric. This mints
/// [`GROUP_ID_BYTES`] bytes from [`fresh_digest`] as hex digits — a subset of that
/// alphabet — hex-encoded with the `hex` crate rather than a hand-written codec. Nothing
/// about the request enters it: an id is not derived from the group's name.
pub fn new_group_id() -> String {
    let digest = fresh_digest();
    ::hex::encode(&digest[..GROUP_ID_BYTES])
}

/// Whether `value` has the shape the model gives `Canary$Id` and `CanaryRun$Id`:
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`. Group lengths and
/// lowercase hex only — the modelled pattern says nothing about the version or variant
/// nibbles — which is stricter than a general UUID parse (that accepts uppercase and
/// braced or unhyphenated forms). Used by the token validation that has to recognise an
/// id this service minted; this crate's tests read the modelled pattern themselves rather
/// than calling this, so a wrong predicate here cannot make an assertion pass vacuously.
pub fn is_uuid(value: &str) -> bool {
    let groups = [8usize, 4, 4, 4, 12];
    let mut parts = value.split('-');
    for len in groups {
        match parts.next() {
            Some(part)
                if part.len() == len
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) => {}
            _ => return false,
        }
    }
    parts.next().is_none()
}

/// The retention default both retention members document: "If you omit this field, the
/// default of 31 days is used." (`CreateCanaryRequest$SuccessRetentionPeriodInDays` /
/// `$FailureRetentionPeriodInDays`, via
/// `smithy_query(query="doc",shape="CreateCanaryRequest",member="SuccessRetentionPeriodInDays")`).
pub const DEFAULT_RETENTION_PERIOD_IN_DAYS: i32 = 31;

/// `CanaryScheduleInput$DurationInSeconds`: "If you omit this field, the default of 0 is
/// used."
pub const DEFAULT_SCHEDULE_DURATION_IN_SECONDS: i64 = 0;

/// `CreateCanaryRequest$ProvisionedResourceCleanup`: "If you omit this parameter, the
/// default of AUTOMATIC is used".
pub const DEFAULT_PROVISIONED_RESOURCE_CLEANUP: &str = "AUTOMATIC";

/// `CanaryRunConfigInput$TimeoutInSeconds`: "If you omit this field, the frequency of the
/// canary is used as this value, up to a maximum of 14 minutes." 14 minutes = 840 s, which
/// is also the member's modelled maximum.
pub const MAX_DEFAULTED_RUN_TIMEOUT_IN_SECONDS: i32 = 840;

/// The member's modelled minimum (`range 3..840`), the floor for a derived default.
pub const MIN_RUN_TIMEOUT_IN_SECONDS: i32 = 3;

/// How long a provisioning transition this emulator owes stays pending.
///
/// `CreateCanary` answers with `CREATING`, a non-terminal state, so the move to `READY`
/// is owed by the clock rather than by any later call: the due instant is recorded and
/// materialised by whichever request next looks at the record. The span is deliberately
/// non-zero, so a caller that acts immediately still observes `CREATING` and the
/// conflict errors that state implies.
pub const PROVISIONING_SETTLE: Duration = Duration::from_millis(500);

/// How long a canary run this emulator started stays `RUNNING` before the clock owes its
/// outcome. Same contract as [`PROVISIONING_SETTLE`].
pub const RUN_SETTLE: Duration = Duration::from_millis(500);

/// The `CanaryRunStateReasonCode` a run carries when the artifacts it produces could not
/// be stored.
///
/// `CanaryRunStatus$StateReasonCode`: "If this value is `EXECUTION_FAILURE`, a non-critical
/// failure occurred such as failing to save generated debug artifacts (for example,
/// screenshots or har files)"
/// (`smithy_query(query="doc",shape="CanaryRunStatus",member="StateReasonCode")`). That is
/// the failure [`canary_run::perform`] reports: the run's artifacts could not be written to
/// the canary's artifact location.
pub const RUN_EXECUTION_FAILURE_REASON_CODE: &str = "EXECUTION_FAILURE";

/// Why a run that was still open when the canary was stopped has no verdict: its schedule
/// was stopped before the run finished, so the run never produced the artifacts a verdict
/// is read from.
pub const RUN_INTERRUPTED_REASON: &str =
    "The canary was stopped before this run completed, so the run produced no result.";

/// Why a run whose outcome came due carries no verdict: nothing recorded the outcome of the
/// work the run performs, so there is no result to report. The call that opens a run records
/// its outcome before it answers ([`canary_run::perform`]), so this is the state of a run
/// whose opening call did not finish.
pub const RUN_UNRECORDED_REASON: &str =
    "The outcome of this run was never recorded, so the run produced no result.";

/// `CanaryStateReasonCode` reported while a freshly created canary is still `CREATING`.
pub const CREATE_IN_PROGRESS_REASON_CODE: &str = "CREATE_IN_PROGRESS";

/// The message every handler answers with when the shared store cannot be reached. One
/// constant so the handlers that can hit it word it identically.
pub const STORE_UNAVAILABLE: &str = "the synthetics canary store is unavailable";

/// `ResourceNotFoundException$Message` for a canary an operation whose wording is CHOSEN
/// rather than recorded cannot resolve.
///
/// The model says of the error only "One of the specified resources was not found." and
/// documents no text, so where nothing records the sentence this service answers one fixed,
/// identifier-free one: the operations that take a canary `Name` (`GetCanary`,
/// `DeleteCanary`, `StartCanary`, `StopCanary`, `GetCanaryRuns`, `StartCanaryDryRun`) and
/// `ListAssociatedGroups`. For those the message is request-independent: a constant here,
/// not built from the lookup key, so no identifier is handed back to the caller.
///
/// The operation whose wording IS recorded echoes the identifier instead — see
/// [`canary_not_found`] — which is why this is a per-operation choice rather than one
/// service-wide constant, exactly as it is for groups ([`GROUP_NOT_FOUND`] beside
/// [`group_not_found`]).
pub const CANARY_NOT_FOUND: &str = "Canary not found.";

/// `ResourceNotFoundException$Message` for a canary an operation whose wording is KNOWN
/// cannot resolve: it echoes the identifier the request supplied.
///
/// The recorded service answers an `AssociateResource` whose `ResourceArn` resolves to no
/// canary with `Canary does not exist: <the ARN as supplied>`, the same construction it
/// uses on that same call for a group identifier it cannot resolve ([`group_not_found`]).
/// What is a REQUIREMENT is that construction and not a remembered string: the sentence
/// plus the request's own value, whichever ARN the caller sent, which is why every
/// assertion over it is a table of ARNs rather than one recorded value
/// (`tests/all/client_error_message_wording.rs`).
pub fn canary_not_found(identifier: &str) -> String {
    format!("Canary does not exist: {identifier}")
}

/// Which transition a state-transition operation would have made, as the refusal below
/// names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanaryTransition {
    /// `StartCanary`.
    Started,
    /// `StopCanary`.
    Stopped,
    /// `DeleteCanary`.
    Deleted,
    /// `StartCanaryDryRun`.
    DryRun,
}

impl CanaryTransition {
    /// How the refusal names this transition.
    pub fn as_refused(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Stopped => "stopped",
            Self::Deleted => "deleted",
            Self::DryRun => "dry run",
        }
    }
}

/// `ConflictException$Message` for a canary whose current state does not admit the
/// transition a call asked for.
///
/// The recorded service words this class of refusal as one fixed sentence naming the
/// transition it will not make, followed by the canary's current `CanaryState` wire value as
/// this store holds it: `Canary is in a state that can't be stopped: READY` for a
/// `StopCanary` of a canary that is not running, and `Canary is in a state that can't be
/// deleted: RUNNING` for a `DeleteCanary` of one that is. The canary's NAME is no part of it
/// — this class of message reports the state that refused the call and no identifier.
///
/// ONE construction for every state-transition operation ([`CanaryTransition`]), so
/// `StartCanary`, `StopCanary`, `DeleteCanary` and `StartCanaryDryRun` cannot drift apart.
pub fn canary_state_conflict(transition: CanaryTransition, state: CanaryState) -> String {
    format!(
        "Canary is in a state that can't be {}: {}",
        transition.as_refused(),
        state.as_wire()
    )
}

/// `ConflictException$Message` for an `AssociateResource` whose canary is ALREADY a member
/// of the group it names.
///
/// Membership is a set and an association is not idempotent: the recorded service refuses
/// the second association of one (group, canary) pair rather than answering it as a success
/// that changed nothing. The sentence is fixed and its variable parts are the request's own
/// `ResourceArn` and the NAME of the group its `GroupIdentifier` resolved to — the group may
/// have been addressed by name, ARN or id, and the message reports the one name the group is
/// held under.
pub fn canary_already_in_group(resource_arn: &str, group_name: &str) -> String {
    format!("Canary {resource_arn} already exists in group {group_name}")
}

/// `ResourceNotFoundException$Message` for a dry run that does not exist, by the same
/// rule as [`CANARY_NOT_FOUND`]: a missing-resource message names the resource KIND that
/// was not found and never the identifier the request asked for.
pub const DRY_RUN_NOT_FOUND: &str = "Dry run not found.";

/// `ResourceNotFoundException$Message` for a group that does not exist, as the group
/// operations whose wording is CHOSEN rather than recorded word it: the sentence names the
/// resource KIND and never the `GroupIdentifier` the request asked for.
///
/// The operations whose wording is KNOWN rather than chosen echo the identifier instead —
/// see [`group_not_found`] — which is why this is a per-operation choice and not one
/// service-wide constant.
pub const GROUP_NOT_FOUND: &str = "Group not found.";

/// The most groups this store holds, which is also the real service's quota: "You can have
/// as many as 20 groups in your account" (`CreateGroup`'s own documentation, via
/// `smithy_query(query="doc",shape="CreateGroup")`). A caller past this bound is past the
/// real service's bound too and is refused with the modelled
/// `ServiceQuotaExceededException`.
pub const MAX_GROUPS: usize = 20;

/// The most canaries this store holds, so a request-driven create loop cannot grow it
/// without limit.
///
/// CloudWatch Synthetics' own default quota is 100 canaries per Region ("Canaries" in the
/// CloudWatch Synthetics service quotas), so a caller past this bound is past the real
/// service's bound too and is refused with the modelled `ServiceQuotaExceededException`.
pub const MAX_CANARIES: usize = 100;

/// The most runs this store keeps per canary, so a request-driven start/stop loop cannot
/// grow one canary's history without limit.
///
/// Emulator-chosen: the real service bounds the history by the canary's retention period
/// (enforced in [`StoredCanary::settle`]) rather than by a count, but a wall-clock
/// retention of days is no bound at all on a loop that runs for seconds, so the oldest
/// runs beyond this many are dropped as well.
pub const MAX_RETAINED_RUNS_PER_CANARY: usize = 100;

/// `ResourceNotFoundException$Message` for a group an operation cannot resolve, where the
/// recorded service's wording is KNOWN: it echoes the identifier.
///
/// Unlike the chosen [`GROUP_NOT_FOUND`], this wording is not invented: the recorded
/// service answers an `AssociateResource` whose `GroupIdentifier` it cannot resolve — and,
/// per the recorded unit `synthetics.delete-group-nonexistent-resource-not-found`, a
/// `DeleteGroup` for a group that does not exist as well as the `GetGroup` reading a group
/// just deleted — with `Group does not exist: <the identifier as supplied>`, naming the
/// value in whichever of its three forms (name, ARN or id) the caller used. So for those
/// operations the rule is the sentence plus the echoed identifier, and nothing else about
/// the group is reported.
///
/// What is a REQUIREMENT here is the construction, not a remembered string: the message is
/// built from the request's own `GroupIdentifier`, which is why every assertion over it is
/// a table of identifiers rather than one recorded value
/// (`tests/all/group_deletion.rs::a_delete_of_a_missing_group_names_the_requested_identifier`).
/// The operations with NO recorded wording keep the service's fixed, identifier-free
/// sentence instead ([`GROUP_NOT_FOUND`], used by `ListGroupResources`): a message is a
/// requirement only where the rule that produces it is known, so the split between the two
/// is per operation and deliberate.
pub fn group_not_found(identifier: &str) -> String {
    format!("Group does not exist: {identifier}")
}

/// The most canaries one group can contain.
///
/// `CreateGroup`: "Each group can contain as many as 10 canaries."
pub const MAX_CANARIES_PER_GROUP: usize = 10;

/// The most groups one canary can be a member of.
///
/// `CreateGroup`: "Any single canary can be a member of up to 10 groups."
pub const MAX_GROUPS_PER_CANARY: usize = 10;

/// Seconds in the day the retention periods are counted in.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// The lifecycle states a canary can be in.
///
/// The wire spellings are the members of the model's `CanaryState` enum (see
/// `ops::types::CANARY_STATE_VALUES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanaryState {
    Creating,
    Ready,
    Starting,
    Running,
    Updating,
    Stopping,
    Stopped,
    Error,
    Deleting,
}

impl CanaryState {
    /// The `CanaryState` member name this state serialises as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Ready => "READY",
            Self::Starting => "STARTING",
            Self::Running => "RUNNING",
            Self::Updating => "UPDATING",
            Self::Stopping => "STOPPING",
            Self::Stopped => "STOPPED",
            Self::Error => "ERROR",
            Self::Deleting => "DELETING",
        }
    }

    /// Whether an operation on the canary is already in flight, so a second one
    /// conflicts with it. `ConflictException`'s documented meaning is "a conflicting
    /// operation is already in progress", which is exactly this set.
    pub fn operation_in_progress(self) -> bool {
        matches!(
            self,
            Self::Creating | Self::Starting | Self::Stopping | Self::Updating | Self::Deleting
        )
    }
}

/// The states a single canary run can be in (model enum `CanaryRunState`, see
/// `ops::types::CANARY_RUN_STATE_VALUES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanaryRunState {
    Running,
    Passed,
    Failed,
}

impl CanaryRunState {
    /// The `CanaryRunState` member name this state serialises as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Running => "RUNNING",
            Self::Passed => "PASSED",
            Self::Failed => "FAILED",
        }
    }
}

/// Where a canary's script came from, plus the layer ARN Synthetics reports for it.
#[derive(Debug, Clone)]
pub struct StoredCode {
    pub handler: Option<String>,
    pub blueprint_types: Vec<String>,
    pub dependencies: Vec<StoredDependency>,
    pub source_location_arn: String,
    pub s3_bucket: Option<String>,
    pub s3_key: Option<String>,
    pub s3_version: Option<String>,
}

/// One entry of `CanaryCodeInput$Dependencies`, kept so the code structure reports back
/// what was supplied.
#[derive(Debug, Clone)]
pub struct StoredDependency {
    pub r#type: Option<String>,
    pub reference: String,
}

/// How often the canary runs, and for how long it keeps doing so.
#[derive(Debug, Clone)]
pub struct StoredSchedule {
    pub expression: String,
    pub duration_in_seconds: i64,
    pub max_retries: Option<i32>,
}

/// How long a started canary keeps running, as its OWN schedule bounds it — which is what
/// decides whether the clock owes it a move back out of [`CanaryState::Running`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunningBound {
    /// The schedule runs the canary once. `CanaryScheduleInput$Expression`: "Specifying
    /// `rate(0 minute)` or `rate(0 hour)` is a special value that causes the canary to run
    /// only once when it is started", so the canary is done once that single run is.
    SingleRun,
    /// The schedule keeps running the canary for this long.
    /// `CanaryScheduleInput$DurationInSeconds`: "How long, in seconds, for the canary to
    /// continue making regular runs according to the schedule in the `Expression` value."
    For(Duration),
    /// Nothing about the schedule bounds it: "If you specify 0, the canary continues making
    /// runs until you stop it."
    UntilStopped,
}

impl StoredSchedule {
    /// The bound this schedule puts on how long the canary runs, read from the two members
    /// that carry one. A run-once expression bounds it however long the duration says,
    /// because there is only one run to make.
    pub fn running_bound(&self) -> RunningBound {
        if is_run_once_schedule(&self.expression) {
            return RunningBound::SingleRun;
        }
        match u64::try_from(self.duration_in_seconds) {
            Ok(0) | Err(_) => RunningBound::UntilStopped,
            Ok(seconds) => RunningBound::For(Duration::from_secs(seconds)),
        }
    }
}

/// Per-run configuration. `environment_variables` is stored but never reported:
/// `CanaryRunConfigOutput` models no member for it, because the real service does not
/// echo canary environment variables back.
#[derive(Debug, Clone, Default)]
pub struct StoredRunConfig {
    pub timeout_in_seconds: i32,
    pub memory_in_mb: Option<i32>,
    pub active_tracing: Option<bool>,
    pub ephemeral_storage: Option<i32>,
    pub environment_variables: BTreeMap<String, String>,
}

/// Artifact encryption settings, as supplied.
#[derive(Debug, Clone)]
pub struct StoredArtifactConfig {
    pub encryption_mode: Option<String>,
    pub kms_key_arn: Option<String>,
}

/// VPC placement, as supplied. `vpc_id` is not stored: the real service derives it from
/// the subnets, and this emulator has no subnet inventory to derive it from, so it is
/// reported absent rather than invented.
#[derive(Debug, Clone)]
pub struct StoredVpcConfig {
    pub subnet_ids: Vec<String>,
    pub security_group_ids: Vec<String>,
    pub ipv6_allowed_for_dual_stack: Option<bool>,
}

/// What one canary run's work produced: the state the run ends in, and the reason it
/// carries when that state is a failure.
///
/// Recorded by the call that opens the run, once the work the run performs has been done
/// ([`canary_run::perform`]), and materialised onto the run by [`StoredCanary::settle`] when
/// the clock owes the run its outcome — so the verdict a client reads is the outcome of work
/// that was actually attempted, never a value chosen when the run began.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub state: CanaryRunState,
    pub state_reason: Option<String>,
    pub state_reason_code: Option<String>,
}

impl RunOutcome {
    /// The run did the work it is judged by: its artifacts were stored. A passing run
    /// carries no reason — there is no failure to explain.
    pub fn passed() -> Self {
        Self {
            state: CanaryRunState::Passed,
            state_reason: None,
            state_reason_code: None,
        }
    }

    /// The run could not store its artifacts, which is what
    /// [`RUN_EXECUTION_FAILURE_REASON_CODE`] reports, with `reason` saying what stopped it.
    pub fn execution_failure(reason: impl Into<String>) -> Self {
        Self {
            state: CanaryRunState::Failed,
            state_reason: Some(reason.into()),
            state_reason_code: Some(RUN_EXECUTION_FAILURE_REASON_CODE.to_string()),
        }
    }
}

/// One recorded run of a canary.
#[derive(Debug, Clone)]
pub struct StoredRun {
    pub id: String,
    pub name: String,
    pub state: CanaryRunState,
    pub state_reason: Option<String>,
    pub state_reason_code: Option<String>,
    pub started: SystemTime,
    pub completed: Option<SystemTime>,
    /// The instant at which the clock owes this run's outcome, while it is `RUNNING`.
    pub outcome_due: Option<SystemTime>,
    /// The verdict the run's own work produced, waiting for that instant.
    pub outcome: Option<RunOutcome>,
    pub artifact_s3_location: String,
    /// The dry run this run belongs to, when it is one.
    ///
    /// `GetCanaryRunsRequest$RunType`: "When CANARY_RUN is provided, all canary runs
    /// excluding dry runs are returned[;] When DRY_RUN is provided, all dry runs excluding
    /// canary runs are returned" — so whether a run is a dry run is a property of the run
    /// itself, and `None` here is a scheduled or manually started canary run.
    pub dry_run_id: Option<String>,
}

impl StoredRun {
    /// Project this run onto the modelled `CanaryRun` shape.
    pub fn to_canary_run(&self) -> CanaryRun {
        CanaryRun {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            status: Some(CanaryRunStatus {
                state: Some(self.state.as_wire().to_string()),
                state_reason: self.state_reason.clone(),
                state_reason_code: self.state_reason_code.clone(),
                test_result: None,
            }),
            timeline: Some(CanaryRunTimeline {
                started: Some(self.started.into()),
                completed: self.completed.map(Into::into),
                metric_timestamp_for_run_and_retries: None,
            }),
            artifact_s3_location: Some(self.artifact_s3_location.clone()),
            dry_run_config: self.dry_run_id.as_ref().map(|id| CanaryDryRunConfigOutput {
                dry_run_id: Some(id.clone()),
            }),
            ..Default::default()
        }
    }
}

/// One dry run of a canary: the identity the API addresses it by, and the configuration it
/// exercises.
///
/// `StartCanaryDryRun` runs a canary that already exists with a configuration that may
/// differ from the canary's own, without changing the canary. The configuration is
/// therefore SNAPSHOT here when the dry run starts, so a later change to the canary cannot
/// rewrite what the dry run was given; the snapshot carries no run history and no nested
/// dry runs of its own, because those belong to the canary record rather than to one of
/// its configurations.
#[derive(Debug, Clone)]
pub struct StoredDryRun {
    /// `DryRunConfigOutput$DryRunId`, a minted [`new_uuid`].
    pub id: String,
    pub config: Box<StoredCanary>,
}

/// One canary, as this emulator persists it. Every reported field is read from here.
#[derive(Debug, Clone)]
pub struct StoredCanary {
    pub id: String,
    pub name: String,
    pub region: String,
    pub code: StoredCode,
    pub execution_role_arn: String,
    pub schedule: StoredSchedule,
    pub run_config: StoredRunConfig,
    pub success_retention_period_in_days: i32,
    pub failure_retention_period_in_days: i32,
    pub artifact_s3_location: String,
    pub artifact_config: Option<StoredArtifactConfig>,
    pub runtime_version: String,
    pub vpc_config: Option<StoredVpcConfig>,
    pub engine_arn: String,
    pub provisioned_resource_cleanup: String,
    pub kms_key_arn: Option<String>,
    pub state: CanaryState,
    pub state_reason: Option<String>,
    pub state_reason_code: Option<String>,
    /// The instant at which the clock owes the move out of [`CanaryState::Creating`].
    pub ready_due: Option<SystemTime>,
    /// The instant at which the clock owes the move out of [`CanaryState::Running`],
    /// because the canary's own schedule bounds how long it keeps running
    /// ([`StoredSchedule::running_bound`]). `None` for a schedule that runs until
    /// `StopCanary` is called.
    pub stop_due: Option<SystemTime>,
    pub created: SystemTime,
    pub last_modified: SystemTime,
    pub last_started: Option<SystemTime>,
    pub last_stopped: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
    /// The browser types the canary was created for, in the order supplied.
    pub browser_types: Vec<String>,
    /// Newest run first, the order `GetCanaryRuns` reports.
    pub runs: Vec<StoredRun>,
    /// The dry runs started for this canary, newest first.
    pub dry_runs: Vec<StoredDryRun>,
}

impl StoredCanary {
    /// This canary's ARN: `arn:aws:synthetics:<region>:<account>:canary:<name>`.
    pub fn arn(&self) -> String {
        canary_arn(&self.region, &self.name)
    }

    /// Materialise every transition the clock already owes at `now`, and nothing else.
    ///
    /// Called by reporting and mutating operations alike: these changes were due at this
    /// instant whether or not anybody called, so applying them on a read does not change
    /// what a later caller sees. That includes retention expiry: a run whose data the
    /// canary's retention period no longer covers was already out of range at this
    /// instant, so it is dropped here rather than when somebody happens to look.
    pub fn settle(&mut self, now: SystemTime) {
        if self.state == CanaryState::Creating
            && self.ready_due.is_some_and(|due| now >= due)
        {
            self.state = CanaryState::Ready;
            self.state_reason = None;
            self.state_reason_code = None;
            self.ready_due = None;
        }
        for run in &mut self.runs {
            if run.state == CanaryRunState::Running
                && run.outcome_due.is_some_and(|due| now >= due)
            {
                // The verdict is the one the run's own work produced, recorded by the call
                // that opened the run; a run whose outcome nobody recorded reports that it
                // has no result rather than a verdict nothing established.
                let outcome = run
                    .outcome
                    .take()
                    .unwrap_or_else(|| RunOutcome::execution_failure(RUN_UNRECORDED_REASON));
                run.state = outcome.state;
                run.state_reason = outcome.state_reason;
                run.state_reason_code = outcome.state_reason_code;
                run.completed = Some(run.outcome_due.unwrap_or(now));
                run.outcome_due = None;
            }
        }
        // A canary whose schedule bounds how long it runs stops on its own once that bound
        // has passed and the runs it started are finished: `rate(0 …)` "causes the canary to
        // run only once when it is started", and a non-zero `DurationInSeconds` is how long
        // "the canary [continues] making regular runs according to the schedule".
        if self.state == CanaryState::Running
            && self.stop_due.is_some_and(|due| now >= due)
            && !self.has_open_canary_run()
        {
            self.last_stopped = Some(self.stop_due.unwrap_or(now));
            self.state = CanaryState::Stopped;
            self.state_reason = None;
            self.state_reason_code = None;
            self.stop_due = None;
        }
        self.expire_runs(now);
    }

    /// Whether a run of the canary's own schedule is still open. A dry run is not one: it
    /// exercises its own configuration independently of the schedule.
    fn has_open_canary_run(&self) -> bool {
        self.runs
            .iter()
            .any(|run| run.state == CanaryRunState::Running && run.dry_run_id.is_none())
    }

    /// Drop every finished run the canary's retention period no longer covers.
    ///
    /// `CreateCanaryRequest$SuccessRetentionPeriodInDays` /
    /// `$FailureRetentionPeriodInDays`: "The number of days to retain data about
    /// successful [failed] runs of this canary. … This setting affects the range of
    /// information returned by GetCanaryRuns".
    fn expire_runs(&mut self, now: SystemTime) {
        // Copied out first: the closure below cannot borrow `self` while `self.runs` is
        // borrowed mutably.
        let success_days = self.success_retention_period_in_days.max(0) as u64;
        let failure_days = self.failure_retention_period_in_days.max(0) as u64;
        self.runs.retain(|run| {
            let Some(completed) = run.completed else {
                // Still open — a `RUNNING` run never carries `completed` — so there is
                // nothing retained yet to expire.
                return true;
            };
            let days = if run.state == CanaryRunState::Passed {
                success_days
            } else {
                failure_days
            };
            let ttl = Duration::from_secs(days.saturating_mul(SECONDS_PER_DAY));
            match now.duration_since(completed) {
                Ok(age) => age < ttl,
                // Completed in this instant's future (a clock that stepped back): keep it.
                Err(_) => true,
            }
        });
    }

    /// Record the start of a run, keeping the history within
    /// [`MAX_RETAINED_RUNS_PER_CANARY`] so a start/stop loop cannot grow it without limit.
    ///
    /// The history is newest-first, so the entries dropped are the oldest.
    pub fn begin_run(&mut self, run: StoredRun) {
        self.runs.insert(0, run);
        self.runs.truncate(MAX_RETAINED_RUNS_PER_CANARY);
    }

    /// Record a dry run of this canary and the run it starts, keeping both histories
    /// within [`MAX_RETAINED_RUNS_PER_CANARY`] so a dry-run loop cannot grow them without
    /// limit.
    ///
    /// Nothing about the canary itself changes: a dry run exercises its own configuration
    /// and is not a run of the canary's schedule, so the canary's state, its last-started
    /// instant and its configuration are left exactly as they were.
    pub fn begin_dry_run(&mut self, dry_run: StoredDryRun, run: StoredRun) {
        self.dry_runs.insert(0, dry_run);
        self.dry_runs.truncate(MAX_RETAINED_RUNS_PER_CANARY);
        self.begin_run(run);
    }

    /// The dry run `dry_run_id` names, if this canary has one.
    pub fn dry_run(&self, dry_run_id: &str) -> Option<&StoredDryRun> {
        self.dry_runs.iter().find(|dry_run| dry_run.id == dry_run_id)
    }

    /// `DryRunConfigOutput` for one of this canary's dry runs: its id, and
    /// `LastDryRunExecutionStatus` — "the last execution status for a canary's dry run" —
    /// read from the newest run that dry run started, so the status reported is the state
    /// of a run this store holds rather than a value chosen when the dry run began.
    pub fn dry_run_config_output(&self, dry_run: &StoredDryRun) -> DryRunConfigOutput {
        DryRunConfigOutput {
            dry_run_id: Some(dry_run.id.clone()),
            // The history is newest first, so the first match is the latest run.
            last_dry_run_execution_status: self
                .runs
                .iter()
                .find(|run| run.dry_run_id.as_deref() == Some(dry_run.id.as_str()))
                .map(|run| run.state.as_wire().to_string()),
        }
    }

    /// Project this canary as one of its dry runs configures it: every configuration
    /// member comes from the dry run's snapshot, while identity, status and timeline come
    /// from the live record — a dry run carries a configuration, it is not a second canary.
    pub fn to_dry_run_canary(&self, dry_run: &StoredDryRun) -> Canary {
        let live = self.to_canary();
        let mut canary = dry_run.config.to_canary();
        canary.id = live.id;
        canary.name = live.name;
        canary.status = live.status;
        canary.timeline = live.timeline;
        canary.tags = live.tags;
        canary.dry_run_config = Some(self.dry_run_config_output(dry_run));
        canary
    }

    /// Finish whichever run is still open, because the canary was stopped.
    ///
    /// A dry run is left alone: it runs its own configuration independently of the
    /// canary's schedule, so stopping the canary does not stop it.
    pub fn close_open_run(&mut self, now: SystemTime) {
        for run in &mut self.runs {
            if run.state == CanaryRunState::Running && run.dry_run_id.is_none() {
                // The run was cut short, so whatever its own work established is not what
                // happened to it: the stop is.
                run.outcome = None;
                run.state = CanaryRunState::Failed;
                run.state_reason = Some(RUN_INTERRUPTED_REASON.to_string());
                run.state_reason_code = Some(RUN_EXECUTION_FAILURE_REASON_CODE.to_string());
                run.completed = Some(now);
                run.outcome_due = None;
            }
        }
    }

    /// Project this record onto the modelled `Canary` shape. The ONLY way a handler
    /// produces a `Canary`, so `CreateCanary`, `GetCanary` and `DescribeCanaries` cannot
    /// disagree about the same record.
    pub fn to_canary(&self) -> Canary {
        Canary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            code: Some(CanaryCodeOutput {
                source_location_arn: Some(self.code.source_location_arn.clone()),
                handler: self.code.handler.clone(),
                blueprint_types: (!self.code.blueprint_types.is_empty())
                    .then(|| self.code.blueprint_types.clone()),
                dependencies: (!self.code.dependencies.is_empty()).then(|| {
                    self.code
                        .dependencies
                        .iter()
                        .map(|dep| Dependency {
                            r#type: dep.r#type.clone(),
                            reference: dep.reference.clone(),
                        })
                        .collect()
                }),
            }),
            execution_role_arn: Some(self.execution_role_arn.clone()),
            schedule: Some(CanaryScheduleOutput {
                expression: Some(self.schedule.expression.clone()),
                duration_in_seconds: Some(self.schedule.duration_in_seconds),
                retry_config: self.schedule.max_retries.map(|max_retries| RetryConfigOutput {
                    max_retries: Some(max_retries),
                }),
            }),
            run_config: Some(CanaryRunConfigOutput {
                timeout_in_seconds: Some(self.run_config.timeout_in_seconds),
                memory_in_mb: self.run_config.memory_in_mb,
                active_tracing: self.run_config.active_tracing,
                ephemeral_storage: self.run_config.ephemeral_storage,
            }),
            success_retention_period_in_days: Some(self.success_retention_period_in_days),
            failure_retention_period_in_days: Some(self.failure_retention_period_in_days),
            status: Some(CanaryStatus {
                state: Some(self.state.as_wire().to_string()),
                state_reason: self.state_reason.clone(),
                state_reason_code: self.state_reason_code.clone(),
            }),
            timeline: Some(CanaryTimeline {
                created: Some(self.created.into()),
                last_modified: Some(self.last_modified.into()),
                last_started: self.last_started.map(Into::into),
                last_stopped: self.last_stopped.map(Into::into),
            }),
            artifact_s3_location: Some(self.artifact_s3_location.clone()),
            engine_arn: Some(self.engine_arn.clone()),
            runtime_version: Some(self.runtime_version.clone()),
            vpc_config: self.vpc_config.as_ref().map(|vpc| VpcConfigOutput {
                // Absent on purpose: the real service resolves the VPC from the subnets,
                // and this emulator has no subnet inventory to resolve it from.
                vpc_id: None,
                subnet_ids: Some(vpc.subnet_ids.clone()),
                security_group_ids: Some(vpc.security_group_ids.clone()),
                ipv6_allowed_for_dual_stack: vpc.ipv6_allowed_for_dual_stack,
            }),
            provisioned_resource_cleanup: Some(self.provisioned_resource_cleanup.clone()),
            browser_configs: (!self.browser_types.is_empty()).then(|| {
                self.browser_types
                    .iter()
                    .map(|browser_type| BrowserConfig {
                        browser_type: Some(browser_type.clone()),
                    })
                    .collect()
            }),
            tags: Some(to_tag_map(&self.tags)),
            artifact_config: self.artifact_config.as_ref().map(|cfg| ArtifactConfigOutput {
                s3_encryption: Some(S3EncryptionConfig {
                    encryption_mode: cfg.encryption_mode.clone(),
                    kms_key_arn: cfg.kms_key_arn.clone(),
                }),
            }),
            kms_key_arn: self.kms_key_arn.clone(),
            ..Default::default()
        }
    }
}

/// One group, as this emulator persists it, and the canaries associated with it. Every
/// reported field is read from here.
///
/// A group is a plain container: the model gives it an id, a name, an ARN, tags and the
/// two timestamps, and no status — `CreateGroup` returns it ready, so there is no
/// transition the clock owes it.
#[derive(Debug, Clone)]
pub struct StoredGroup {
    pub id: String,
    pub name: String,
    /// The Region the create was signed for, which is the Region in the group's ARN:
    /// "Although the group ARN format reflects the Region name where it was created, a
    /// group is not constrained to any Region." It is also the group's HOME Region, the
    /// only one `DeleteGroup` accepts a delete from.
    pub region: String,
    pub created: SystemTime,
    pub last_modified: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// The ARNs of the canaries associated with this group, which `AssociateResource`
    /// writes and `ListGroupResources` reports.
    ///
    /// A SET, because a canary is either a member or it is not — associating one twice
    /// cannot make it a member twice, which is why a second association of one pair is
    /// REFUSED rather than made ([`canary_already_in_group`]) — and an ordered one, because
    /// `ListGroupResources` pages over this collection and a tokenised page boundary needs a
    /// total, stable order.
    pub resources: BTreeSet<String>,
}

impl StoredGroup {
    /// This group's ARN: `arn:aws:synthetics:<region>:<account>:group:<id>`.
    pub fn arn(&self) -> String {
        group_arn(&self.region, &self.id)
    }

    /// Project this record onto the modelled `Group` shape. The ONLY way a handler
    /// produces a `Group`, so `CreateGroup` and `GetGroup` cannot disagree about the
    /// same record.
    pub fn to_group(&self) -> Group {
        Group {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            arn: Some(self.arn()),
            tags: Some(to_tag_map(&self.tags)),
            created_time: Some(self.created.into()),
            last_modified_time: Some(self.last_modified.into()),
        }
    }

    /// Project this record onto the modelled `GroupSummary` shape — the group's identity
    /// without its tags or timestamps, which is every member `GroupSummary` declares
    /// (`smithy_query(query="shape",shape="GroupSummary")`). The ONLY way a handler
    /// produces a `GroupSummary`, so the summary a listing reports and the `Group` a
    /// reader reports cannot disagree about the same record.
    pub fn to_group_summary(&self) -> GroupSummary {
        GroupSummary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            arn: Some(self.arn()),
        }
    }
}

/// Per-service state for com-amazonaws-synthetics-synthetics. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSyntheticsSyntheticsState {
    /// Canaries by name — the identity the API addresses them by.
    pub canaries: BTreeMap<String, StoredCanary>,
    /// Groups by name. "The names for all groups in your account, across all Regions,
    /// must be unique" (`CreateGroupRequest$Name`), so the name is a key; a [`BTreeMap`]
    /// because `ListGroups` and `ListGroupResources` page over this collection and a
    /// tokenised page boundary needs a total, stable order.
    pub groups: BTreeMap<String, StoredGroup>,
}

impl ComAmazonawsSyntheticsSyntheticsState {
    /// Apply [`StoredCanary::settle`] to every record, so any operation starts from a
    /// store in which the clock's own transitions have already happened.
    pub fn settle(&mut self, now: SystemTime) {
        for canary in self.canaries.values_mut() {
            canary.settle(now);
        }
    }

    /// The group a `GroupIdentifier` names, if the store holds it.
    ///
    /// `GetGroupRequest$GroupIdentifier` (and every other group-addressing member):
    /// "You can specify the group name, the ARN, or the group ID as the GroupIdentifier",
    /// so exactly those three forms resolve to the same record. An ARN identifies a group
    /// only when it is THAT group's whole ARN: the partition, the service, the Region and
    /// the account are part of a resource's identity, so an ARN naming another service or
    /// another account resolves to nothing rather than to the local group that happens to
    /// share its resource segment.
    pub fn group_by_identifier(&self, identifier: &str) -> Option<&StoredGroup> {
        if let Some(group) = self.groups.get(identifier) {
            return Some(group);
        }
        if identifier.starts_with(ARN_PREFIX) {
            return self
                .groups
                .values()
                .find(|group| group.arn() == identifier);
        }
        self.groups
            .values()
            .find(|group| group.id == identifier)
    }

    /// The key in [`Self::groups`] a `GroupIdentifier` names, for a caller that has to
    /// reach the record MUTABLY. One resolution rule, [`Self::group_by_identifier`]'s, so a
    /// write cannot address a different group than a read.
    pub fn group_key(&self, identifier: &str) -> Option<String> {
        self.group_by_identifier(identifier)
            .map(|group| group.name.clone())
    }

    /// The canary a `CanaryArn` names, if this store holds it.
    ///
    /// Keyed off the stored record's own ARN, which spells the Region the canary was
    /// created in — so an ARN naming another Region, or another account, matches nothing.
    /// That is also how `AssociateResource`'s "You must run this operation in the Region
    /// where the canary exists" falls out: a canary that does not exist here is reported
    /// as not found.
    pub fn canary_by_arn(&self, arn: &str) -> Option<&StoredCanary> {
        self.canaries.values().find(|canary| canary.arn() == arn)
    }

    /// The groups the canary at `resource_arn` is currently a member of, in group-name
    /// order — [`Self::groups`] is ordered, so this has a total, stable order and a
    /// tokenised page boundary over it means the same thing on the next call.
    ///
    /// Membership is stored on the group side ([`StoredGroup::resources`]), so this is the
    /// one place the relation is read the other way round, which is what
    /// `ListAssociatedGroups` reports and what [`Self::membership_count`] counts.
    pub fn groups_of_resource(&self, resource_arn: &str) -> Vec<&StoredGroup> {
        self.groups
            .values()
            .filter(|group| group.resources.contains(resource_arn))
            .collect()
    }

    /// How many groups the canary at `resource_arn` is currently a member of.
    pub fn membership_count(&self, resource_arn: &str) -> usize {
        self.groups_of_resource(resource_arn).len()
    }

    /// Drop `resource_arn` from every group that holds it, because the resource itself is
    /// gone: an association can only name a canary that exists.
    pub fn forget_resource(&mut self, resource_arn: &str) {
        for group in self.groups.values_mut() {
            group.resources.remove(resource_arn);
        }
    }

    /// The group a `ResourceArn` names, if this store holds it.
    ///
    /// ARN-only, unlike [`Self::group_by_identifier`]: the tag operations address a
    /// resource by `ResourceArn` and never by name or bare id, so a group whose NAME
    /// happens to spell an ARN is not reachable through this lookup. Keyed off the stored
    /// record's own ARN, so an ARN naming another Region or another account matches
    /// nothing.
    pub fn group_by_arn(&self, arn: &str) -> Option<&StoredGroup> {
        self.groups.values().find(|group| group.arn() == arn)
    }

    /// The tags of the canary or group a `ResourceArn` names, or `None` when this store
    /// holds no such resource.
    ///
    /// `ResourceArn`'s pattern pins the resource segment to `canary:…` or `group:…`
    /// (`smithy_query(query="shape",shape="ResourceArn")`), so the ARN itself says which
    /// collection it resolves against, and the whole ARN — partition, Region and account
    /// included — has to be that record's own: tags hang off a resource's identity, not
    /// off its name. One lookup, so every tag operation addresses the same record as
    /// every other.
    pub fn tags_of_resource(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        match arn_resource_kind(resource_arn)? {
            TaggedResourceKind::Canary => {
                self.canary_by_arn(resource_arn).map(|canary| &canary.tags)
            }
            TaggedResourceKind::Group => self.group_by_arn(resource_arn).map(|group| &group.tags),
        }
    }

    /// [`Self::tags_of_resource`], for a caller that has to CHANGE the tags: the very same
    /// stored field, resolved by the very same rule, so a tag write lands on the record a
    /// tag read reports and on the `Tags` member the resource's own reader projects.
    pub fn tags_of_resource_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        match arn_resource_kind(resource_arn)? {
            TaggedResourceKind::Canary => {
                // The key is read off the matching record, so the mutable lookup below
                // cannot address a different canary than the immutable one.
                let name = self.canary_by_arn(resource_arn)?.name.clone();
                self.canaries.get_mut(&name).map(|canary| &mut canary.tags)
            }
            TaggedResourceKind::Group => {
                let name = self.group_by_arn(resource_arn)?.name.clone();
                self.groups.get_mut(&name).map(|group| &mut group.tags)
            }
        }
    }
}

/// Project a stored tag map onto the modelled `TagMap`.
///
/// ALWAYS a map, including for a resource that carries no tags: an output tag map is a
/// member the service reports whenever it reports the resource, and "no tags" is expressed
/// as an EMPTY map rather than as a missing member, so a client reading the member finds a
/// map it can read, write back and compare either way. The modelled `length 1..50` bounds
/// a tag map a CALLER sends — how many tags one call may carry and that it may not be
/// empty — and is not a bound on this projection.
///
/// The one place a `TagMap` is built from stored tags, so `Canary$Tags`, `Group$Tags` and
/// `ListTagsForResource` cannot disagree about the same stored field.
pub fn to_tag_map(tags: &BTreeMap<String, String>) -> HashMap<String, String> {
    tags.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// How many tags one canary or group carries at once: "You can associate as many as 50
/// tags with a canary or group" (`TagResource`'s own documentation), which is also the
/// upper bound the model puts on a `TagMap`.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Which kind of resource a `ResourceArn` names. Its pattern admits exactly these two
/// (`…:\d{12}:(canary|group):[0-9a-z_\-]+$`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResourceKind {
    Canary,
    Group,
}

/// The kind of resource `arn` names, or `None` when its resource segment names neither.
///
/// An ARN's sixth colon-separated field is the resource type
/// (`arn:partition:service:region:account-id:resource-type:resource-id`), which for this
/// service's `ResourceArn` is `canary` or `group`.
pub fn arn_resource_kind(arn: &str) -> Option<TaggedResourceKind> {
    match arn.split(':').nth(5)? {
        "canary" => Some(TaggedResourceKind::Canary),
        "group" => Some(TaggedResourceKind::Group),
        _ => None,
    }
}

/// `NotFoundException$Message` for a `ResourceArn` a tag operation cannot resolve.
///
/// One sentence for both resource kinds, and it names neither the kind nor the ARN: the
/// recorded service answers a `ListTagsForResource` whose `ResourceArn` it cannot resolve
/// with exactly this, where the readers addressed by name or `GroupIdentifier` word the
/// same miss per resource kind ([`CANARY_NOT_FOUND`], [`GROUP_NOT_FOUND`]). It carries
/// nothing from the request, so the ARN the caller supplied is never echoed back.
pub const TAGGED_RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// Every ARN starts with this, which is what tells an ARN-form identifier apart from a
/// group name or a group id.
pub const ARN_PREFIX: &str = "arn:";

/// A group's ARN: `arn:aws:synthetics:<region>:<account>:group:<id>`.
///
/// `Group$Arn` is modelled
/// `^arn:(aws[a-zA-Z-]*)?:synthetics:[a-z]{2,4}(-[a-z]{2,4})?-[a-z]+-\d{1}:\d{12}:group:[0-9a-z]+$`,
/// so the resource segment carries the group's ID — not its name, which may hold "any
/// Unicode characters".
pub fn group_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("synthetics", region, &format!("group:{id}"))
}

/// `RuntimeVersion$Language` for a runtime version whose canaries are written in Node.js.
const LANGUAGE_NODEJS: &str = "nodejs";

/// `RuntimeVersion$Language` for a runtime version whose canaries are written in Python.
const LANGUAGE_PYTHON: &str = "python";

/// What the name of a Python runtime family carries between the `syn-` prefix and the
/// automation library it bundles (`syn-python-selenium-5.1`); every other published family
/// is a Node.js one (`syn-nodejs-puppeteer-9.0`, `syn-nodejs-playwright-1.0`,
/// `syn-nodejs-2.2`, `syn-1.0`).
const PYTHON_FAMILY_SEGMENT: &str = "-python-";

/// The language canary scripts on `version_name` are written in.
///
/// A property of the runtime FAMILY, not of one catalogued entry: a version name names its
/// family, so the language is read off the name rather than restated per entry and cannot
/// disagree with it.
pub fn runtime_language(version_name: &str) -> &'static str {
    if version_name.contains(PYTHON_FAMILY_SEGMENT) {
        LANGUAGE_PYTHON
    } else {
        LANGUAGE_NODEJS
    }
}

/// How a description opens when the version it describes is a patch release of a family
/// already published: such a release carries the dependency updates for the vulnerabilities
/// fixed since the `.0` release, and the published description says so before listing the
/// dependencies.
const SECURITY_FIX_SENTENCE: &str = "Security fixes. ";

/// How a description introduces the runtime's dependency list, which is what the published
/// description of a runtime version carries.
const DEPENDENCIES_LABEL: &str = "Dependencies: ";

/// Whether `version_name` is a PATCH release of its family: a published runtime version ends
/// in `<major>.<minor>`, and a non-zero minor is a patch release of the `<major>.0` one.
fn is_patch_release(version_name: &str) -> bool {
    version_name
        .rsplit_once('-')
        .and_then(|(_, version)| version.split('.').nth(1))
        .and_then(|minor| minor.parse::<u32>().ok())
        .is_some_and(|minor| minor > 0)
}

/// `RuntimeVersion$Description` — "A description of the runtime version, created by Amazon".
///
/// The published description of a runtime version lists the dependencies that version
/// bundles, prefixed with [`SECURITY_FIX_SENTENCE`] when the version is a patch release of
/// an already published family. Derived rather than stored, so the catalog records each
/// version's dependencies once and every description reads the same way.
fn runtime_description(version_name: &str, dependencies: &str) -> String {
    let security_fixes = if is_patch_release(version_name) {
        SECURITY_FIX_SENTENCE
    } else {
        ""
    };
    format!("{security_fixes}{DEPENDENCIES_LABEL}{dependencies}")
}

/// The RFC 3339 form every catalogued date is written in, and the form the modelled
/// `Timestamp` serialises as.
const CATALOG_DATE_FORMAT: ::aws_smithy_types::date_time::Format =
    ::aws_smithy_types::date_time::Format::DateTime;

/// One canary runtime version this emulator's Synthetics publishes, as
/// `DescribeRuntimeVersions` reports it. Dates are held as RFC 3339 text so the catalog
/// below reads like the published table, and are parsed by
/// [`RuntimeVersionEntry::to_runtime_version`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeVersionEntry {
    /// `RuntimeVersion$VersionName` — the value a canary carries as its `RuntimeVersion`.
    pub version_name: &'static str,
    /// What this version bundles, as the published description lists it: the language
    /// runtime and the browser-automation library a canary on this version runs against, at
    /// the line the runtime versions page gives for it. The exact patch builds of each
    /// dependency are not restated here, so nothing is reported that this emulator does not
    /// know; [`runtime_description`] renders this into the description.
    pub dependencies: &'static str,
    /// `RuntimeVersion$ReleaseDate` — "The date that the runtime version was released".
    pub release_date: &'static str,
    /// `RuntimeVersion$DeprecationDate` — "If this runtime version is deprecated, this value
    /// is the date of deprecation", so `None` for a version that is not deprecated.
    pub deprecation_date: Option<&'static str>,
}

impl RuntimeVersionEntry {
    /// Project this entry onto the modelled `RuntimeVersion` shape — the ONE place a
    /// `RuntimeVersion` is built, so every reader of the catalog reports it identically.
    ///
    /// A date that is not RFC 3339 is reported absent rather than invented; the catalog's
    /// own test asserts every entry carries a parseable `ReleaseDate`, so an unparseable
    /// literal fails there instead of reaching a caller.
    pub fn to_runtime_version(&self) -> RuntimeVersion {
        RuntimeVersion {
            version_name: Some(self.version_name.to_string()),
            description: Some(runtime_description(self.version_name, self.dependencies)),
            language: Some(runtime_language(self.version_name).to_string()),
            release_date: catalog_date(self.release_date),
            deprecation_date: self.deprecation_date.and_then(catalog_date),
        }
    }
}

/// An RFC 3339 catalog date as a modelled timestamp.
///
/// A literal that is not RFC 3339 reports the member absent rather than inventing a date;
/// the `debug_assert` keeps that failure local to the catalog (and this crate's tests assert
/// every entry carries a parseable `ReleaseDate`).
fn catalog_date(date: &str) -> Option<::aws_smithy_types::DateTime> {
    let parsed = ::aws_smithy_types::DateTime::from_str(date, CATALOG_DATE_FORMAT).ok();
    debug_assert!(
        parsed.is_some(),
        "a catalogued runtime version date must be RFC 3339, got {date:?}"
    );
    parsed
}

/// Every canary runtime version this service publishes, NEWEST FIRST — the order the
/// published catalog is reported in, and a total, stable one, so a
/// `DescribeRuntimeVersions` page boundary means the same thing on the next call.
///
/// This is the ONE catalog: `DescribeRuntimeVersions` reports it
/// ([`RuntimeVersionEntry::to_runtime_version`]) and canary creation validates against it
/// ([`is_published_runtime_version`]), so the versions a client discovers here and the
/// versions a canary may be created on cannot disagree.
///
/// Source: the CloudWatch User Guide's canary runtime versions page, which the model's own
/// `RuntimeVersion` doc links to
/// (<https://docs.aws.amazon.com/AmazonCloudWatch/latest/monitoring/CloudWatch_Synthetics_Canaries_Library.html>);
/// it is that page that names the runtime versions, the dependencies each bundles and the
/// release of each. A release the page dates by month is recorded here as the first day of
/// that month, and the entries are listed by release date so the newest runtime of every
/// family — Node.js with Playwright, Node.js with Puppeteer, Python with Selenium, and the
/// retired `syn-nodejs-2.x` / `syn-1.0` ones the page still names — comes first.
///
/// Only versions that are OFFERED are catalogued: a canary cannot be created on a runtime
/// this emulator does not offer, so nothing in the list is deprecated and every entry's
/// `DeprecationDate` is absent — which is exactly what the member's own doc asks for ("If
/// this runtime version is deprecated, this value is the date of deprecation").
pub const RUNTIME_VERSION_CATALOG: &[RuntimeVersionEntry] = &[
    RuntimeVersionEntry {
        version_name: "syn-nodejs-playwright-3.0",
        dependencies: "Node JS 22.x, Playwright 1.x",
        release_date: "2025-08-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-playwright-2.0",
        dependencies: "Node JS 22.x, Playwright 1.x",
        release_date: "2025-05-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-11.0",
        dependencies: "Node JS 22.x, Puppeteer-core 24.x",
        release_date: "2025-05-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-10.0",
        dependencies: "Node JS 20.x, Puppeteer-core 23.x",
        release_date: "2025-02-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-python-selenium-5.1",
        dependencies: "Python 3.9, Selenium 4.x",
        release_date: "2025-02-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-9.1",
        dependencies: "Node JS 20.x, Puppeteer-core 22.x",
        release_date: "2024-12-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-playwright-1.0",
        dependencies: "Node JS 20.x, Playwright 1.x",
        release_date: "2024-11-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-9.0",
        dependencies: "Node JS 20.x, Puppeteer-core 22.x",
        release_date: "2024-10-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-python-selenium-5.0",
        dependencies: "Python 3.9, Selenium 4.x",
        release_date: "2024-10-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-8.0",
        dependencies: "Node JS 20.x, Puppeteer-core 22.x",
        release_date: "2024-08-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-python-selenium-4.1",
        dependencies: "Python 3.9, Selenium 4.x",
        release_date: "2024-08-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-7.0",
        dependencies: "Node JS 18.x, Puppeteer-core 21.x",
        release_date: "2024-05-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-python-selenium-4.0",
        dependencies: "Python 3.9, Selenium 4.x",
        release_date: "2024-05-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-6.2",
        dependencies: "Node JS 18.x, Puppeteer-core 19.x",
        release_date: "2024-02-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-6.1",
        dependencies: "Node JS 18.x, Puppeteer-core 19.x",
        release_date: "2023-11-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-python-selenium-3.0",
        dependencies: "Python 3.8, Selenium 4.x",
        release_date: "2023-11-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-6.0",
        dependencies: "Node JS 18.x, Puppeteer-core 19.x",
        release_date: "2023-08-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-python-selenium-2.1",
        dependencies: "Python 3.8, Selenium 3.x",
        release_date: "2023-08-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-5.2",
        dependencies: "Node JS 16.x, Puppeteer-core 5.x",
        release_date: "2023-06-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-5.1",
        dependencies: "Node JS 16.x, Puppeteer-core 5.x",
        release_date: "2023-03-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-python-selenium-2.0",
        dependencies: "Python 3.8, Selenium 3.x",
        release_date: "2023-02-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-5.0",
        dependencies: "Node JS 16.x, Puppeteer-core 5.x",
        release_date: "2023-01-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-4.0",
        dependencies: "Node JS 14.x, Puppeteer-core 5.x",
        release_date: "2022-11-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-3.9",
        dependencies: "Node JS 12.x, Puppeteer-core 5.x",
        release_date: "2022-08-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-3.8",
        dependencies: "Node JS 12.x, Puppeteer-core 5.x",
        release_date: "2022-06-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-3.7",
        dependencies: "Node JS 12.x, Puppeteer-core 5.x",
        release_date: "2022-05-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-python-selenium-1.3",
        dependencies: "Python 3.8, Selenium 3.x",
        release_date: "2022-05-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-3.6",
        dependencies: "Node JS 12.x, Puppeteer-core 5.x",
        release_date: "2022-04-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-3.5",
        dependencies: "Node JS 12.x, Puppeteer-core 5.x",
        release_date: "2022-02-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-python-selenium-1.2",
        dependencies: "Python 3.8, Selenium 3.x",
        release_date: "2022-02-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-3.4",
        dependencies: "Node JS 12.x, Puppeteer-core 5.x",
        release_date: "2021-12-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-3.3",
        dependencies: "Node JS 12.x, Puppeteer-core 5.x",
        release_date: "2021-10-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-python-selenium-1.1",
        dependencies: "Python 3.8, Selenium 3.x",
        release_date: "2021-10-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-3.2",
        dependencies: "Node JS 12.x, Puppeteer-core 5.x",
        release_date: "2021-07-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-3.1",
        dependencies: "Node JS 12.x, Puppeteer-core 5.x",
        release_date: "2021-05-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-puppeteer-3.0",
        dependencies: "Node JS 12.x, Puppeteer-core 5.x",
        release_date: "2021-02-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-python-selenium-1.0",
        dependencies: "Python 3.8, Selenium 3.x",
        release_date: "2021-02-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-2.2",
        dependencies: "Node JS 10.x, Puppeteer-core 3.x",
        release_date: "2021-01-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-2.1",
        dependencies: "Node JS 10.x, Puppeteer-core 3.x",
        release_date: "2020-11-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-nodejs-2.0",
        dependencies: "Node JS 10.x, Puppeteer-core 3.x",
        release_date: "2020-10-01T00:00:00Z",
        deprecation_date: None,
    },
    RuntimeVersionEntry {
        version_name: "syn-1.0",
        dependencies: "Node JS 10.x, Puppeteer-core 1.x",
        release_date: "2020-06-01T00:00:00Z",
        deprecation_date: None,
    },
];

/// Where `version_name` sits in [`RUNTIME_VERSION_CATALOG`], or `None` when the catalog does
/// not offer it. The page-token reader uses this, so a token can only resume at a version
/// this service itself published.
pub fn runtime_version_position(version_name: &str) -> Option<usize> {
    RUNTIME_VERSION_CATALOG
        .iter()
        .position(|entry| entry.version_name == version_name)
}

/// The modelled maximum length of `Token`, the shape every paged operation of this service
/// targets with its `NextToken` (`len 4..252`).
pub const MAX_TOKEN_LEN: usize = 252;

/// The prefix of a page token that resumes at an OFFSET — the one form that still works for
/// a key too long to carry inside [`MAX_TOKEN_LEN`]. One constant, so every paged operation
/// writes the same token form.
pub const TOKEN_OFFSET_PREFIX: &str = "ix#";

/// Where a page starts within an ordered collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageStart {
    /// Resume at this key: exact across inserts and deletes, because the page boundary is a
    /// key in the ordered collection rather than a position in it.
    Key(String),
    /// Resume at this position — the fallback for a key a token cannot carry.
    Offset(usize),
}

/// The token that resumes the next page after `offset` entries, whose first entry has the
/// key `next_key`.
///
/// `key_prefix` is the issuing operation's own prefix, so the two paged operations share one
/// token FORM without sharing a token: a token issued over one collection does not decode
/// over the other. The offset form is used only when prefix + key would exceed the modelled
/// token length.
pub fn encode_page_token(key_prefix: &str, offset: usize, next_key: &str) -> String {
    if key_prefix.len() + next_key.len() <= MAX_TOKEN_LEN {
        format!("{key_prefix}{next_key}")
    } else {
        format!("{TOKEN_OFFSET_PREFIX}{offset}")
    }
}

/// Read a token [`encode_page_token`] issued for `key_prefix` over a collection of `len`
/// entries. `None` means the token is a BAD REQUEST, never an empty page — the caller
/// answers it with the operation's modelled `ValidationException`.
///
/// `key_is_issuable` recognises a key this operation could itself have put in a token, and an
/// offset must land inside the collection, so a forged token resumes nowhere.
pub fn decode_page_token(
    key_prefix: &str,
    token: &str,
    len: usize,
    key_is_issuable: impl FnOnce(&str) -> bool,
) -> Option<PageStart> {
    if let Some(key) = token.strip_prefix(key_prefix) {
        return key_is_issuable(key).then(|| PageStart::Key(key.to_string()));
    }
    token
        .strip_prefix(TOKEN_OFFSET_PREFIX)
        .and_then(|offset| offset.parse::<usize>().ok())
        .filter(|offset| *offset <= len)
        .map(PageStart::Offset)
}

/// How many entries a page carries: `max_results` when the request names one, otherwise
/// `default` — what the member's own doc says is used when it is omitted.
///
/// Floored at one entry independently of the modelled `@range` the request boundary already
/// enforces: a page size of zero would return nothing while still emitting a token, so a
/// client loop would never terminate.
pub fn page_size(max_results: Option<i32>, default: usize) -> usize {
    max_results
        .and_then(|max| usize::try_from(max).ok())
        .unwrap_or(default)
        .max(1)
}

/// A canary's ARN: `arn:aws:synthetics:<region>:<account>:canary:<name>`.
///
/// The shape of the `ResourceArn` the group and tag operations accept
/// (`^arn:(aws[a-zA-Z-]*)?:synthetics:…:\d{12}:canary:[0-9a-z_\-]{1,255}$`), so those
/// operations can key off these same records once they are implemented.
pub fn canary_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("synthetics", region, &format!("canary:{name}"))
}

/// The ARN of the Lambda function Synthetics provisions for a canary, which the service
/// reports as `Canary$EngineArn`. Synthetics names these `cwsyn-<canary name>-<canary id>`.
pub fn engine_arn(region: &str, name: &str, id: &str) -> String {
    ::engine::emulator_arn("lambda", region, &format!("function:cwsyn-{name}-{id}"))
}

/// The ARN of the Lambda layer Synthetics stores the canary script in, which the service
/// reports as `CanaryCodeOutput$SourceLocationArn`.
pub fn source_location_arn(region: &str, name: &str, id: &str) -> String {
    ::engine::emulator_arn("lambda", region, &format!("layer:cwsyn-{name}-{id}:1"))
}

/// The region this request was signed for, which is the region the resources it creates
/// live in. Falls back to the emulator's default region when the request carries no SigV4
/// credential scope (an unsigned request routed by its host).
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The `target` every audit record this service emits carries, so a consumer can select
/// the access trail without reading the rest of the emulator's traces.
///
/// A state-changing operation emits ONE record, at `INFO`, naming the operation, the
/// caller's presented access key id, the Region the request resolved to, the resource it
/// changed and the outcome. Metadata only — never a request body, a tag value or a
/// credential secret.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// The identity a request presents, for an audit record: the access key id in its SigV4
/// credential scope. `emulator_server::extract_akid` answers `"default"` when the request
/// carries no scope. An access key id NAMES a caller and is not a secret — the signature
/// and the secret key are never read here.
pub fn request_caller(parts: &::http::request::Parts) -> String {
    ::emulator_server::extract_akid(&parts.headers)
}

/// The region an unsigned request is attributed to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The scheme every Amazon S3 location is written with.
pub const S3_LOCATION_SCHEME: &str = "s3://";

/// Amazon S3's bucket naming rules, as the S3 User Guide's "General purpose bucket naming
/// rules" states them: a name is 3–63 characters long, uses only lowercase letters,
/// numbers, periods and hyphens, begins and ends with a letter or a number, must not be
/// formatted as an IP address, must not contain two adjacent periods, must not start with
/// `xn--`, `sthree-` or `amzn-s3-demo-`, and must not end with `-s3alias` or `--ol-s3`.
const S3_BUCKET_MIN_LEN: usize = 3;
const S3_BUCKET_MAX_LEN: usize = 63;
const S3_BUCKET_RESERVED_PREFIXES: [&str; 3] = ["xn--", "sthree-", "amzn-s3-demo-"];
const S3_BUCKET_RESERVED_SUFFIXES: [&str; 2] = ["-s3alias", "--ol-s3"];

/// Whether `location` is a well-formed Amazon S3 location for canary artifacts.
///
/// `CreateCanaryRequest$ArtifactS3Location` — and the same member on
/// `UpdateCanaryRequest` and `StartCanaryDryRunRequest` — documents itself as "The
/// location in Amazon S3 where Synthetics stores artifacts from the test runs of this
/// canary … The name of the Amazon S3 bucket can't include a period (.)"
/// (`smithy_query(query="doc",shape="CreateCanaryRequest",member="ArtifactS3Location")`).
/// So the value names an S3 bucket and an optional key prefix inside it: `s3://<bucket>`
/// or `s3://<bucket>/<key prefix>`, with the bucket component held to Amazon S3's own
/// bucket naming rules (see the constants above) plus that member's extra no-period rule.
/// A value that is not such a location is refused with the modelled
/// `ValidationException`; nothing else about the value is checked here, because the
/// member's length is already a modelled `@length` constraint the request boundary
/// enforces.
pub fn is_s3_artifact_location(location: &str) -> bool {
    let Some(bucket_and_key) = location.strip_prefix(S3_LOCATION_SCHEME) else {
        return false;
    };
    // The bucket is everything up to the first `/`; the rest is the key prefix the
    // artifacts are written under, which may be absent and is otherwise opaque (any
    // sequence of characters is a legal S3 key).
    let bucket = match bucket_and_key.split_once('/') {
        Some((bucket, _key_prefix)) => bucket,
        None => bucket_and_key,
    };
    is_s3_bucket_name(bucket)
}

/// Whether `bucket` satisfies Amazon S3's bucket naming rules AND the no-period rule
/// `ArtifactS3Location` adds to them. See [`is_s3_artifact_location`].
fn is_s3_bucket_name(bucket: &str) -> bool {
    if !(S3_BUCKET_MIN_LEN..=S3_BUCKET_MAX_LEN).contains(&bucket.len()) {
        return false;
    }
    // "The name of the Amazon S3 bucket can't include a period (.)" — the member's own
    // rule, stricter than S3's, which also settles S3's "no two adjacent periods" and
    // "must not be formatted as an IP address" rules: neither form survives it.
    if bucket.contains('.') {
        return false;
    }
    if !bucket
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return false;
    }
    let alphanumeric = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    if !bucket.bytes().next().is_some_and(alphanumeric)
        || !bucket.bytes().next_back().is_some_and(alphanumeric)
    {
        return false;
    }
    if S3_BUCKET_RESERVED_PREFIXES
        .iter()
        .any(|prefix| bucket.starts_with(prefix))
    {
        return false;
    }
    !S3_BUCKET_RESERVED_SUFFIXES
        .iter()
        .any(|suffix| bucket.ends_with(suffix))
}

/// How Synthetics words the refusal of a location that is not a well-formed S3 location.
///
/// One sentence naming the member and the defect class, the way the service states it;
/// it carries nothing from the request, so a rejected value is never echoed back.
pub const ARTIFACT_S3_LOCATION_MALFORMED: &str = "ArtifactS3Location: The location is malformed.";

/// Whether `requested` names a runtime version this service publishes — a lookup in the ONE
/// catalog [`RUNTIME_VERSION_CATALOG`], the same list `DescribeRuntimeVersions` reports, so
/// a client that picks a version out of that answer can create a canary on it.
///
/// `RuntimeVersion` is modelled as a bare `String` with `@length 1..1024` and no enum, and
/// the member documents its value set as something the service publishes rather than
/// something the model carries — "Specifies the runtime version to use for the canary. For
/// a list of valid runtime versions and more information about runtime versions, see
/// Canary Runtime Versions"
/// (`smithy_query(query="doc",shape="CreateCanaryRequest",member="RuntimeVersion")`), hence
/// a server-side catalog.
///
/// An exact match: a runtime version is an identifier the service publishes, not a pattern,
/// so nothing about the value is normalised first.
pub fn is_published_runtime_version(requested: &str) -> bool {
    runtime_version_position(requested).is_some()
}

/// How Synthetics words the refusal of a runtime version it does not publish.
///
/// Built from the request, because the message is the only place the service reports WHICH
/// value it rejected — a caller fixing the request, or falling back to
/// `DescribeRuntimeVersions`, has that value and nothing else to go on.
pub fn unknown_runtime_version(requested: &str) -> String {
    format!("Invalid runtime version specified: {requested}")
}

/// How often `expression` runs the canary, in seconds, when that is determinable.
///
/// `CanaryScheduleInput$Expression` documents the rate form as `rate(<number> <unit>)`
/// with `unit` one of `minute`, `minutes` or `hour`, and `rate(0 minute)` / `rate(0 hour)`
/// as the special "run once" value — which has no frequency, hence `None`. A `cron(...)`
/// expression also yields `None` here: its frequency is not a single interval.
pub fn schedule_frequency_in_seconds(expression: &str) -> Option<i64> {
    let (count, unit) = rate_expression(expression)?;
    if count == 0 {
        return None;
    }
    let seconds_per_unit = match unit {
        "minute" | "minutes" => 60,
        "hour" | "hours" => 3600,
        _ => return None,
    };
    count.checked_mul(seconds_per_unit)
}

/// Whether `expression` is the schedule that runs the canary ONCE.
///
/// `CanaryScheduleInput$Expression`: "Specifying `rate(0 minute)` or `rate(0 hour)` is a
/// special value that causes the canary to run only once when it is started." The count is
/// what makes it that value, so any unit the rate form admits spells it.
pub fn is_run_once_schedule(expression: &str) -> bool {
    rate_expression(expression).is_some_and(|(count, _unit)| count == 0)
}

/// The `<number>` and `<unit>` of a `rate(<number> <unit>)` expression, when `expression` is
/// one. The ONE parse of that form, so the frequency and the run-once readings above cannot
/// disagree about which values are rate expressions.
fn rate_expression(expression: &str) -> Option<(i64, &str)> {
    let body = expression
        .trim()
        .strip_prefix("rate(")
        .and_then(|rest| rest.strip_suffix(')'))?;
    let mut fields = body.split_whitespace();
    let count: i64 = fields.next()?.parse().ok()?;
    let unit = fields.next()?;
    if fields.next().is_some() {
        return None;
    }
    Some((count, unit))
}

/// The `TimeoutInSeconds` a canary gets when `RunConfig.TimeoutInSeconds` is omitted.
///
/// `CanaryRunConfigInput$TimeoutInSeconds`: "If you omit this field, the frequency of the
/// canary is used as this value, up to a maximum of 14 minutes." A schedule with no single
/// frequency (`cron(...)`, or the run-once `rate(0 ...)`) therefore takes that maximum.
pub fn default_run_timeout_in_seconds(expression: &str) -> i32 {
    match schedule_frequency_in_seconds(expression) {
        Some(frequency) => i32::try_from(frequency)
            .unwrap_or(MAX_DEFAULTED_RUN_TIMEOUT_IN_SECONDS)
            .clamp(
                MIN_RUN_TIMEOUT_IN_SECONDS,
                MAX_DEFAULTED_RUN_TIMEOUT_IN_SECONDS,
            ),
        None => MAX_DEFAULTED_RUN_TIMEOUT_IN_SECONDS,
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSyntheticsSyntheticsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSyntheticsSyntheticsState>)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: how long a started canary keeps running is what its own schedule bounds — the
    /// run-once expression bounds it to its single run, a non-zero `DurationInSeconds`
    /// bounds it to that span, and a schedule with neither runs until it is stopped.
    ///
    /// The `For` rows are what an end-to-end assertion cannot reach: the member's range is
    /// `0..31622400` seconds (`smithy_query(query="shape",shape="CanaryScheduleInput")`), so
    /// the shortest bound a client can ask for is a second, and both published ends of that
    /// range are rows here.
    #[test]
    fn a_schedules_running_bound_is_read_from_its_own_members() {
        let rows: &[(&str, &str, i64, RunningBound)] = &[
            ("run once, minutes", "rate(0 minute)", 0, RunningBound::SingleRun),
            ("run once, hours", "rate(0 hour)", 0, RunningBound::SingleRun),
            (
                "run once, with a duration that cannot apply",
                "rate(0 minute)",
                600,
                RunningBound::SingleRun,
            ),
            ("every minute, no duration", "rate(1 minute)", 0, RunningBound::UntilStopped),
            ("a cron schedule, no duration", "cron(0 * * * ? *)", 0, RunningBound::UntilStopped),
            (
                "every minute, for the shortest duration the member admits",
                "rate(1 minute)",
                1,
                RunningBound::For(Duration::from_secs(1)),
            ),
            (
                "every hour, for an hour",
                "rate(1 hour)",
                3600,
                RunningBound::For(Duration::from_secs(3600)),
            ),
            (
                "a cron schedule, for the longest duration the member admits",
                "cron(0 * * * ? *)",
                31_622_400,
                RunningBound::For(Duration::from_secs(31_622_400)),
            ),
            (
                "a duration no member could carry",
                "rate(1 minute)",
                -1,
                RunningBound::UntilStopped,
            ),
        ];
        for (case, expression, duration_in_seconds, expected) in rows {
            let schedule = StoredSchedule {
                expression: (*expression).to_string(),
                duration_in_seconds: *duration_in_seconds,
                max_retries: None,
            };
            let actual = schedule.running_bound();
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
