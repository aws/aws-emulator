// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-emrserverless-awstoledowebservice service.
//!
//! The application is the primary resource of the EMR Serverless control plane, and a
//! job run lives inside one application. [`ApplicationRecord`] is the ONE copy of an
//! application every handler reads and writes: `CreateApplication` mints it,
//! `GetApplication` / `ListApplications` report it, `UpdateApplication` applies the
//! members a request names to it, `StartApplication` /
//! `StopApplication` move its state, `DeleteApplication` terminates it (the record is
//! retained in the terminal `TERMINATED` state rather than dropped, so a client reading
//! the application back after a delete observes that state, while the unfiltered
//! `ListApplications` inventory drops it — see [`is_live_application_state`]), and
//! `StartJobRun` / `GetJobRun` / `ListJobRuns` / `CancelJobRun` work on the job runs it
//! owns. An interactive session lives inside one application the same way a job run
//! does: `StartSession` mints a [`SessionRecord`] on the application it names,
//! `GetSession` reports it, `ListSessions` reports the sessions one application holds a
//! page at a time, and `GetSessionEndpoint` hands a client the endpoint of that
//! stored session together with a freshly minted, time-limited authentication token.
//!
//! Two things here move with the CLOCK rather than with a call, and both are stored as a
//! due instant that the next read materialises (nothing sleeps): the stages a submitted
//! job run passes through before it settles ([`JOB_RUN_STAGES`]), and the automatic stop
//! an application's `autoStopConfiguration` owes it once it has been idle past the idle
//! timeout it names ([`ApplicationRecord::auto_stop_due`]).
//!
//! Each record holds the MODELLED shape (`Application`, `JobRun`) verbatim, so a
//! reporting operation returns exactly what the mutating one stored rather than
//! re-deriving anything from a request.
//!
//! One thing a record owes beyond its own members is the LOG ARTIFACTS a finished job run
//! publishes to the destinations its monitoring configuration names — the Amazon S3
//! destination and the CloudWatch Logs group — which the clock owes at the instant the run
//! finished: [`job_run_logs`] states what is written, where, and which call writes it.

/// The log artifacts a finished job run publishes into the destinations its
/// `monitoringConfiguration` names, and the publication every call that settles this store
/// drains.
pub mod job_run_logs;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt::Debug;
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;
use sha2::{Digest, Sha256};

use crate::ops::types::{
    Application, ApplicationSummary, AutoStartConfig, AutoStopConfig, CloudWatchLoggingConfiguration,
    Configuration, ConflictException, CreateApplicationInput, DiskEncryptionConfiguration,
    IdentityCenterConfiguration, IdentityCenterConfigurationInput, ImageConfiguration,
    ImageConfigurationInput, JobRun, JobRunSummary, JobLevelCostAllocationConfiguration,
    ManagedPersistenceMonitoringConfiguration, MonitoringConfiguration,
    ResourceNotFoundException, Session, SessionSummary, ValidationException,
    WorkerTypeSpecification, WorkerTypeSpecificationInput,
};

/// This service's ARN namespace, and the `sigv4Name` the model declares
/// (`smithy_query(query="service_info")`): every ARN this emulator mints for an
/// application or a job run carries it.
pub const ARN_NAMESPACE: &str = "emr-serverless";

/// The region an ARN is built for when the request carries no SigV4 credential scope
/// to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The largest page any list operation serves, and the page size it uses when the
/// request names no `maxResults`.
///
/// Source: `ListApplicationsRequest$maxResults`, `ListJobRunsRequest$maxResults` and
/// `ListSessionsRequest$maxResults` are all modelled `@range(min: 1, max: 50)` with no
/// declared default.
pub const PAGE_LIMIT: i32 = 50;

/// How many applications the store holds before `CreateApplication` refuses.
///
/// EMR Serverless publishes an adjustable per-account quota on applications, which
/// this emulator does not model, so this is its OWN bound on the store rather than a
/// claim about any account's limit: set far above any workflow an emulator serves,
/// while keeping the state a caller can grow bounded.
pub const MAX_APPLICATIONS: usize = 1_000;

/// The same bound, for the job runs one application holds.
pub const MAX_JOB_RUNS_PER_APPLICATION: usize = 1_000;

/// The most tag pairs one resource keeps.
///
/// Source: `TagMap` is modelled `@length(min: 0, max: 200)` wherever it appears —
/// `Application$tags`, `JobRun$tags`, `TagResourceRequest$tags` and
/// `ListTagsForResourceResponse$tags` — so a resource carrying more pairs than that
/// could not be reported at all.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The HTTP status a successful TAG MUTATION answers with.
///
/// `TagResource` and `UntagResource` both leave their success code DEFAULTED in the
/// model — `@http` names a method and a URI and no `code:` — so the protocol's 200
/// default states nothing about this service, and the status a success carries is
/// behaviour the recorded AWS answers state. Both mutations write only the tag set of
/// the resource their `resourceArn` names and report nothing back:
/// `TagResourceResponse` and `UntagResourceResponse` declare no members
/// (`smithy_query(query="shape",shape="TagResourceResponse,UntagResourceResponse")`), so
/// there is no entity for a caller to parse and the answer is 204 No Content. A 200 there
/// promises a body the answer does not carry, and every client path gated on the
/// no-content status — conditional body parsing, caches and proxies that key on it — takes
/// the wrong branch. The two operations share the `/tags/{resourceArn}` route and this one
/// status, so the rule lives here rather than twice in the handlers; the status itself is
/// the cross-service one, and the FRAMING of an answer carrying it is `service_common`'s
/// too (see `crate::service`) — what belongs to this service is only which of its
/// operations answers it.
///
/// This does NOT extend to every output structure of this service that happens to declare
/// no members: `DeleteApplication`, `StartApplication` and `StopApplication` report nothing
/// either, and the recorded AWS answers for those carry the defaulted 200, which they keep.
pub const TAG_MUTATION_SUCCESS_STATUS: u16 =
    ::service_common::no_content_responses::NO_CONTENT_STATUS;

/// The same bound, for the interactive sessions one application holds. `StartSession`
/// models `ServiceQuotaExceededException` ("The maximum number of resources per account
/// has been reached"), which is what it answers past this bound.
pub const MAX_SESSIONS_PER_APPLICATION: usize = 1_000;

/// The job run timeout stored when `StartJobRun` names none.
///
/// `JobRun$executionTimeoutMinutes` documents it: "Returns the job run timeout value
/// from the `StartJobRun` call. If you didn't specify a timeout, this value defaults
/// to 720 minutes."
pub const DEFAULT_EXECUTION_TIMEOUT_MINUTES: i64 = 720;

/// Whether an application automatically starts on job submission when
/// `autoStartConfiguration` names nothing.
///
/// `AutoStartConfig$enabled` documents it: "Enables the application to automatically
/// start on job submission. Defaults to true."
pub const DEFAULT_AUTO_START_ENABLED: bool = true;

/// Whether an application automatically stops when idle, when
/// `autoStopConfiguration` names nothing.
///
/// `AutoStopConfig$enabled` documents it: "Enables the application to automatically
/// stop after a certain amount of time being idle. Defaults to true."
pub const DEFAULT_AUTO_STOP_ENABLED: bool = true;

/// The idle timeout stored when `autoStopConfiguration` names none.
///
/// `AutoStopConfig$idleTimeoutMinutes` documents it: "The amount of idle time in
/// minutes after which your application will automatically stop. Defaults to 15
/// minutes."
pub const DEFAULT_IDLE_TIMEOUT_MINUTES: i32 = 15;

/// Whether an application's logs are kept in managed log persistence, when its
/// `monitoringConfiguration` names no `managedPersistenceMonitoringConfiguration`.
///
/// `ManagedPersistenceMonitoringConfiguration$enabled` documents it: "Enables managed
/// logging and defaults to true. If set to false, managed logging will be turned off."
pub const DEFAULT_MANAGED_PERSISTENCE_ENABLED: bool = true;

/// Whether an application allocates cost per job run, when `CreateApplication` names no
/// `jobLevelCostAllocationConfiguration`.
///
/// `JobLevelCostAllocationConfiguration$enabled` "enables job level cost allocation for
/// the application" and the model declares no default, so it is the opt-in member it
/// reads as: an application created without it runs with it off, and a read reports that
/// rather than omitting the flag.
pub const DEFAULT_JOB_LEVEL_COST_ALLOCATION_ENABLED: bool = false;

/// Whether an application runs under Amazon EMR extended support.
///
/// No operation this service models carries an `extendedSupportConfiguration` member —
/// neither `CreateApplicationRequest` nor `UpdateApplicationRequest` declares one
/// (`smithy_query(query="operation",operation="CreateApplication,UpdateApplication")`) —
/// so no request reaching this emulator turns it on, and every application it holds runs
/// under the account-level setting, which is off. The sessions an application serves run
/// under the application's setting, so they report the same flag.
pub const EXTENDED_SUPPORT_ENABLED: bool = false;

/// Which level the extended-support setting an application runs under was set at: the
/// account, since nothing sets it per application (see [`EXTENDED_SUPPORT_ENABLED`]).
pub const EXTENDED_SUPPORT_SOURCE: &str = "ACCOUNT";

/// The domain EMR Serverless publishes an interactive session's endpoint under, between
/// the label naming the session and the region: a session is reached at
/// `https://<sessionId>.<this>.<region>.amazonaws.com`.
pub const SESSION_ENDPOINT_DOMAIN: &str = "s.emr-serverless-services";

/// How long a resource stays in the state the call that started the work returned
/// (`CREATING` after a create, `STARTING` after a start, `STOPPING` after a stop,
/// `CANCELLING` after a cancel) before the clock owes it the settled state.
///
/// EMR Serverless takes seconds to minutes; the emulator CHOOSES this span, and it is
/// deliberately not zero so a caller acting straight after the call still observes the
/// transient state. The due instant is stored on the record and materialised by
/// whichever later call reads it — nothing sleeps.
pub const LIFECYCLE_TRANSITION: Duration = Duration::from_millis(500);

/// How long a submitted job run spends in EACH of the non-terminal `JobRunState` values
/// it passes through ([`JOB_RUN_STAGES`]) before the clock owes it the next one.
///
/// A real job run spends real time being admitted, queued and scheduled; the emulator
/// CHOOSES this span, deliberately not zero, so a caller that submits work and then
/// polls or filters for in-flight runs observes every stage it passes through. The due
/// instants are stored on the record and materialised by whichever later call reads it —
/// nothing sleeps.
pub const JOB_RUN_STAGE_TRANSITION: Duration = Duration::from_millis(750);

/// How long this emulator holds a resource idle for, per MINUTE of idle time the
/// configuration governing it names.
///
/// The idle spans this service is configured with are stated in MINUTES, and the
/// shortest either of them admits — `AutoStopConfig$idleTimeoutMinutes`
/// (`@range(min: 1, max: 10080)`) and `StartSessionRequest$idleTimeoutMinutes` — is one
/// minute, longer than the window a client polls a lifecycle edge in. So the span an
/// idle timeout is SERVED as is the emulator's own, exactly like [`LIFECYCLE_TRANSITION`]
/// and [`JOB_RUN_STAGE_TRANSITION`]: each configured minute of idle time is served as
/// this span, deliberately not zero — a client acting straight after the call that left
/// the resource idle still observes it in the state it is idle in — and short enough that
/// the state change the idle timeout owes falls due inside the window a client polls in,
/// while the configured value still ORDERS those changes: a resource configured for more
/// idle minutes always stops after one configured for fewer. The configured value itself
/// is stored and reported verbatim; only the span the emulator waits for is its own.
pub const IDLE_MINUTE_SPAN: Duration = Duration::from_secs(5);

/// The span this emulator serves `minutes` of configured idle time as: one
/// [`IDLE_MINUTE_SPAN`] per minute, saturating rather than overflowing, and held to the
/// longest transient span this service ever waits out ([`MAX_TRANSIENT_SPAN`]).
pub fn idle_span(minutes: u64) -> Duration {
    IDLE_MINUTE_SPAN
        .saturating_mul(u32::try_from(minutes).unwrap_or(u32::MAX))
        .min(MAX_TRANSIENT_SPAN)
}

/// How long the URL `GetDashboardForJobRun` mints stays usable.
///
/// Source: the operation's own documentation — "The URL is valid for one hour after you
/// generate it. To access the application UI after that hour elapses, you must invoke
/// the API again to generate a new URL."
pub const DASHBOARD_URL_VALIDITY: Duration = Duration::from_secs(60 * 60);

/// How long the authentication token `GetSessionEndpoint` mints stays usable.
///
/// The operation documents that the token it returns is "time-limited" and that a client
/// must "Call this operation again when the authentication token expires to obtain a new
/// token", but names no span, so the emulator CHOOSES this one — deliberately not zero,
/// so a client that reads `authTokenExpiresAt` straight after the call sees a token it
/// can still use. It matches the one validity this service does document for a credential
/// it hands a client, [`DASHBOARD_URL_VALIDITY`].
pub const SESSION_AUTH_TOKEN_VALIDITY: Duration = Duration::from_secs(60 * 60);

/// `EndpointUrl`'s modelled `@length(min: 1, max: 256)`, which every URL
/// `GetSessionEndpoint` reports stays inside.
pub const ENDPOINT_URL_MAX_LENGTH: usize = 256;

/// The longest span this emulator ever holds a resource in a transient state for,
/// whatever a request asks.
///
/// `CancelJobRunRequest$shutdownGracePeriodInSeconds` carries no modelled range, so a
/// caller can name a wait of any length; a span longer than this is held to it. A
/// century is past the lifetime of any emulator process, so a caller asking for longer
/// observes the same thing either way — a wait that never ends — while the due instant
/// stays an instant the platform clock can represent.
pub const MAX_TRANSIENT_SPAN: Duration = Duration::from_secs(100 * 365 * 24 * 60 * 60);

/// The instant `span` after `instant`, saturating instead of overflowing.
///
/// Every due instant this service stores goes through here: `SystemTime`'s `+` PANICS
/// when the sum is not representable, and some of these spans come from a request
/// member the model puts no bound on, so a single request must not be able to reach
/// that. The span is first held to [`MAX_TRANSIENT_SPAN`], and a sum the platform
/// still cannot represent falls back to the furthest instant this function can reach.
pub fn due_at(instant: SystemTime, span: Duration) -> SystemTime {
    let span = span.min(MAX_TRANSIENT_SPAN);
    instant
        .checked_add(span)
        .or_else(|| instant.checked_add(LIFECYCLE_TRANSITION))
        .unwrap_or(instant)
}

/// The CPU architecture an application runs on when `CreateApplication` names none.
///
/// `Architecture` is `ARM64 | X86_64` and `CreateApplicationRequest$architecture` is
/// optional; EMR Serverless applies the x86_64 architecture to an application that
/// names none ("The default architecture is x86_64" in the EMR Serverless user guide's
/// architecture section), and reports it on every later read.
pub const DEFAULT_ARCHITECTURE: &str = "X86_64";

/// `Application$stateDetails` for an application a caller created.
///
/// The member is "the state details of the application", and the detail an EMR
/// Serverless application carries from its create says how it came to be; the service
/// keeps it across the application's later states rather than rewriting it per state,
/// so a transition with no detail of its own leaves this one in place.
pub const CREATED_STATE_DETAILS: &str = "Application created by user.";

/// One engine EMR Serverless serves: the casing its API reports the engine in, and the
/// earliest Amazon EMR release that offers it.
#[derive(Debug, Clone, Copy)]
pub struct SupportedEngine {
    /// The canonical `EngineType` value, which is what every read reports whatever
    /// casing the create asked in.
    pub canonical_type: &'static str,
    /// The `(major, minor, patch)` of the earliest Amazon EMR release supporting it.
    pub min_release: (u32, u32, u32),
}

/// Every engine type an application can be created for, with the release each needs.
///
/// `EngineType` is an OPEN string in the model (len 1..64, no enum), so which engines
/// exist — and from which Amazon EMR release — is a service-side rule rather than a
/// model constraint. Source: `Application$type`'s documentation, "The type of
/// application, such as Spark or Hive", for the engines and the casing the API reports
/// them in, and "EMR Serverless is available with Amazon EMR releases 6.6.0 and later"
/// for the release each one needs.
pub const SUPPORTED_ENGINES: &[SupportedEngine] = &[
    SupportedEngine {
        canonical_type: "Spark",
        min_release: (6, 6, 0),
    },
    SupportedEngine {
        canonical_type: "Hive",
        min_release: (6, 6, 0),
    },
];

/// `JobRun$stateDetails` for a job run that has been accepted but not yet run.
pub const SUBMITTED_STATE_DETAILS: &str = "Job run submitted.";

/// `JobRun$stateDetails` for the state the clock owes every submitted job run.
///
/// This emulator is a control plane: it records a job run faithfully, but it runs no
/// Spark or Hive workload, so a job run can never reach `SUCCESS` — it ends in the
/// modelled failure state with a reason that says exactly that.
pub const NOT_EXECUTED_STATE_DETAILS: &str =
    "This emulator records job runs but executes no Spark or Hive workload, so the job \
     run ended without running.";

/// `JobRun$stateDetails` for a job run a caller cancelled.
pub const CANCELLED_STATE_DETAILS: &str = "Job run cancelled at the request of the caller.";

/// `JobRun$stateDetails` for each stage of [`JOB_RUN_STAGES`].
pub const PENDING_STATE_DETAILS: &str = "Job run accepted and pending admission.";
pub const QUEUED_STATE_DETAILS: &str = "Job run queued, waiting for capacity.";
pub const SCHEDULED_STATE_DETAILS: &str = "Job run scheduled on the application.";

/// The states a submitted job run passes through, in order, with the `stateDetails`
/// each one carries.
///
/// A job run is NOT terminal the instant it is submitted: it is admitted, queued and
/// scheduled before anything runs, and each of those is a value of the `JobRunState`
/// enum (`SUBMITTED`, `PENDING`, `SCHEDULED`, `QUEUED`, `RUNNING`, `SUCCESS`, `FAILED`,
/// `CANCELLING`, `CANCELLED`), so a client that submits work and then polls or filters
/// for in-flight runs observes it in them.
///
/// `RUNNING` is NOT among them, and neither is `SUCCESS`: this emulator is a control
/// plane that executes no Spark or Hive workload, so it never reports a run as running
/// or succeeded — once scheduled, the clock owes it the modelled failure state with
/// [`NOT_EXECUTED_STATE_DETAILS`], which says exactly that.
pub const JOB_RUN_STAGES: &[(&str, &str)] = &[
    ("PENDING", PENDING_STATE_DETAILS),
    ("QUEUED", QUEUED_STATE_DETAILS),
    ("SCHEDULED", SCHEDULED_STATE_DETAILS),
    ("FAILED", NOT_EXECUTED_STATE_DETAILS),
];

/// The state changes the clock owes a job run submitted at `now`: one per entry of
/// [`JOB_RUN_STAGES`], each [`JOB_RUN_STAGE_TRANSITION`] after the one before it.
pub fn job_run_stages(now: SystemTime) -> VecDeque<PendingState> {
    JOB_RUN_STAGES
        .iter()
        .enumerate()
        .map(|(index, (state, details))| PendingState {
            due: due_at(
                now,
                JOB_RUN_STAGE_TRANSITION * (u32::try_from(index).unwrap_or(0) + 1),
            ),
            state: (*state).to_string(),
            state_details: Some((*details).to_string()),
        })
        .collect()
}

/// `Application$stateDetails` for an application the service stopped itself, because it
/// sat idle past the idle timeout its `autoStopConfiguration` names.
pub const AUTO_STOPPED_STATE_DETAILS: &str =
    "Application stopped automatically after being idle past its configured idle timeout.";

/// The `JobRunState` values from which a job run never moves again.
///
/// Source: the `JobRunState` enum (`SUBMITTED`, `PENDING`, `SCHEDULED`, `RUNNING`,
/// `SUCCESS`, `FAILED`, `CANCELLING`, `CANCELLED`, `QUEUED`) — the three that name a
/// finished run.
pub const TERMINAL_JOB_RUN_STATES: &[&str] = &["SUCCESS", "FAILED", "CANCELLED"];

/// `Session$stateDetails` for a session a caller has just asked for, before the service
/// has brought it up.
pub const SESSION_SUBMITTED_STATE_DETAILS: &str = "Session submitted.";

/// `Session$stateDetails` for a session that has come up and is ready for statements.
pub const SESSION_STARTED_STATE_DETAILS: &str = "Session started.";

/// `Session$stateDetails` for a session the service terminated for having stayed idle
/// for its `idleTimeoutMinutes`.
pub const SESSION_IDLE_TIMEOUT_STATE_DETAILS: &str =
    "Session terminated after remaining idle for its idle timeout.";

/// `Session$stateDetails` for a session a caller asked to terminate, while the
/// termination is still in flight.
///
/// `TerminateSession` documents the pair of states it moves a session through: "After
/// you terminate a session, it enters the TERMINATING state and then the TERMINATED
/// state."
pub const SESSION_TERMINATING_STATE_DETAILS: &str = "Session termination requested.";

/// `Session$stateDetails` for a session a caller's `TerminateSession` ended.
pub const SESSION_TERMINATED_STATE_DETAILS: &str = "Session terminated by request.";

/// The `SessionState` values from which a session never moves again.
///
/// Source: the `SessionState` enum (`SUBMITTED`, `STARTING`, `STARTED`, `IDLE`, `BUSY`,
/// `FAILED`, `TERMINATING`, `TERMINATED`) — the two that name a session that is over.
pub const TERMINAL_SESSION_STATES: &[&str] = &["FAILED", "TERMINATED"];

/// True when `state` names a `SessionState` a session never leaves.
pub fn is_terminal_session_state(state: &str) -> bool {
    TERMINAL_SESSION_STATES.contains(&state)
}

/// The `SessionState` values a `TerminateSession` ACTS on: the ones naming a session
/// that is still live, so a terminate has a session left to end.
///
/// Derived from the generated `SessionState` value list rather than re-listed, so a value
/// the enum gains is classified in one place: a state is terminable when the enum holds
/// it, it is not terminal ([`TERMINAL_SESSION_STATES`]) and it is not
/// [`SESSION_TERMINATING_STATE`], which already names the termination a terminate asks
/// for. A session in any OTHER state has no live work left for a terminate to end, which
/// the `@idempotent` operation REPORTS rather than refuses.
pub fn is_terminable_session_state(state: &str) -> bool {
    crate::ops::types::SESSION_STATE_VALUES.contains(&state)
        && !is_terminal_session_state(state)
        && state != SESSION_TERMINATING_STATE
}

/// The `SessionState` a terminate moves a live session to.
///
/// Source: `TerminateSession`'s own documentation — "After you terminate a session, it
/// enters the TERMINATING state and then the TERMINATED state."
pub const SESSION_TERMINATING_STATE: &str = "TERMINATING";

/// The `SessionState` the clock then owes that session, from the same sentence.
pub const SESSION_TERMINATED_STATE: &str = "TERMINATED";

/// The earliest Amazon EMR release whose applications serve interactive sessions.
///
/// Source: `StartSession`'s own documentation — "This operation is supported for EMR
/// release 7.13.0 and later."
pub const MIN_SESSION_RELEASE: (u32, u32, u32) = (7, 13, 0);

/// Whether an application on `release_label` serves interactive sessions.
///
/// A label this emulator can read no version out of is judged to serve them rather than
/// refused for a version nothing can order — the same leniency
/// [`supported_engine_type`] applies.
pub fn release_supports_sessions(release_label: &str) -> bool {
    release_version(release_label).is_none_or(|version| version >= MIN_SESSION_RELEASE)
}

/// The `ApplicationState` values `DeleteApplication` admits, judged against the state
/// the application has SETTLED on ([`ApplicationRecord::settled_state`]).
///
/// Source: the operation's own documentation — "An application has to be in a stopped
/// or created state in order to be deleted."
pub const DELETABLE_APPLICATION_STATES: &[&str] = &["CREATED", "STOPPED"];

/// The `ApplicationState` values `StartApplication` admits: an application holds
/// capacity only from a status that has no capacity yet.
///
/// The same pair the delete admits, named separately because the two gates state
/// different sentences about it.
pub const STARTABLE_APPLICATION_STATES: &[&str] = &["CREATED", "STOPPED"];

/// How a status gate renders the statuses it admits: the bracketed, comma-separated
/// list the service's own refusals carry, built from the gate's own set so the sentence
/// and the check can never disagree.
fn allowed_status_list(states: &[&str]) -> String {
    format!("[{}]", states.join(", "))
}

/// What a `ValidationException` says when `StartApplication` is asked for on an
/// application whose status does not admit a start.
///
/// Recorded behaviour: the sentence states the operation that was refused and the
/// statuses that would admit it, and names neither the application nor the status it is
/// in — so it is fixed text plus the allowed-status list.
pub fn application_not_startable_message() -> String {
    format!(
        "Application cannot be STARTED. Applications must be in one of the following statuses: {}",
        allowed_status_list(STARTABLE_APPLICATION_STATES)
    )
}

/// What a `ValidationException` says when `DeleteApplication` is asked for on an
/// application whose status does not admit a delete.
///
/// Recorded behaviour: this sentence DOES name the application — the id the request
/// carried — and the status it is currently in, beside the allowed-status list. Its
/// fixed text reads "must be in of the following statuses", which is how the service
/// words it; it is reproduced as the service says it rather than repaired.
pub fn application_not_deletable_message(application_id: &str, current_status: &str) -> String {
    format!(
        "Application {application_id} must be in of the following statuses {}. Current status: \
         {current_status}",
        allowed_status_list(DELETABLE_APPLICATION_STATES)
    )
}

/// The `ApplicationState` a deleted application is retained in: the terminal value of
/// the enum.
pub const TERMINATED_APPLICATION_STATE: &str = "TERMINATED";

/// True when `state` names an application that is still part of the account's
/// inventory, rather than one a delete has terminated.
///
/// `DeleteApplication` is a SOFT delete here: the record is retained so a read reports
/// the terminal state. The inventory a list reports with no `states` filter is therefore
/// the applications that have not been deleted — naming `TERMINATED` explicitly is how a
/// caller asks for the deleted ones.
pub fn is_live_application_state(state: &str) -> bool {
    state != TERMINATED_APPLICATION_STATE
}

/// The `ApplicationState` values `UpdateApplication` admits, judged against the state
/// the application has SETTLED on ([`ApplicationRecord::settled_state`]).
///
/// Source: that operation's own documentation — "An application has to be in a stopped
/// or created state in order to be updated."
///
/// That precondition is the one a WHOLE-application update is judged against. An
/// application that is RUNNING admits the documented subset of members in
/// [`STARTED_UPDATABLE_MEMBER_PATHS`] as well, so the precondition is judged per NAMED
/// member rather than per request.
pub const UPDATABLE_APPLICATION_STATES: &[&str] = &["CREATED", "STOPPED"];

/// The `ApplicationState` an application holding capacity is in — the state the
/// running-application carve-out below applies to.
pub const STARTED_APPLICATION_STATE: &str = "STARTED";

/// The configuration member PATHS `UpdateApplication` accepts while the application it
/// names is `STARTED`.
///
/// The operation's model documentation states only the general precondition above, and
/// EMR Serverless carves a fixed subset out of it: an update that names any OTHER
/// configuration member of a running application is refused with a
/// `ValidationException` that ENUMERATES this subset, which is where both this catalog
/// and [`started_update_refusal_message`] come from —
///
/// > Only the following configurations can be updated when an application is in STARTED
/// > state: schedulerConfiguration.maxConcurrentRuns, extendedSupportConfiguration,
/// > imageConfiguration, workerTypeSpecifications.imageConfiguration, maximumCapacity.
///
/// A path naming a SUB-member (`schedulerConfiguration.maxConcurrentRuns`) is updatable
/// only in that member: the rest of the block it belongs to keeps what the application
/// has stored. `extendedSupportConfiguration` is part of the set the service
/// enumerates, and this service's model declares no such member on
/// `UpdateApplicationRequest`
/// (`smithy_query(query="operation",operation="UpdateApplication")`), so no request
/// reaching this emulator can name it — it is kept here because the refusal states the
/// service's set, not this emulator's reading of it.
pub const STARTED_UPDATABLE_MEMBER_PATHS: &[&str] = &[
    "schedulerConfiguration.maxConcurrentRuns",
    "extendedSupportConfiguration",
    "imageConfiguration",
    "workerTypeSpecifications.imageConfiguration",
    "maximumCapacity",
];

/// The `ValidationException` message an update naming a member a running application
/// does not admit is refused with: the enumeration of
/// [`STARTED_UPDATABLE_MEMBER_PATHS`], which is the rule itself rather than a sentence
/// about one request — so it names no member, id or value the request carried.
pub fn started_update_refusal_message() -> String {
    format!(
        "Only the following configurations can be updated when an application is in \
         {STARTED_APPLICATION_STATE} state: {}.",
        STARTED_UPDATABLE_MEMBER_PATHS.join(", ")
    )
}

/// The `ValidationException` message the auto-start gate refuses a submit with: the
/// service's own fixed sentence, which states the gate — the application is not running
/// and will not start itself — and names no application id, state or remedy call.
///
/// `work_item` is what could not be started ("Job run", "Session"): the sentence is
/// otherwise the same for every submit the gate stops.
pub fn auto_start_disabled_message(work_item: &str) -> String {
    format!(
        "{work_item} cannot be started as Application is not in \
         {STARTED_APPLICATION_STATE} state and auto-start is disabled"
    )
}

/// The prefix a pagination cursor carries, so a token this service did not mint is
/// refused rather than read as a resume point.
const PAGE_TOKEN_PREFIX: &str = "v1_";

/// The `(major, minor, patch)` release an `emr-<major>.<minor>.<patch>` label names, or
/// `None` when the label carries no such version.
///
/// `ReleaseLabel` is a free string (pattern `^[A-Za-z0-9._/-]+$`), so a label this
/// emulator cannot read a release out of is one it cannot order against any minimum.
pub fn release_version(release_label: &str) -> Option<(u32, u32, u32)> {
    let rest = release_label.strip_prefix("emr-")?;
    // A label may carry a suffix after the version (`emr-6.6.0-20220101`); the version
    // is what precedes it.
    let version = rest.split('-').next()?;
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}

/// Which MEMBER of a create this service could not serve: the two members
/// [`resolve_engine_type`] judges, each with its own refusal wording.
///
/// A refusal has to point the caller at the member it must fix, so an unrecognised
/// engine type and a release label that names no release this service offers are
/// distinct answers rather than one shared message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedEngine {
    /// `type` names no engine this service serves at any release.
    Type,
    /// `type` is an engine this service serves, but `releaseLabel` names no release
    /// that offers it — including a label that names no Amazon EMR release at all.
    ReleaseLabel,
}

/// The release a label must reach for EMR Serverless to serve anything at all: the
/// earliest release any engine in [`SUPPORTED_ENGINES`] is offered from.
///
/// Source: "EMR Serverless is available with Amazon EMR releases 6.6.0 and later", which
/// is what each engine's `min_release` records.
fn earliest_served_release() -> (u32, u32, u32) {
    SUPPORTED_ENGINES
        .iter()
        .map(|engine| engine.min_release)
        .min()
        .unwrap_or((0, 0, 0))
}

/// True when `release_label` names an Amazon EMR release this service serves: a label
/// this emulator can read an `emr-<major>.<minor>.<patch>` version out of, at or after
/// the earliest release EMR Serverless is offered from.
///
/// A label carrying no readable version (and one naming a release older than the service
/// itself) names no release that exists here, which is the `releaseLabel` refusal.
pub fn release_label_is_served(release_label: &str) -> bool {
    release_version(release_label).is_some_and(|version| version >= earliest_served_release())
}

/// The canonical engine type to store for a create naming `engine_type` at
/// `release_label`, or which MEMBER made the pair unservable.
///
/// The engine is matched WITHOUT case, because the service reports it in its own casing
/// rather than the caller's. The type is judged first: a value that names no engine at
/// all is that member's own refusal whatever release it was asked for. Otherwise the
/// release label has to name a release that offers that engine.
pub fn resolve_engine_type(
    engine_type: &str,
    release_label: &str,
) -> Result<&'static str, UnsupportedEngine> {
    let engine = SUPPORTED_ENGINES
        .iter()
        .find(|engine| engine.canonical_type.eq_ignore_ascii_case(engine_type))
        .ok_or(UnsupportedEngine::Type)?;
    if !release_label_is_served(release_label) {
        return Err(UnsupportedEngine::ReleaseLabel);
    }
    match release_version(release_label) {
        Some(version) if version < engine.min_release => Err(UnsupportedEngine::ReleaseLabel),
        _ => Ok(engine.canonical_type),
    }
}

/// The `ValidationException` message for a `type` that names no engine this service
/// serves: built from the request, naming the `type` member value it refused.
///
/// The wording is the one this service uses for the refusal, which names the rejected
/// type and nothing else of the request — as its other refusals do (its
/// `ResourceNotFoundException` for an application reads "Application does not exist",
/// carrying no id).
pub fn unsupported_engine_type_message(engine_type: &str) -> String {
    format!("Type {engine_type} is not supported for release label")
}

/// The `ValidationException` message for a `releaseLabel` that names no release offering
/// the engine asked for.
///
/// Unlike [`unsupported_engine_type_message`] it interpolates nothing: the engine type
/// the request named is valid, so quoting it would point the caller at the member that
/// is not the problem. The phrase states both ways the release label can fail — the
/// release exists but does not offer that engine, or it does not exist at all.
pub const UNSUPPORTED_RELEASE_LABEL_MESSAGE: &str =
    "Type is not supported for the release label or release label does not exist";

/// True when `state` names a `JobRunState` a job run never leaves.
pub fn is_terminal_job_run_state(state: &str) -> bool {
    TERMINAL_JOB_RUN_STATES.contains(&state)
}

/// A state change the clock owes a resource: the instant it falls due, the state the
/// resource carries from then on, and the detail text that goes with it.
#[derive(Debug, Clone)]
pub struct PendingState {
    pub due: SystemTime,
    pub state: String,
    pub state_details: Option<String>,
}

/// One job run, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct JobRunRecord {
    /// The modelled `JobRun` shape `GetJobRun` returns verbatim.
    pub job_run: JobRun,
    /// `JobRunSummary$type` — the engine type of the application the run was
    /// submitted to. `JobRun` carries no such member, so it is held beside it.
    pub job_run_type: String,
    /// The state changes the clock owes this job run, in the order they fall due: a
    /// submitted run passes through several states ([`JOB_RUN_STAGES`]) before it
    /// settles, and a cancel replaces whatever is left of them.
    pub pending: VecDeque<PendingState>,
    /// The dashboard URL grant the last `GetDashboardForJobRun` minted for this run,
    /// if any, and how many it has minted. The counter is what makes each URL a fresh
    /// one rather than a repeat of the last.
    pub dashboard: Option<DashboardGrant>,
    pub dashboard_grants: u64,
    /// Where this run's log artifacts are published, as the configuration the SUBMIT
    /// carried resolved it ([`job_run_logs::log_destination`]), or `None` when neither
    /// the application nor the run named an Amazon S3 destination.
    pub log_destination: Option<job_run_logs::LogDestination>,
    /// The same, for the CloudWatch Logs destination the submit's configuration named
    /// ([`job_run_logs::cloud_watch::cloud_watch_destination`]), or `None` where logging
    /// there was not enabled for the run.
    pub cloud_watch_destination: Option<job_run_logs::cloud_watch::CloudWatchDestination>,
    /// The lifecycle this run's driver log reports: the submit, and every state the clock
    /// or a cancel moved it to since. Bounded by
    /// [`job_run_logs::MAX_JOB_RUN_LOG_EVENTS`].
    pub log_events: Vec<job_run_logs::JobRunLogEvent>,
    /// Whether the clock owes this run's artifacts a publication: set when the run
    /// reaches a state it never leaves, and cleared by the call that writes them, so one
    /// finished run publishes once however many calls arrive afterwards.
    pub log_publication_due: bool,
}

/// A dashboard URL this service minted for one job run: the token the URL carries and
/// the instant it stops being usable.
///
/// `GetDashboardForJobRun` is not a report — it "Creates and returns a URL" — so the
/// grant it mints is recorded here rather than recomputed per read.
#[derive(Debug, Clone)]
pub struct DashboardGrant {
    pub token: String,
    pub expires_at: SystemTime,
}

impl JobRunRecord {
    /// Materialise every state change the clock owes this job run, in order, for each
    /// one whose due instant `now` is past; answers the instant the run FINISHED when
    /// one of them settled it in a state it never leaves.
    ///
    /// `updatedAt` moves to the DUE instant rather than to `now`: the change happened
    /// when it fell due, whether or not anybody was reading. `endedAt` is set only
    /// when the settled state is one a job run never leaves, and the stages left over
    /// behind it are dropped — nothing moves a finished run again.
    pub fn settle(&mut self, now: SystemTime) -> Option<SystemTime> {
        let mut finished_at = None;
        while let Some(next) = self.pending.front() {
            if now < next.due {
                break;
            }
            let Some(pending) = self.pending.pop_front() else {
                break;
            };
            let at = DateTime::from(pending.due);
            self.job_run.state = pending.state.clone();
            if let Some(details) = pending.state_details.as_ref() {
                self.job_run.state_details = details.clone();
            }
            self.job_run.updated_at = at;
            self.job_run.attempt_updated_at = Some(at);
            self.record_lifecycle(pending.due);
            if is_terminal_job_run_state(&self.job_run.state) {
                self.job_run.ended_at = Some(at);
                self.pending.clear();
                finished_at = Some(pending.due);
                // The artifacts a finished run leaves in its configured destinations are
                // owed from THIS instant, whether or not anybody was reading: see
                // [`job_run_logs`] for what is written and which call writes it.
                self.log_publication_due =
                    self.log_destination.is_some() || self.cloud_watch_destination.is_some();
            }
        }
        finished_at
    }

    /// Record the state this run carries now as a line of its driver log, stamped at the
    /// instant the change happened rather than at whoever observed it.
    ///
    /// The lines are what the published log reports ([`job_run_logs::driver_log_text`]).
    /// Past [`job_run_logs::MAX_JOB_RUN_LOG_EVENTS`] a line is dropped rather than
    /// queued, so a run nobody reads cannot grow the record without limit.
    pub fn record_lifecycle(&mut self, at: SystemTime) {
        if self.log_events.len() >= job_run_logs::MAX_JOB_RUN_LOG_EVENTS {
            return;
        }
        self.log_events.push(job_run_logs::JobRunLogEvent {
            at,
            state: self.job_run.state.clone(),
            state_details: Some(self.job_run.state_details.clone()),
        });
    }

    /// The `JobRunSummary` projection `ListJobRuns` returns: the stored record's own
    /// members, never request input.
    pub fn summary(&self) -> JobRunSummary {
        let run = &self.job_run;
        JobRunSummary {
            application_id: run.application_id.clone(),
            id: run.job_run_id.clone(),
            name: run.name.clone(),
            mode: run.mode.clone(),
            arn: run.arn.clone(),
            created_by: run.created_by.clone(),
            created_at: run.created_at,
            updated_at: run.updated_at,
            execution_role: run.execution_role.clone(),
            state: run.state.clone(),
            state_details: run.state_details.clone(),
            release_label: run.release_label.clone(),
            r#type: Some(self.job_run_type.clone()),
            attempt: run.attempt,
            attempt_created_at: run.attempt_created_at,
            attempt_updated_at: run.attempt_updated_at,
        }
    }
}

/// One interactive session, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    /// The modelled `Session` shape `GetSession` returns verbatim.
    pub session: Session,
    /// The state change the clock owes this session, if any.
    pub pending: Option<PendingState>,
}

impl SessionRecord {
    /// Materialise every state change the clock owes this session, if `now` is past
    /// their due instants; answers the instant the session ENDED when one of them
    /// settled it in a state it never leaves.
    ///
    /// `updatedAt` moves to the DUE instant rather than to `now`: the change happened
    /// when it fell due, whether or not anybody was reading. A session that comes up
    /// owes a further change when its idle timeout falls due, so this settles in a loop
    /// rather than once: a reader arriving after both instants observes the later state.
    pub fn settle(&mut self, now: SystemTime) -> Option<SystemTime> {
        while let Some(pending) = self.pending.clone() {
            if now < pending.due {
                return None;
            }
            let at = DateTime::from(pending.due);
            self.session.state = pending.state;
            if let Some(details) = pending.state_details {
                self.session.state_details = details;
            }
            self.session.updated_at = at;
            self.pending = None;
            if self.session.state == "STARTED" {
                // The session came up at the due instant, and this emulator runs no
                // statement in it, so it has been idle since that same instant.
                self.session.started_at = Some(at);
                self.session.idle_since = Some(at);
                self.pending = self.idle_timeout_pending(pending.due);
            }
            if is_terminal_session_state(&self.session.state) {
                self.session.ended_at = Some(at);
                return Some(pending.due);
            }
        }
        None
    }

    /// The `SessionSummary` projection `ListSessions` returns: the stored record's own
    /// members, never request input.
    pub fn summary(&self) -> SessionSummary {
        let session = &self.session;
        SessionSummary {
            application_id: session.application_id.clone(),
            session_id: session.session_id.clone(),
            arn: session.arn.clone(),
            name: session.name.clone(),
            state: session.state.clone(),
            state_details: session.state_details.clone(),
            release_label: session.release_label.clone(),
            execution_role_arn: session.execution_role_arn.clone(),
            created_by: session.created_by.clone(),
            created_at: session.created_at,
            updated_at: session.updated_at,
        }
    }

    /// The termination the session's idle timeout owes it, measured from `idle_since`.
    ///
    /// `StartSessionRequest$idleTimeoutMinutes` documents it: "The idle timeout in
    /// minutes for the session. After the session remains idle for this duration, Amazon
    /// EMR Serverless automatically terminates it." A session that named no timeout, or
    /// named zero minutes, is owed no termination: nothing documents a default, so none
    /// is invented. Each minute it names is served as one [`IDLE_MINUTE_SPAN`], the span
    /// this emulator holds a resource idle for.
    fn idle_timeout_pending(&self, idle_since: SystemTime) -> Option<PendingState> {
        let minutes = u64::try_from(self.session.idle_timeout_minutes.unwrap_or(0)).ok()?;
        if minutes == 0 {
            return None;
        }
        Some(PendingState {
            due: due_at(idle_since, idle_span(minutes)),
            state: "TERMINATED".to_string(),
            state_details: Some(SESSION_IDLE_TIMEOUT_STATE_DETAILS.to_string()),
        })
    }
}

/// One EMR Serverless application, as this emulator persists it, together with the job
/// runs submitted to it.
#[derive(Debug, Clone)]
pub struct ApplicationRecord {
    /// The modelled `Application` shape `GetApplication` returns verbatim.
    pub application: Application,
    /// The state change the clock owes this application, if any.
    pub pending: Option<PendingState>,
    /// This application's job runs, keyed by id. A `BTreeMap` is what gives
    /// `ListJobRuns` the stable total order its cursor indexes into — ids sort in the
    /// order they were minted.
    pub job_runs: BTreeMap<String, JobRunRecord>,
    /// A `StartJobRun` `clientToken` to the job run that token started, so a retried
    /// submit answers with the first job run instead of a second one.
    pub job_run_tokens: HashMap<String, String>,
    /// The counter job run ids are minted from.
    pub next_job_run_seq: u64,
    /// This application's interactive sessions, keyed by id, in the stable total order
    /// ids were minted in — the order a session listing walks them in.
    pub sessions: BTreeMap<String, SessionRecord>,
    /// A `StartSession` `clientToken` to the session that token started, so a retried
    /// start answers with the first session instead of a second one.
    pub session_tokens: HashMap<String, String>,
    /// When this application was last ACTIVE: the instant it was created, the instant a
    /// start brought it up, and the instant its last job run or session was started or
    /// finished.
    ///
    /// `AutoStopConfig` is stated in terms of "a certain amount of time being idle", so
    /// the idle span the configuration bounds is measured from here.
    pub last_activity_at: SystemTime,
}

impl ApplicationRecord {
    /// Materialise every state change the clock owes this application and its job
    /// runs, if `now` is past their due instants — including the automatic stop its
    /// `autoStopConfiguration` owes an application that has been idle too long.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(pending) = self.pending.as_ref() {
            if now >= pending.due {
                self.application.state = pending.state.clone();
                if let Some(details) = pending.state_details.as_ref() {
                    self.application.state_details = Some(details.clone());
                }
                self.application.updated_at = DateTime::from(pending.due);
                if self.application.state == "STARTED" {
                    // The application came up at the due instant, so that is the instant
                    // from which it has been idle: the span `autoStopConfiguration`
                    // bounds runs from when the application was there to use, not from
                    // the call that asked for it.
                    self.last_activity_at = self.last_activity_at.max(pending.due);
                }
                self.pending = None;
            }
        }
        for record in self.job_runs.values_mut() {
            if let Some(finished_at) = record.settle(now) {
                // A job run finishing is the application's last activity: the idle
                // span the auto-stop bounds starts when the work stopped, not when
                // somebody happened to read it.
                self.last_activity_at = self.last_activity_at.max(finished_at);
            }
        }
        for record in self.sessions.values_mut() {
            // A session ending is activity on the application, the same way a job run
            // finishing is: the idle span the auto-stop bounds starts when the session
            // was over, not when somebody happened to read it.
            if let Some(ended_at) = record.settle(now) {
                self.last_activity_at = self.last_activity_at.max(ended_at);
            }
        }
        // `AutoStopConfig$enabled`: "Enables the application to automatically stop
        // after a certain amount of time being idle. Defaults to true." The stop falls
        // due at the end of that idle span, so it is materialised at the DUE instant
        // rather than at `now`.
        if let Some(due) = self.auto_stop_due() {
            if now >= due {
                self.application.state = "STOPPED".to_string();
                self.application.state_details = Some(AUTO_STOPPED_STATE_DETAILS.to_string());
                self.application.updated_at = DateTime::from(due);
            }
        }
    }

    /// Whether this application stops by itself once it has been idle long enough.
    ///
    /// `AutoStopConfig$enabled` documents the default as true, and `CreateApplication`
    /// materialises it, so an absent configuration here is read the same way.
    pub fn auto_stop_enabled(&self) -> bool {
        self.application
            .auto_stop_configuration
            .as_ref()
            .and_then(|config| config.enabled)
            .unwrap_or(DEFAULT_AUTO_STOP_ENABLED)
    }

    /// How long this application may sit idle before it stops itself.
    ///
    /// `AutoStopConfig$idleTimeoutMinutes` is "the amount of idle time in minutes after
    /// which your application will automatically stop" (`@range(min: 1, max: 10080)`,
    /// "Defaults to 15 minutes"), so the caller's value is the number of idle minutes
    /// that applies, each served as one [`IDLE_MINUTE_SPAN`].
    pub fn idle_timeout(&self) -> Duration {
        let minutes = self
            .application
            .auto_stop_configuration
            .as_ref()
            .and_then(|config| config.idle_timeout_minutes)
            .unwrap_or(DEFAULT_IDLE_TIMEOUT_MINUTES);
        let minutes = u64::try_from(minutes).unwrap_or(1).max(1);
        idle_span(minutes)
    }

    /// The instant the clock owes this application a `STOPPED` state with no
    /// `StopApplication` call, or `None` when nothing is owed.
    ///
    /// Only a RUNNING application can be stopped for being idle, and only one that is
    /// actually idle: an application holding a job run or an interactive session that
    /// has not finished is working, however long ago the last call reached it.
    pub fn auto_stop_due(&self) -> Option<SystemTime> {
        if self.application.state != "STARTED" || self.pending.is_some() {
            return None;
        }
        if !self.auto_stop_enabled() {
            return None;
        }
        if !self.unfinished_job_run_ids().is_empty() || !self.unfinished_session_ids().is_empty() {
            return None;
        }
        Some(due_at(self.last_activity_at, self.idle_timeout()))
    }

    /// Record that something ran on this application at `now`, which is what restarts
    /// the idle span its `autoStopConfiguration` bounds.
    pub fn mark_activity(&mut self, now: SystemTime) {
        self.last_activity_at = self.last_activity_at.max(now);
    }

    /// The lifecycle state this application has SETTLED on: the state the clock
    /// already owes it while a transition is still pending, and the state it carries
    /// now otherwise.
    ///
    /// A precondition on the lifecycle is judged against this, so a transition the
    /// emulator has not materialised yet never refuses a call the settled application
    /// admits.
    pub fn settled_state(&self) -> &str {
        self.pending
            .as_ref()
            .map_or(self.application.state.as_str(), |pending| {
                pending.state.as_str()
            })
    }

    /// Terminate this application, which is what `DeleteApplication` does to it.
    ///
    /// The record is RETAINED in `TERMINATED` — the terminal value of the
    /// `ApplicationState` enum — rather than dropped, so a client polling the
    /// application after a delete observes a terminal state instead of the state it
    /// last had. `stateDetails` is left as stored: it says how the application came to
    /// be, which the delete does not change.
    pub fn terminate(&mut self, now: SystemTime) {
        self.pending = None;
        self.application.state = "TERMINATED".to_string();
        self.application.updated_at = now_timestamp(now);
    }

    /// Mint an unused job run id matching `JobRunId`'s `^[0-9a-z]+$`.
    ///
    /// Sixteen lowercase hex digits, like the ids EMR Serverless issues; the loop
    /// skips an id already taken, which is what makes the counter's wrap safe.
    pub fn mint_job_run_id(&mut self) -> String {
        loop {
            self.next_job_run_seq = self.next_job_run_seq.wrapping_add(1);
            let id = format!("{:016x}", self.next_job_run_seq);
            if !self.job_runs.contains_key(&id) {
                return id;
            }
        }
    }

    /// Whether interactive sessions are enabled on this application.
    ///
    /// `InteractiveConfiguration$sessionEnabled` documents it: "Enables interactive
    /// sessions on the application. When set to `true`, you can start interactive
    /// sessions using the `StartSession` operation." Nothing documents a default, and
    /// the member is optional, so an application that enabled nothing serves no
    /// sessions.
    pub fn session_enabled(&self) -> bool {
        self.application
            .interactive_configuration
            .as_ref()
            .and_then(|config| config.session_enabled)
            .unwrap_or(false)
    }

    /// Whether this application starts by itself when a job run is submitted to it.
    ///
    /// `AutoStartConfig$enabled` documents the default as true, and
    /// `CreateApplication` materialises it, so an absent configuration here is read
    /// the same way.
    pub fn auto_start_enabled(&self) -> bool {
        self.application
            .auto_start_configuration
            .as_ref()
            .and_then(|config| config.enabled)
            .unwrap_or(DEFAULT_AUTO_START_ENABLED)
    }

    /// The job runs of this application that have not finished.
    pub fn unfinished_job_run_ids(&self) -> Vec<String> {
        self.job_runs
            .values()
            .filter(|record| !is_terminal_job_run_state(&record.job_run.state))
            .map(|record| record.job_run.job_run_id.clone())
            .collect()
    }

    /// The interactive sessions of this application that have not finished.
    pub fn unfinished_session_ids(&self) -> Vec<String> {
        self.sessions
            .values()
            .filter(|record| !is_terminal_session_state(&record.session.state))
            .map(|record| record.session.session_id.clone())
            .collect()
    }

    /// The `ApplicationSummary` projection `ListApplications` returns.
    pub fn summary(&self) -> ApplicationSummary {
        let application = &self.application;
        ApplicationSummary {
            id: application.application_id.clone(),
            name: application.name.clone(),
            arn: application.arn.clone(),
            release_label: application.release_label.clone(),
            r#type: application.r#type.clone(),
            state: application.state.clone(),
            state_details: application.state_details.clone(),
            created_at: application.created_at,
            updated_at: application.updated_at,
            architecture: application.architecture.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-emrserverless-awstoledowebservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsEmrserverlessAwstoledowebserviceState {
    /// Every stored application, keyed by its id. A `BTreeMap` so `ListApplications`
    /// walks them in one stable order and a cursor taken on one page resumes in the
    /// right place on the next.
    pub applications: BTreeMap<String, ApplicationRecord>,
    /// A `CreateApplication` `clientToken` to what that token already created: the
    /// application, and the fingerprint of the request that created it, so a repeated
    /// create answers with the first application when the request matches and is
    /// refused as a conflict when it does not.
    pub create_application_tokens: HashMap<String, CreateApplicationReplay>,
    /// The counter application ids are minted from.
    pub next_application_seq: u64,
    /// The counter session ids are minted from.
    ///
    /// Store-wide rather than per application, because a session id names ONE session:
    /// the endpoint a client is handed for a session is a host built from that id
    /// ([`session_endpoint_url`]), so two sessions carrying the same id would be reached
    /// at the same host.
    pub next_session_seq: u64,
}

impl ComAmazonawsEmrserverlessAwstoledowebserviceState {
    /// Mint an unused application id matching `ApplicationId`'s `^[0-9a-z]+$`.
    pub fn mint_application_id(&mut self) -> String {
        loop {
            self.next_application_seq = self.next_application_seq.wrapping_add(1);
            let id = format!("{:016x}", self.next_application_seq);
            if !self.applications.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint a session id matching `SessionId`'s `^[0-9a-z]+$` that NO application holds
    /// a session under, the same way [`Self::mint_application_id`] mints an application
    /// id: sixteen lowercase hex digits, with the loop skipping an id already taken,
    /// which is what makes the counter's wrap safe.
    pub fn mint_session_id(&mut self) -> String {
        loop {
            self.next_session_seq = self.next_session_seq.wrapping_add(1);
            let id = format!("{:016x}", self.next_session_seq);
            if !self.holds_session(&id) {
                return id;
            }
        }
    }

    /// Whether any application in the store holds a session under `session_id`.
    fn holds_session(&self, session_id: &str) -> bool {
        self.applications
            .values()
            .any(|record| record.sessions.contains_key(session_id))
    }

    /// The application `id` names, with every state change the clock owes it (and its
    /// job runs) already materialised.
    pub fn settled_mut(&mut self, id: &str, now: SystemTime) -> Option<&mut ApplicationRecord> {
        let record = self.applications.get_mut(id)?;
        record.settle(now);
        Some(record)
    }

    /// Materialise every state change the clock owes, for a caller about to report on
    /// all of them.
    pub fn settle_all(&mut self, now: SystemTime) {
        for record in self.applications.values_mut() {
            record.settle(now);
        }
    }

    /// Take the log artifacts the clock owes every job run that has just finished, so the
    /// caller can write them with this store's guard released.
    ///
    /// Each run's artifacts are handed out ONCE: the record's owed flag is cleared here,
    /// so however many calls settle the store afterwards, one finished run publishes one
    /// set of artifacts. See [`job_run_logs`] for what they are and where they go.
    pub fn take_due_job_run_log_publications(
        &mut self,
    ) -> Vec<job_run_logs::PendingPublication> {
        let mut publications = Vec::new();
        for application in self.applications.values_mut() {
            for record in application.job_runs.values_mut() {
                if !record.log_publication_due {
                    continue;
                }
                record.log_publication_due = false;
                if let Some(publication) = job_run_logs::publication(record) {
                    publications.push(publication);
                }
            }
        }
        publications
    }

    /// The application the ARN `application_arn` names.
    ///
    /// Matched on the ARN the create STORED rather than on an id parsed out of the
    /// request, so a well-formed ARN of another partition, region or account names no
    /// application here even when one happens to carry the same id.
    fn application_by_arn(&self, application_arn: &str) -> Option<&ApplicationRecord> {
        self.applications
            .values()
            .find(|record| record.application.arn == application_arn)
    }

    /// The same, for a caller about to write to the record.
    fn application_by_arn_mut(
        &mut self,
        application_arn: &str,
    ) -> Option<&mut ApplicationRecord> {
        self.applications
            .values_mut()
            .find(|record| record.application.arn == application_arn)
    }

    /// The tag pairs the resource `resource_arn` names carries, or the refusal this
    /// service answers an ARN naming no such resource with.
    ///
    /// The tags of a resource live on the ONE record that resource is stored in
    /// (`Application$tags`, `JobRun$tags`, `Session$tags`), so this reports exactly
    /// what the resource's own `Get` reports. A resource that carries no tag map
    /// carries no tags, which is an EMPTY set rather than an absent one.
    pub fn resource_tags(
        &self,
        resource_arn: &str,
    ) -> Result<HashMap<String, String>, ResourceNotFoundException> {
        match parse_resource_arn(resource_arn) {
            TaggedResource::Application { application_arn } => {
                let record = self
                    .application_by_arn(application_arn)
                    .ok_or_else(application_not_found)?;
                Ok(record.application.tags.clone().unwrap_or_default())
            }
            TaggedResource::JobRun {
                application_arn,
                job_run_id,
            } => {
                let record = self
                    .application_by_arn(application_arn)
                    .ok_or_else(application_not_found)?;
                let run = record
                    .job_runs
                    .get(job_run_id)
                    .ok_or_else(job_run_not_found)?;
                Ok(run.job_run.tags.clone().unwrap_or_default())
            }
            TaggedResource::Session {
                application_arn,
                session_id,
            } => {
                let record = self
                    .application_by_arn(application_arn)
                    .ok_or_else(application_not_found)?;
                let session = record
                    .sessions
                    .get(session_id)
                    .ok_or_else(session_not_found)?;
                Ok(session.session.tags.clone().unwrap_or_default())
            }
        }
    }

    /// The stored tag map of the resource `resource_arn` names, for a caller about to
    /// add or remove pairs in it, or the same refusal for an ARN naming no resource.
    pub fn resource_tags_mut(
        &mut self,
        resource_arn: &str,
    ) -> Result<&mut HashMap<String, String>, ResourceNotFoundException> {
        match parse_resource_arn(resource_arn) {
            TaggedResource::Application { application_arn } => {
                let record = self
                    .application_by_arn_mut(application_arn)
                    .ok_or_else(application_not_found)?;
                Ok(record.application.tags.get_or_insert_default())
            }
            TaggedResource::JobRun {
                application_arn,
                job_run_id,
            } => {
                let record = self
                    .application_by_arn_mut(application_arn)
                    .ok_or_else(application_not_found)?;
                let run = record
                    .job_runs
                    .get_mut(job_run_id)
                    .ok_or_else(job_run_not_found)?;
                Ok(run.job_run.tags.get_or_insert_default())
            }
            TaggedResource::Session {
                application_arn,
                session_id,
            } => {
                let record = self
                    .application_by_arn_mut(application_arn)
                    .ok_or_else(application_not_found)?;
                let session = record
                    .sessions
                    .get_mut(session_id)
                    .ok_or_else(session_not_found)?;
                Ok(session.session.tags.get_or_insert_default())
            }
        }
    }
}

/// The ARN of an application:
/// `arn:aws:emr-serverless:<region>:<account>:/applications/<applicationId>`.
///
/// Source: `ApplicationArn`'s pattern
/// `^arn:(aws[a-zA-Z0-9-]*):emr-serverless:.+:(\d{12}):\/applications\/[0-9a-zA-Z]+$`
/// — note the leading slash on the resource segment.
pub fn application_arn(region: &str, application_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("/applications/{application_id}"),
    )
}

/// The ARN of a job run, which `JobArn`'s pattern defines as its application's ARN
/// followed by `/jobruns/<jobRunId>`.
pub fn job_run_arn(application_arn: &str, job_run_id: &str) -> String {
    format!("{application_arn}/jobruns/{job_run_id}")
}

/// The ARN of a session, which `SessionArn`'s pattern
/// `^arn:(aws[a-zA-Z0-9-]*):emr-serverless:.+:(\d{12}):\/applications\/[0-9a-zA-Z]+\/sessions\/[0-9a-zA-Z]+$`
/// defines as its application's ARN followed by `/sessions/<sessionId>`.
pub fn session_arn(application_arn: &str, session_id: &str) -> String {
    format!("{application_arn}/sessions/{session_id}")
}

/// The region this request was signed for — the region every ARN it mints names.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The IAM principal this request was signed by: the `createdBy` of what it creates.
///
/// `RequestIdentityUserArn`'s pattern is
/// `^arn:(aws[a-zA-Z0-9-]*):(iam|sts)::(\d{12})?:[\w/+=,.@-]+$`, so the ARN carries no
/// region. The emulator models no identity store beyond the access key id a request is
/// signed with, which is exactly what names the caller here; it is filtered to the
/// characters the pattern's resource segment admits.
pub fn caller_arn(parts: &::http::request::Parts) -> String {
    let akid = ::engine::extract_akid(&parts.headers);
    let user: String = akid
        .chars()
        .filter(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '=' | ',' | '.' | '@' | '-')
        })
        .collect();
    let user = if user.is_empty() {
        "default".to_string()
    } else {
        user
    };
    ::engine::emulator_arn("iam", "", &format!("user/{user}"))
}

/// What a `CreateApplication` `clientToken` already named: the application that token
/// created, and the fingerprint of the request that created it.
///
/// `clientToken` is `@idempotencyToken` on an `@idempotent` operation, which makes the
/// token name ONE request rather than one application: the fingerprint is what tells a
/// retry of that request from a different request reusing its token.
#[derive(Debug, Clone)]
pub struct CreateApplicationReplay {
    pub application_id: String,
    pub fingerprint: String,
}

/// What a `ConflictException` says when a `clientToken` is replayed with a request that
/// is not the one it already created.
///
/// It quotes nothing of either request: the condition is that the two differ, not which
/// member differed, and the token is the caller's own value.
pub const IDEMPOTENCY_CONFLICT_MESSAGE: &str = "Request did not match the original";

/// The `ConflictException` payload for a `clientToken` replayed with different
/// parameters.
pub fn request_did_not_match() -> ConflictException {
    ConflictException {
        message: IDEMPOTENCY_CONFLICT_MESSAGE.to_string(),
    }
}

/// A fingerprint of every member of a `CreateApplication` request EXCEPT its
/// `clientToken`: two requests fingerprint alike exactly when they ask for the same
/// application.
///
/// The members are rendered in a FIXED order, and every map in them in sorted key order,
/// so the fingerprint depends on the values a request carries and not on the order they
/// arrived in or on how a `HashMap` happens to iterate. The digest keeps the stored
/// token table small whatever the request carried.
pub fn create_application_fingerprint(input: &CreateApplicationInput) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("name={:?}", input.name));
    parts.push(format!("releaseLabel={:?}", input.release_label));
    parts.push(format!("type={:?}", input.r#type));
    parts.push(format!("architecture={:?}", input.architecture));
    parts.push(format!("initialCapacity={}", sorted_map(&input.initial_capacity)));
    parts.push(format!("maximumCapacity={:?}", input.maximum_capacity));
    parts.push(format!("tags={}", sorted_map(&input.tags)));
    parts.push(format!(
        "autoStartConfiguration={:?}",
        input.auto_start_configuration
    ));
    parts.push(format!(
        "autoStopConfiguration={:?}",
        input.auto_stop_configuration
    ));
    parts.push(format!(
        "networkConfiguration={:?}",
        input.network_configuration
    ));
    parts.push(format!(
        "imageConfiguration={:?}",
        input.image_configuration
    ));
    parts.push(format!(
        "workerTypeSpecifications={}",
        sorted_map(&input.worker_type_specifications)
    ));
    parts.push(format!(
        "runtimeConfiguration={}",
        fingerprint_configurations(input.runtime_configuration.as_deref())
    ));
    parts.push(format!(
        "monitoringConfiguration={}",
        fingerprint_monitoring(input.monitoring_configuration.as_ref())
    ));
    parts.push(format!(
        "diskEncryptionConfiguration={}",
        fingerprint_disk_encryption(input.disk_encryption_configuration.as_ref())
    ));
    parts.push(format!(
        "interactiveConfiguration={:?}",
        input.interactive_configuration
    ));
    parts.push(format!(
        "schedulerConfiguration={:?}",
        input.scheduler_configuration
    ));
    parts.push(format!(
        "identityCenterConfiguration={:?}",
        input.identity_center_configuration
    ));
    parts.push(format!(
        "jobLevelCostAllocationConfiguration={:?}",
        input.job_level_cost_allocation_configuration
    ));
    digest(&parts.join("\n"))
}

/// A map rendered in sorted key order, so its fingerprint does not depend on iteration
/// order. Values that themselves hold a map have their own renderer below.
fn sorted_map<V: Debug>(map: &Option<HashMap<String, V>>) -> String {
    match map {
        None => "none".to_string(),
        Some(map) => {
            let sorted: BTreeMap<&String, &V> = map.iter().collect();
            format!("{sorted:?}")
        }
    }
}

/// A `Configuration` list rendered with each entry's `properties` map sorted, and the
/// same applied to the nested lists each entry may carry.
fn fingerprint_configurations(configurations: Option<&[Configuration]>) -> String {
    match configurations {
        None => "none".to_string(),
        Some(list) => {
            let rendered: Vec<String> = list.iter().map(fingerprint_configuration).collect();
            format!("[{}]", rendered.join(","))
        }
    }
}

fn fingerprint_configuration(configuration: &Configuration) -> String {
    format!(
        "{{classification={:?},properties={},configurations={}}}",
        configuration.classification,
        sorted_map(&configuration.properties),
        fingerprint_configurations(configuration.configurations.as_deref())
    )
}

/// The monitoring configuration, with the CloudWatch block's `logTypes` map sorted.
fn fingerprint_monitoring(configuration: Option<&MonitoringConfiguration>) -> String {
    match configuration {
        None => "none".to_string(),
        Some(configuration) => format!(
            "{{s3={:?},managedPersistence={:?},prometheus={:?},cloudWatch={}}}",
            configuration.s3_monitoring_configuration,
            configuration.managed_persistence_monitoring_configuration,
            configuration.prometheus_monitoring_configuration,
            fingerprint_cloud_watch(configuration.cloud_watch_logging_configuration.as_ref())
        ),
    }
}

fn fingerprint_cloud_watch(configuration: Option<&CloudWatchLoggingConfiguration>) -> String {
    match configuration {
        None => "none".to_string(),
        Some(configuration) => format!(
            "{{enabled={:?},logGroupName={:?},logStreamNamePrefix={:?},encryptionKeyArn={:?},\
             logTypes={}}}",
            configuration.enabled,
            configuration.log_group_name,
            configuration.log_stream_name_prefix,
            configuration.encryption_key_arn,
            sorted_map(&configuration.log_types)
        ),
    }
}

/// The disk encryption configuration, with its `encryptionContext` map sorted.
fn fingerprint_disk_encryption(configuration: Option<&DiskEncryptionConfiguration>) -> String {
    match configuration {
        None => "none".to_string(),
        Some(configuration) => format!(
            "{{encryptionContext={},encryptionKeyArn={:?}}}",
            sorted_map(&configuration.encryption_context),
            configuration.encryption_key_arn
        ),
    }
}

/// The SHA-256 of `value`, hex-encoded: a fixed-size stand-in for the rendered request.
fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

/// The dashboard grant to record for the `sequence`-th URL this job run has been
/// granted, generated at `now`.
///
/// The token is derived from the run's own ARN, the attempt the URL is for and the
/// grant counter, so every call mints a DIFFERENT token for the same run (the
/// operation documents exactly that: "The URL is valid for one hour after you generate
/// it. To access the application UI after that hour elapses, you must invoke the API
/// again to generate a new URL").
pub fn mint_dashboard_grant(
    job_run_arn: &str,
    attempt: i32,
    sequence: u64,
    now: SystemTime,
) -> DashboardGrant {
    let since_epoch = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let token = digest(&format!("{job_run_arn}|{attempt}|{sequence}|{since_epoch}"));
    DashboardGrant {
        // `Url` is `@length(1..2048)`, and a 32-character token leaves the URL far
        // inside it while staying unguessable.
        token: token[..32].to_string(),
        expires_at: due_at(now, DASHBOARD_URL_VALIDITY),
    }
}

/// The URL a caller views one job run's dashboard at: the endpoint this request reached
/// the emulator on, the run it names, and the grant minted for this call.
///
/// The emulator serves no live Spark or Tez UI (it runs no workload), so the URL names
/// the dashboard path of the endpoint the caller is already talking to rather than an
/// Amazon EMR Studio host nothing here answers for.
pub fn job_run_dashboard_url(
    parts: &::http::request::Parts,
    region: &str,
    application_id: &str,
    job_run_id: &str,
    attempt: i32,
    grant: &DashboardGrant,
) -> String {
    let endpoint = request_endpoint(parts, region);
    format!(
        "{endpoint}/dashboard/applications/{application_id}/jobruns/{job_run_id}\
         ?attempt={attempt}&authToken={}",
        grant.token
    )
}

/// A fresh authentication token for connecting a client to one interactive session's
/// endpoint, for `GetSessionEndpoint`.
///
/// The token authenticates to the SESSION endpoint, not to this emulator, so it must
/// never be accepted as a SigV4 credential — but it is still a secret handed to a
/// caller, and nothing in a service may construct a secret itself. It is therefore
/// minted by the one component that generates secrets, the emulator's credential store,
/// and then removed from that store again: the caller gets the secret, the emulator
/// keeps no credential it would accept, and nothing accumulates per call.
///
/// `None` when the store minted no session-token secret, which leaves the operation to
/// refuse rather than report a token it made up.
pub fn mint_session_auth_token(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let token = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .map(str::to_string);
    // The token is not a credential of this emulator; see above.
    let _ = store.delete(&minted.id);
    token
}

/// The endpoint a client connects a Livy or Spark Connect client to one interactive
/// session on: an https ORIGIN whose host names that session under the EMR Serverless
/// session-endpoint domain of the region the call was signed for, and nothing else — no
/// port, no path, no query.
///
/// A client reads this value to build a connection: it takes the host out of it for an
/// SNI or `Host` header, or hands the whole origin to a client library that appends its
/// own protocol path. So the value is an origin naming the session, derived from the
/// STORED session's own id and the request's region — every call for one session reports
/// the same endpoint, and two sessions never share one.
pub fn session_endpoint_url(region: &str, session_id: &str) -> String {
    let url = session_endpoint_origin(region, session_id);
    if url.chars().count() <= ENDPOINT_URL_MAX_LENGTH {
        return url;
    }
    // A request signed for a region name long enough to push the origin past
    // `EndpointUrl`'s bound is answered for this service's default region, whose length
    // no request can influence.
    session_endpoint_origin(DEFAULT_REGION, session_id)
}

/// The one place the session-endpoint host is spelled: the session's label, the
/// session-endpoint domain, the region, and the AWS suffix.
fn session_endpoint_origin(region: &str, session_id: &str) -> String {
    format!("https://{session_id}.{SESSION_ENDPOINT_DOMAIN}.{region}.amazonaws.com")
}

/// The endpoint this request arrived on, falling back to the public EMR Serverless
/// endpoint for `region` when the request names no authority at all.
fn request_endpoint(parts: &::http::request::Parts, region: &str) -> String {
    let authority = parts
        .headers
        .get(::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .filter(|host| !host.is_empty())
        .map(ToString::to_string)
        .or_else(|| parts.uri.authority().map(ToString::to_string));
    match authority {
        Some(authority) => format!("https://{authority}"),
        None => format!("https://{ARN_NAMESPACE}.{region}.amazonaws.com"),
    }
}

/// The `autoStartConfiguration` to persist: the caller's, with
/// `AutoStartConfig$enabled`'s documented default filled in.
pub fn adopt_auto_start(config: Option<AutoStartConfig>) -> Option<AutoStartConfig> {
    let config = config.unwrap_or_default();
    Some(AutoStartConfig {
        enabled: Some(config.enabled.unwrap_or(DEFAULT_AUTO_START_ENABLED)),
    })
}

/// The `autoStopConfiguration` to persist: the caller's, with `AutoStopConfig`'s two
/// documented defaults filled in.
pub fn adopt_auto_stop(config: Option<AutoStopConfig>) -> Option<AutoStopConfig> {
    let config = config.unwrap_or_default();
    Some(AutoStopConfig {
        enabled: Some(config.enabled.unwrap_or(DEFAULT_AUTO_STOP_ENABLED)),
        idle_timeout_minutes: Some(
            config
                .idle_timeout_minutes
                .unwrap_or(DEFAULT_IDLE_TIMEOUT_MINUTES),
        ),
    })
}

/// The `tags` to persist: the caller's, or the empty map an application created with
/// no tags carries.
///
/// `Application$tags` is "the tags assigned to the application", a `@length(0..200)`
/// map, so an application with none has an EMPTY tag map rather than no tag map, and
/// every read reports it.
pub fn adopt_tags(tags: Option<HashMap<String, String>>) -> Option<HashMap<String, String>> {
    Some(tags.unwrap_or_default())
}

/// The resource a `ResourceArn` names: the application ARN that owns it, and the child
/// the ARN names under that application.
///
/// Source: `ResourceArn`'s pattern
/// `^arn:(aws[a-zA-Z0-9-]*):emr-serverless:.+:(\d{12}):\/applications\/[0-9a-zA-Z]+(\/jobruns\/[0-9a-zA-Z]+|\/sessions\/[0-9a-zA-Z]+)?$`
/// — an application ARN, optionally followed by one `/jobruns/<id>` or `/sessions/<id>`
/// pair. `TagResource`, `UntagResource` and `ListTagsForResource` all bind that one
/// shape, so they read an ARN the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource<'a> {
    /// An application, named by the ARN it was minted with.
    Application { application_arn: &'a str },
    /// A job run of the application the ARN's leading part names.
    JobRun {
        application_arn: &'a str,
        job_run_id: &'a str,
    },
    /// An interactive session of that application.
    Session {
        application_arn: &'a str,
        session_id: &'a str,
    },
}

/// Read `resource_arn` as one of the three resources the pattern admits.
///
/// INFALLIBLE: the modelled pattern is refused at the boundary, so an ARN that reaches
/// a handler is one of these three forms. An `ApplicationId` is `[0-9a-zA-Z]+`, so
/// neither segment pair can appear inside the application part of the ARN.
pub fn parse_resource_arn(resource_arn: &str) -> TaggedResource<'_> {
    if let Some((application_arn, job_run_id)) = resource_arn.rsplit_once("/jobruns/") {
        return TaggedResource::JobRun {
            application_arn,
            job_run_id,
        };
    }
    if let Some((application_arn, session_id)) = resource_arn.rsplit_once("/sessions/") {
        return TaggedResource::Session {
            application_arn,
            session_id,
        };
    }
    TaggedResource::Application {
        application_arn: resource_arn,
    }
}

/// The `architecture` to persist: the caller's, or [`DEFAULT_ARCHITECTURE`].
pub fn adopt_architecture(architecture: Option<String>) -> Option<String> {
    Some(architecture.unwrap_or_else(|| DEFAULT_ARCHITECTURE.to_string()))
}

/// The `monitoringConfiguration` to persist: the caller's, with the managed log
/// persistence every application runs under materialised inside it.
///
/// `Application$monitoringConfiguration` reports "the configuration setting for
/// monitoring" THIS application has, which every application has — nothing in it is
/// required — so a read reports the configuration the service applied rather than
/// omitting the block, down to the blocks INSIDE it that carry a documented default.
pub fn adopt_monitoring(
    config: Option<MonitoringConfiguration>,
) -> Option<MonitoringConfiguration> {
    let mut config = config.unwrap_or_default();
    config.managed_persistence_monitoring_configuration = Some(adopt_managed_persistence(
        config.managed_persistence_monitoring_configuration.take(),
    ));
    Some(config)
}

/// The `managedPersistenceMonitoringConfiguration` to persist: the caller's, with
/// [`DEFAULT_MANAGED_PERSISTENCE_ENABLED`] filled in. The encryption key stays whatever
/// the caller named; there is no default key.
fn adopt_managed_persistence(
    config: Option<ManagedPersistenceMonitoringConfiguration>,
) -> ManagedPersistenceMonitoringConfiguration {
    let config = config.unwrap_or_default();
    ManagedPersistenceMonitoringConfiguration {
        enabled: Some(config.enabled.unwrap_or(DEFAULT_MANAGED_PERSISTENCE_ENABLED)),
        encryption_key_arn: config.encryption_key_arn,
    }
}

/// The `jobLevelCostAllocationConfiguration` to persist, on the same rule: the caller's,
/// with [`DEFAULT_JOB_LEVEL_COST_ALLOCATION_ENABLED`] filled in.
pub fn adopt_job_level_cost_allocation(
    config: Option<JobLevelCostAllocationConfiguration>,
) -> Option<JobLevelCostAllocationConfiguration> {
    let config = config.unwrap_or_default();
    Some(JobLevelCostAllocationConfiguration {
        enabled: Some(
            config
                .enabled
                .unwrap_or(DEFAULT_JOB_LEVEL_COST_ALLOCATION_ENABLED),
        ),
    })
}

/// The `imageConfiguration` to persist for the `ImageConfigurationInput` a request
/// carried.
///
/// `ImageConfiguration$imageUri` is required, so an input naming no image stores
/// nothing. `resolvedImageDigest` stays unset: resolving it means pulling the image,
/// which this emulator does not do, so there is no digest to report.
pub fn adopt_image_configuration(
    input: Option<ImageConfigurationInput>,
) -> Option<ImageConfiguration> {
    let input = input?;
    let image_uri = input.image_uri?;
    Some(ImageConfiguration {
        image_uri,
        resolved_image_digest: None,
        application_level_digest_resolution: input.application_level_digest_resolution,
    })
}

/// The `workerTypeSpecifications` to persist for the input map a request carried.
pub fn adopt_worker_type_specifications(
    input: Option<HashMap<String, WorkerTypeSpecificationInput>>,
) -> Option<HashMap<String, WorkerTypeSpecification>> {
    let input = input?;
    Some(
        input
            .into_iter()
            .map(|(worker_type, specification)| {
                (
                    worker_type,
                    WorkerTypeSpecification {
                        image_configuration: adopt_image_configuration(
                            specification.image_configuration,
                        ),
                    },
                )
            })
            .collect(),
    )
}

/// The `identityCenterConfiguration` to persist for the input a request carried.
///
/// `identityCenterApplicationArn` names an IAM Identity Center application this
/// emulator does not create, so it stays unset rather than invented.
pub fn adopt_identity_center_configuration(
    input: Option<IdentityCenterConfigurationInput>,
) -> Option<IdentityCenterConfiguration> {
    let input = input?;
    Some(IdentityCenterConfiguration {
        identity_center_instance_arn: input.identity_center_instance_arn,
        identity_center_application_arn: None,
        user_background_sessions_enabled: input.user_background_sessions_enabled,
    })
}

/// The page size a list operation serves: the caller's `maxResults`, or [`PAGE_LIMIT`]
/// when the request named none. The modelled `@range(1..50)` is already enforced at the
/// boundary, so this only has to turn it into a usable count.
pub fn page_size(max_results: Option<i32>) -> usize {
    let requested = max_results.unwrap_or(PAGE_LIMIT);
    usize::try_from(requested.clamp(1, PAGE_LIMIT)).unwrap_or(1)
}

/// True when `value` satisfies `ApplicationId` / `JobRunId`'s `^[0-9a-z]+$` with their
/// 1..64 length. The boundary validator already refuses a modelled id member, so this
/// is for a value the model does not constrain: the id carried inside a `nextToken`.
pub fn is_resource_id(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
}

/// Encode the id a page resumes at as an opaque `nextToken`.
///
/// `NextToken`'s pattern is `^[A-Za-z0-9_=-]+$`, which the `v1_` prefix and lowercase
/// hex body both satisfy.
pub fn encode_page_token(id: &str) -> String {
    format!("{PAGE_TOKEN_PREFIX}{}", hex::encode(id))
}

/// The id a `nextToken` resumes at, or `None` when the token is not one this service
/// minted (the caller answers that with a `ValidationException`).
pub fn decode_page_token(token: &str) -> Option<String> {
    let body = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    let id = String::from_utf8(hex::decode(body).ok()?).ok()?;
    is_resource_id(&id).then_some(id)
}

/// What a `ResourceNotFoundException` says when the application a request names does
/// not exist.
///
/// EMR Serverless states a not-found refusal as the fixed phrase for the RESOURCE CLASS
/// that was not found, naming no identifier: the same sentence comes back whichever
/// operation refused and whatever id was asked for. Recorded responses show it on both
/// an application report and a job submission, so the phrase belongs to the resource,
/// not to the operation — hence one constant every handler shares.
pub const APPLICATION_NOT_FOUND_MESSAGE: &str = "Application does not exist";

/// The same, for the job run a request names.
pub const JOB_RUN_NOT_FOUND_MESSAGE: &str = "Job Run with given ID doesn't exist";

/// The same, for a job run ATTEMPT a request names that the run does not have.
///
/// A not-found refusal states which part of the request was not found, so an attempt
/// number naming no attempt of an existing job run is its own phrase: a caller told the
/// ID does not exist would discard an id that is in fact valid.
pub const JOB_RUN_ATTEMPT_NOT_FOUND_MESSAGE: &str = "Job Run with given attempt doesn't exist";

/// What a `ValidationException` says when a job run cannot be cancelled.
///
/// State-agnostic on purpose: it states the condition the operation refuses on (the run
/// is not in a state a cancel can act on), not the state the run happens to be in.
pub const JOB_RUN_NOT_CANCELLABLE_MESSAGE: &str = "Job run is not in a cancellable state";

/// The same, for the session a request names: one phrase for the resource class, naming
/// no identifier, as this service's other not-found refusals do.
///
/// A session is addressed by an ID INSIDE the application the route names, so the phrase
/// is the one this service uses for a child resource whose id resolved to nothing — the
/// same sentence [`JOB_RUN_NOT_FOUND_MESSAGE`] states for the other child of an
/// application, with the resource class as its only difference.
pub const SESSION_NOT_FOUND_MESSAGE: &str = "Session with given ID doesn't exist";

/// What a `ValidationException` says when `StartSession` is asked for on an application
/// that does not enable interactive sessions.
///
/// `StartSession` documents the precondition it refuses on — the application must "have
/// interactive sessions enabled" — and the refusal states that condition as fixed text,
/// carrying no part of the request and no remedy call: the member that enables them is
/// the application's, not the session request's.
pub const SESSIONS_NOT_ENABLED_MESSAGE: &str = "Sessions are not enabled on this application";

/// The `ResourceNotFoundException` payload for an application id that names none.
///
/// Takes no argument on purpose: the message states the resource class, so there is no
/// way for a caller's id to reach the error body through here.
pub fn application_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: APPLICATION_NOT_FOUND_MESSAGE.to_string(),
    }
}

/// The `ResourceNotFoundException` payload for a job run id that names none.
pub fn job_run_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: JOB_RUN_NOT_FOUND_MESSAGE.to_string(),
    }
}

/// The `ResourceNotFoundException` payload for an attempt number that names no attempt
/// of a job run that DOES exist.
pub fn job_run_attempt_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: JOB_RUN_ATTEMPT_NOT_FOUND_MESSAGE.to_string(),
    }
}

/// The `ResourceNotFoundException` payload for a session id that names none.
pub fn session_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: SESSION_NOT_FOUND_MESSAGE.to_string(),
    }
}

/// What a `ValidationException` says when an ACCESS-GRANT read — an operation that mints
/// a URL or an authentication token for a resource inside an application, rather than
/// reporting that resource — is given a job-run id naming no job run.
///
/// An access grant is minted FOR a resource the request names, so a name that resolves
/// to nothing is a fault in the request rather than a resource that went missing: this
/// service answers that class as input validation (HTTP 400), with the sentence for the
/// identifier's resource class as fixed text terminated by a period, and with no part of
/// the request inside it. Recorded on the job-run dashboard path; the resource REPORTS
/// (`GetJobRun`, `CancelJobRun`) keep [`JOB_RUN_NOT_FOUND_MESSAGE`] and its 404.
pub const JOB_RUN_ID_INVALID_MESSAGE: &str = "Job Run with given ID doesn't exist.";

/// The same, for a session id naming no session reachable through the access-grant read
/// that was called.
///
/// A session id names ONE session service-wide (see
/// [`ComAmazonawsEmrserverlessAwstoledowebserviceState::next_session_seq`]), so the
/// session reference a grant is asked for is resolved as a whole: whether the
/// application the route names holds no such session or does not exist at all, what the
/// request got wrong is the session it asked for a grant on. The sentence states that by
/// naming the INPUT MEMBER, carrying neither the id submitted for it nor the
/// application's.
pub const SESSION_ID_INVALID_MESSAGE: &str = "SessionId is invalid.";

/// The `ValidationException` payload an access-grant read refuses an unresolvable
/// job-run id with.
pub fn job_run_id_invalid() -> ValidationException {
    ValidationException {
        message: JOB_RUN_ID_INVALID_MESSAGE.to_string(),
    }
}

/// The `ValidationException` payload an access-grant read refuses an unresolvable
/// session reference with.
pub fn session_id_invalid() -> ValidationException {
    ValidationException {
        message: SESSION_ID_INVALID_MESSAGE.to_string(),
    }
}

/// Now, as the wire timestamp type every modelled timestamp member uses.
pub fn now_timestamp(now: SystemTime) -> DateTime {
    DateTime::from(now)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEmrserverlessAwstoledowebserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEmrserverlessAwstoledowebserviceState>)
}
