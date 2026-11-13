// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-emrcontainers-awschicagowebservice service.
//!
//! This holds the virtual-cluster / job-run core loop of EMR on EKS, the job templates
//! `StartJobRun` values are stored in, the security-configuration templates a virtual
//! cluster can reference, and the managed endpoints that gateway into one. Every field a
//! describe or a list answers with is read back out of one of these records: nothing is
//! echoed from the request that asked for it, and nothing is canned.
//!
//! Three things live here beside the records because several operations need them and a
//! second copy would let two operations disagree:
//!
//! * [`ComAmazonawsEmrcontainersAwschicagowebserviceState::mint_id`] — the one place a
//!   `ResourceIdString` (`^[0-9a-z]+$`) is minted.
//! * [`ComAmazonawsEmrcontainersAwschicagowebserviceState::settle`] — the one place a
//!   status transition the clock has already made due is materialised, and where a
//!   managed-endpoint session past its `expiresAt` leaves the store. Every handler,
//!   reporting or mutating, calls it first, so a reader never has to know which other
//!   call started the transition. It is also where a job run's start waits for a running
//!   slot when its virtual cluster configures a `maxConcurrentJobRuns`, so the counts
//!   [`ComAmazonawsEmrcontainersAwschicagowebserviceState::cluster_scheduler_status`]
//!   reports never exceed the capacity the cluster was created with. A run's walk does
//!   not stop at that start: the admission owes the run its OUTCOME in turn, so a client
//!   that polls a run reaches a terminal state with a `finishedAt` without anybody
//!   cancelling it ([`JobRunRecord`]).
//! * [`ComAmazonawsEmrcontainersAwschicagowebserviceState::spent_client_token`] — the one
//!   place a `clientToken` already spent in its scope is judged (replay, conflict or
//!   fresh), so the six operations that take an `@idempotencyToken` cannot drift into six
//!   readings of one rule.
//!
//! Every record carries the region it was created in, and every lookup here takes that
//! region: EMR on EKS is a regional service, so a resource exists only at the endpoint
//! that minted it, and an id offered to another region's endpoint names nothing. The
//! region a request is in is [`request_region`].

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};
use sha2::{Digest, Sha256};

use crate::ops::types::{
    Certificate, ConfigurationOverrides, ContainerInfo, ContainerProvider, Credentials, Endpoint,
    GetManagedEndpointSessionCredentialsOutput, JobDriver, JobRun, JobTemplate, JobTemplateData,
    RetryPolicyConfiguration, RetryPolicyExecution, SchedulerConfiguration, SchedulerStatus,
    SecurityConfiguration, SecurityConfigurationData, VirtualCluster,
};

/// What a managed endpoint is reached THROUGH — its address and the certificate that
/// secures it — provisioned when the endpoint is created and held on its record
/// ([`ManagedEndpointRecord::connection`]).
mod managed_endpoint_connection;
pub use managed_endpoint_connection::*;

/// The network an endpoint's gateway runs in, as the service that owns the container
/// cluster reports it: resolved, never derived here.
mod container_cluster_network;
pub use container_cluster_network::*;

/// The one decision a `clientToken` already spent in its scope is judged by — replay,
/// conflict or fresh — for every `@idempotencyToken` operation of this service.
mod spent_client_token;
pub use spent_client_token::*;

/// The ARN namespace this service's ARNs carry (`OPERATIONS.md`: ARN namespace
/// `emr-containers`), e.g.
/// `arn:aws:emr-containers:{region}:{account}:/virtualclusters/{id}`.
pub const ARN_SERVICE: &str = "emr-containers";

/// The region an ARN is built in when the request carries no SigV4 credential scope to
/// read one from (an unsigned in-process call, say).
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long a status transition the clock owes takes to come due: `PENDING` ->
/// `RUNNING` after `StartJobRun`, `CANCEL_PENDING` -> `CANCELLED` after `CancelJobRun`,
/// `TERMINATING` -> `TERMINATED` after `DeleteVirtualCluster`, and the terminal step each
/// resource a torn-down cluster holds takes with it
/// ([`ComAmazonawsEmrcontainersAwschicagowebserviceState::start_virtual_cluster_teardown`]).
///
/// EMR on EKS documents no duration for any of them, so this span is the emulator's own
/// choice. It is deliberately non-zero: a caller that reads straight after the call that
/// started the transition observes the transient state, exactly as it would against AWS.
///
/// The span is when a transition becomes DUE, not when it is materialised: an admission
/// to `RUNNING` also waits for the cluster's scheduler to have a free concurrency slot
/// (see [`ComAmazonawsEmrcontainersAwschicagowebserviceState::settle`]).
pub const TRANSITION_SPAN: Duration = Duration::from_millis(400);

/// How long a job run this service has admitted stays `RUNNING` before the outcome of
/// its driver comes due.
///
/// This is not a status step a resource takes by itself ([`TRANSITION_SPAN`]) but the
/// span a run spends executing, so it is deliberately long enough that a client which
/// polls `DescribeJobRun` observes the run `RUNNING` over several reads and still sees it
/// reach a terminal state in seconds. EMR on EKS documents no duration for a job run —
/// it is the submitted work's own — so this span is the emulator's own choice.
pub const JOB_OUTCOME_SPAN: Duration = Duration::from_secs(5);

/// `VirtualClusterState` members this slice uses. Each is a value of the generated
/// `VIRTUAL_CLUSTER_STATE_VALUES` catalog in `ops::types::shared_types`.
pub const CLUSTER_RUNNING: &str = "RUNNING";
pub const CLUSTER_TERMINATING: &str = "TERMINATING";
pub const CLUSTER_TERMINATED: &str = "TERMINATED";

/// `JobRunState` members this slice uses. Each is a value of the generated
/// `JOB_RUN_STATE_VALUES` catalog in `ops::types::shared_types`.
///
/// `JOB_SUBMITTED` is the one state no handler here writes: EMR on EKS reports it
/// for a run handed to the scheduler, and this slice accepts a run `PENDING` and admits it
/// to `RUNNING` from there. It is named because `job_state_is_terminal` and
/// [`job_state_is_in_queue`] are written against the whole catalog, so a run in either
/// waiting state is counted as queued.
///
/// `JOB_COMPLETED` is the outcome an admitted run the caller leaves alone reaches, and
/// `JOB_FAILED` the one it reaches when the compute it was admitted to is no longer there
/// to finish it ([`job_run_outcome`]).
pub const JOB_PENDING: &str = "PENDING";
pub const JOB_SUBMITTED: &str = "SUBMITTED";
pub const JOB_RUNNING: &str = "RUNNING";
pub const JOB_CANCEL_PENDING: &str = "CANCEL_PENDING";
pub const JOB_CANCELLED: &str = "CANCELLED";
pub const JOB_FAILED: &str = "FAILED";
pub const JOB_COMPLETED: &str = "COMPLETED";

/// `EndpointState` members this slice uses. Each is a value of the generated
/// `ENDPOINT_STATE_VALUES` catalog in `ops::types::shared_types`
/// (`smithy_query(query="shape", shape="EndpointState")`:
/// `CREATING`, `ACTIVE`, `TERMINATING`, `TERMINATED`, `TERMINATED_WITH_ERRORS`).
///
/// A managed endpoint is accepted `CREATING` and reaches `ACTIVE` on the clock.
/// `TERMINATED_WITH_ERRORS` is the one state nothing here writes: it reports a teardown
/// that failed, and nothing in this emulator can fail a teardown. It is named because
/// [`endpoint_state_is_terminal`] is written against the whole catalog.
pub const ENDPOINT_CREATING: &str = "CREATING";
pub const ENDPOINT_ACTIVE: &str = "ACTIVE";
pub const ENDPOINT_TERMINATING: &str = "TERMINATING";
pub const ENDPOINT_TERMINATED: &str = "TERMINATED";
pub const ENDPOINT_TERMINATED_WITH_ERRORS: &str = "TERMINATED_WITH_ERRORS";

/// The `EndpointState` values no further transition leaves.
pub const ENDPOINT_TERMINAL_STATES: &[&str] =
    &[ENDPOINT_TERMINATED, ENDPOINT_TERMINATED_WITH_ERRORS];

/// Whether an `EndpointState` is one a managed endpoint never leaves.
pub fn endpoint_state_is_terminal(state: &str) -> bool {
    ENDPOINT_TERMINAL_STATES.contains(&state)
}

/// The `JobRunState` values no further transition leaves. A job run that reaches one of
/// them has a `finishedAt`.
pub const JOB_TERMINAL_STATES: &[&str] = &[JOB_CANCELLED, JOB_FAILED, JOB_COMPLETED];

/// Whether a `JobRunState` is one a job run never leaves.
pub fn job_state_is_terminal(state: &str) -> bool {
    JOB_TERMINAL_STATES.contains(&state)
}

/// The `JobRunState` values a run WAITING for scheduler capacity is in. Both
/// `SchedulerStatus$currentInQueueJobRuns` ("job runs currently waiting in the queue
/// (`PENDING` or `SUBMITTED`)") and `SchedulerConfiguration$maxInQueueJobRuns` ("job runs
/// that can be in the `PENDING` or `SUBMITTED` state at any time") name the same two.
pub const JOB_IN_QUEUE_STATES: &[&str] = &[JOB_PENDING, JOB_SUBMITTED];

/// Whether a `JobRunState` is one a run waiting in the scheduler queue is in.
pub fn job_state_is_in_queue(state: &str) -> bool {
    JOB_IN_QUEUE_STATES.contains(&state)
}

/// `JobRun$stateDetails` for a run this service has admitted to its virtual cluster.
pub const JOB_RUNNING_STATE_DETAILS: &str = "Job run admitted to the virtual cluster.";

/// `JobRun$stateDetails` for a run whose cancellation has been accepted but not yet
/// carried out.
pub const JOB_CANCEL_PENDING_STATE_DETAILS: &str = "Job run cancellation requested.";

/// `JobRun$stateDetails` for a run a cancellation ended.
pub const JOB_CANCELLED_STATE_DETAILS: &str = "Job run cancelled before it finished.";

/// `JobRun$stateDetails` for a run whose driver ran to the end of the span this service
/// gives it with nothing cancelling it and its compute up throughout.
pub const JOB_COMPLETED_STATE_DETAILS: &str = "Job run completed.";

/// `JobRun$stateDetails` for a run whose compute went away before its driver was done.
pub const JOB_CLUSTER_UNAVAILABLE_STATE_DETAILS: &str =
    "The virtual cluster this job run was admitted to is no longer available, so the job \
     run did not finish.";

/// `JobRun$failureReason` for that outcome, a value of the modelled `FailureReason` enum
/// (`INTERNAL_ERROR`, `USER_ERROR`, `VALIDATION_ERROR`, `CLUSTER_UNAVAILABLE`).
///
/// Nothing the submission carried is at fault — the two caller-facing values do not apply
/// — and the run did not end for a fault of this service either: the compute behind the
/// virtual cluster the run was admitted to stopped being available to it, which is the
/// condition this value names.
pub const JOB_CLUSTER_UNAVAILABLE_FAILURE_REASON: &str = "CLUSTER_UNAVAILABLE";

/// The `JobRunState` an admitted run's driver ends in, and the `JobRun$failureReason` that
/// end carries, given whether the compute the run was admitted to is still available to
/// it.
///
/// A job run is work submitted to the virtual cluster's compute, so the end of its driver
/// is a statement about that compute: while it is there, the driver this service admitted
/// and nobody cancelled reached its end, which is the terminal state AWS reports for a run
/// that finished and carries no `failureReason`; once it is gone, the run cannot be
/// finished on it and ends in the modelled failure state for compute that is unavailable
/// ([`JOB_CLUSTER_UNAVAILABLE_FAILURE_REASON`]).
pub fn job_run_outcome(compute_available: bool) -> (&'static str, Option<&'static str>) {
    if compute_available {
        (JOB_COMPLETED, None)
    } else {
        (JOB_FAILED, Some(JOB_CLUSTER_UNAVAILABLE_FAILURE_REASON))
    }
}

/// The `JobRun$stateDetails` — "additional details of the job run state" — a run reports
/// once it has settled into `state`.
///
/// One mapping for every state this service moves a run INTO, so a run's detail always
/// describes the state it is reported in rather than an earlier one. A state a run merely
/// waits in carries no detail beyond the state itself.
pub fn job_run_state_details(state: &str) -> Option<&'static str> {
    match state {
        JOB_RUNNING => Some(JOB_RUNNING_STATE_DETAILS),
        JOB_CANCEL_PENDING => Some(JOB_CANCEL_PENDING_STATE_DETAILS),
        JOB_CANCELLED => Some(JOB_CANCELLED_STATE_DETAILS),
        JOB_COMPLETED => Some(JOB_COMPLETED_STATE_DETAILS),
        JOB_FAILED => Some(JOB_CLUSTER_UNAVAILABLE_STATE_DETAILS),
        _ => None,
    }
}

/// The scheduler limits a submitted `SchedulerConfiguration` actually configures.
///
/// `None` when it configures none — both an absent structure and one whose every member
/// is omitted, which are the same statement: `SchedulerConfiguration` is documented as
/// "when not set, no concurrency or queue limits are applied", each member as "if you
/// omit this field, the service applies no … limit", and `VirtualCluster$schedulerConfiguration`
/// as "the service does not return this field when no scheduler limits are configured".
/// Normalising on the way IN is what lets every read answer that, whether the cluster was
/// created with an empty configuration or had its last limit removed by an update.
pub fn configured_scheduler_limits(
    configuration: Option<SchedulerConfiguration>,
) -> Option<SchedulerConfiguration> {
    configuration.filter(|configuration| {
        configuration.max_in_queue_job_runs.is_some()
            || configuration.max_concurrent_job_runs.is_some()
    })
}

/// The refusal a `JobDriver` that does not name the one driver its job runs on earns,
/// and `None` when it names exactly one.
///
/// `JobDriver` is documented as "Specify the driver that the job runs on. Exactly one of
/// the two available job drivers is required, either sparkSqlJobDriver or
/// sparkSubmitJobDriver" (smithy_query: shape `JobDriver`), while BOTH of its members are
/// optional in the model — so the boundary's constraint walk admits a value that names
/// neither driver and one that names both, and neither can run. The rule belongs to the
/// shape, so it is checked here, once, and applied by every operation that accepts a
/// `JobDriver`: the submission that carries one, and the job template that stores one for
/// a later submission to run.
///
/// `member` is where the value sits in the request, so the refusal says which one it is
/// about.
pub fn job_driver_refusal(member: &str, driver: &JobDriver) -> Option<String> {
    let named = usize::from(driver.spark_submit_job_driver.is_some())
        + usize::from(driver.spark_sql_job_driver.is_some());
    (named != 1).then(|| {
        format!(
            "{member} must specify exactly one of the two available job drivers, either sparkSqlJobDriver or sparkSubmitJobDriver."
        )
    })
}

/// The alphabet a minted id is drawn from: `ResourceIdString` is
/// `^[0-9a-z]+$` (len 1..64), so base-36 over `0-9a-z` is exactly its character set.
const ID_ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Characters in a minted id. Well inside `ResourceIdString`'s 1..64 length, and wide
/// enough that the wall clock plus the per-state counter below never collide.
const ID_WIDTH: usize = 25;

/// The most virtual clusters this emulator keeps at once.
///
/// Not an AWS quota: it is the emulator's own retention cap, so the store cannot grow
/// with the number of requests served. A retired cluster (TERMINATED and holding no job
/// runs) is reclaimed oldest-first to make room; once every slot holds a live cluster,
/// `CreateVirtualCluster` is refused rather than allocating further. Sized well above any
/// plausible test or demo workload, so a client never meets it by accident.
pub const MAX_VIRTUAL_CLUSTERS: usize = 200;

/// The most job runs this emulator keeps at once, on the same terms as
/// [`MAX_VIRTUAL_CLUSTERS`]: a run in a terminal state is reclaimed oldest-first, and
/// `StartJobRun` is refused once every slot holds a run that has not finished.
pub const MAX_JOB_RUNS: usize = 2_000;

/// The most job templates this emulator keeps at once.
///
/// Not an AWS quota: the emulator's own retention cap, so the store cannot grow with the
/// number of requests served. A job template has no lifecycle state, so nothing about it
/// is ever "retired" and there is nothing to reclaim: once every slot is taken,
/// `CreateJobTemplate` is refused until a caller releases one with `DeleteJobTemplate`.
pub const MAX_JOB_TEMPLATES: usize = 500;

/// The most security configurations this emulator keeps at once.
///
/// Not an AWS quota: the emulator's own retention cap, so the store cannot grow with the
/// number of requests served. A security configuration has no status and so no retired
/// form to reclaim — the only way one leaves the store is a caller deleting it — so once
/// every slot is taken `CreateSecurityConfiguration` is refused rather than allocating
/// further. Sized well above any plausible test or demo workload.
pub const MAX_SECURITY_CONFIGURATIONS: usize = 200;

/// The most tags one resource holds.
///
/// `TagMap` is `len 0..50`, and it is the shape of BOTH the tag set a request carries and
/// the tag set a read answers with (`VirtualCluster$tags`, `JobRun$tags`,
/// `ListTagsForResource$tags`), so a resource that held more than this could not be
/// reported inside its own modelled shape. The boundary already refuses a single request
/// carrying more; what has to hold it is a MERGE, which is why `TagResource` checks the
/// size the merged set would reach before writing any of it.
pub const MAX_RESOURCE_TAGS: usize = 50;

/// The most managed endpoints this emulator keeps at once.
///
/// Not an AWS quota: the emulator's own retention cap, on the same terms as
/// [`MAX_VIRTUAL_CLUSTERS`]. An endpoint in a terminal `EndpointState` is reclaimed
/// oldest-first to make room, and the endpoints of a reclaimed virtual cluster go with it;
/// once every slot holds a live endpoint, `CreateManagedEndpoint` is refused rather than
/// growing the store without bound.
pub const MAX_MANAGED_ENDPOINTS: usize = 200;

/// The most managed-endpoint sessions this emulator keeps at once.
///
/// Not an AWS quota: the emulator's own retention cap, on the same terms as
/// [`MAX_MANAGED_ENDPOINTS`]. A session leaves the store on its own expiry
/// ([`ComAmazonawsEmrcontainersAwschicagowebserviceState::settle`]), so the cap is only
/// reached by sessions that are all still valid — and then a further
/// `GetManagedEndpointSessionCredentials` is refused rather than growing the store with
/// the number of requests served.
pub const MAX_ENDPOINT_SESSIONS: usize = 200;

/// The one `credentialType` a managed-endpoint session is issued for.
///
/// `GetManagedEndpointSessionCredentialsRequest$credentialType` documents the value
/// itself: "Type of the token requested. Currently supported and default value of this
/// field is TOKEN", so a request naming another type asks for a credential kind the
/// operation does not issue.
pub const SESSION_CREDENTIAL_TYPE_TOKEN: &str = "TOKEN";

/// How long a managed-endpoint session token is valid when the request names no
/// `durationInSeconds`.
///
/// `GetManagedEndpointSessionCredentialsRequest$durationInSeconds`: "Duration in seconds
/// for which the session token is valid. The default duration is 15 minutes and the
/// maximum is 12 hours."
pub const DEFAULT_SESSION_DURATION_SECONDS: i32 = 15 * 60;

/// The longest a managed-endpoint session token may be asked for: the 12-hour maximum the
/// sentence quoted on [`DEFAULT_SESSION_DURATION_SECONDS`] documents.
pub const MAX_SESSION_DURATION_SECONDS: i32 = 12 * 60 * 60;

/// The idempotency scope `CreateVirtualCluster` remembers a `clientToken` under.
///
/// A token is scoped to the operation that minted the resource, so the same token on
/// another operation is a different request.
pub const CREATE_VIRTUAL_CLUSTER_SCOPE: &str = "CreateVirtualCluster";

/// How many `UpdateVirtualCluster` `clientToken`s one virtual cluster remembers.
///
/// `UpdateVirtualCluster`'s token is remembered ON the cluster it updated rather than in
/// [`ComAmazonawsEmrcontainersAwschicagowebserviceState::idempotency`], because an update
/// mints no resource for a token to name and a cluster may be updated any number of
/// times: a per-record, bounded window keeps the store from growing with the number of
/// updates served (and the window is reclaimed with the cluster). Within it, a retried
/// update is ignored, which is the whole window the `@idempotencyToken` contract is
/// about; a token replayed after this many LATER updates applies again.
pub const MAX_RETAINED_UPDATE_TOKENS: usize = 32;

/// The idempotency scope `CreateSecurityConfiguration` remembers a `clientToken` under,
/// on the same terms as [`CREATE_VIRTUAL_CLUSTER_SCOPE`]: the same token replayed names
/// the security configuration the first call minted.
pub const CREATE_SECURITY_CONFIGURATION_SCOPE: &str = "CreateSecurityConfiguration";

/// The idempotency scope `StartJobRun` remembers a `clientToken` under: scoped to the
/// virtual cluster as well as the operation, because the token identifies one submission
/// to one cluster.
pub fn start_job_run_scope(virtual_cluster_id: &str) -> String {
    format!("StartJobRun:{virtual_cluster_id}")
}

/// The idempotency scope `CreateManagedEndpoint` remembers a `clientToken` under: scoped
/// to the virtual cluster as well as the operation, because a managed endpoint is created
/// IN one cluster (`POST /virtualclusters/{virtualClusterId}/endpoints`) and the token
/// identifies one creation in it.
pub fn create_managed_endpoint_scope(virtual_cluster_id: &str) -> String {
    format!("CreateManagedEndpoint:{virtual_cluster_id}")
}

/// The idempotency scope `GetManagedEndpointSessionCredentials` remembers a `clientToken`
/// under: scoped to the managed endpoint as well as the operation, because a session is
/// issued FOR one endpoint of one virtual cluster
/// (`POST /virtualclusters/{virtualClusterIdentifier}/endpoints/{endpointIdentifier}/credentials`)
/// and the token identifies one such request.
pub fn endpoint_session_scope(virtual_cluster_id: &str, endpoint_id: &str) -> String {
    format!("GetManagedEndpointSessionCredentials:{virtual_cluster_id}/{endpoint_id}")
}

/// The idempotency scope `CreateJobTemplate` remembers a `clientToken` under. A job
/// template is account-and-region wide (`POST /jobtemplates` names no parent resource),
/// so the operation alone scopes the token.
pub const CREATE_JOB_TEMPLATE_SCOPE: &str = "CreateJobTemplate";

/// The `message` an idempotency conflict carries, for every operation of this service
/// whose input declares an `@idempotencyToken` `clientToken`.
///
/// A token already spent in its scope, replayed with a request that differs from the one
/// it named, is refused with ONE fixed sentence. The sentence has no variable parts: it
/// names neither the token, nor the resource that token already named, nor the member that
/// differs — so a caller telling an idempotency conflict apart from the other
/// `ValidationException`s of the same operation (the error code is shared by all of them)
/// reads the same text whichever operation refused it, and the refusal discloses no
/// resource identifier on a path that answers with none.
pub const IDEMPOTENCY_CONFLICT_MESSAGE: &str =
    "At least one input parameter differs from a previous call using the same client token";

/// Wall-clock milliseconds since the epoch, the unit every stored timestamp uses.
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// A stored millisecond timestamp as the modelled `timestamp` type.
pub fn date_time(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// A modelled `timestamp` as the millisecond unit every stored timestamp uses, so a
/// query-string bound and a stored `createdAt` compare in one unit.
pub fn millis_of(value: &::aws_smithy_types::DateTime) -> i64 {
    value
        .secs()
        .saturating_mul(1_000)
        .saturating_add(i64::from(value.subsec_nanos()) / 1_000_000)
}

/// The opaque `nextToken` a `@paginated` list in this service hands back: the place
/// `(createdAt, creation sequence)` in a newest-first order the next page starts at,
/// prefixed with the SCOPE that issued it ([`list_cursor_scope`]), so a cursor one listing
/// at one region's endpoint minted is never read as a position in any other order.
///
/// One encoder for every paginated list here, so the five cannot drift apart. The
/// components are joined by `:`, which neither a decimal millisecond (it may carry a
/// leading `-`) nor a sequence can contain, so the split in [`decode_list_cursor`] is
/// unambiguous whatever the stored instant is: a scope may carry `:` itself, and is matched
/// as a whole prefix, which leaves exactly the two numeric components after it. A client
/// must not read a cursor; the service only has to resume from it.
pub fn encode_list_cursor(scope: &str, created_at_millis: i64, sequence: u64) -> String {
    format!("{scope}:{created_at_millis}:{sequence}")
}

/// The place in a newest-first order a `nextToken` names.
///
/// `None` — refused as an invalid request — when the token is not a cursor this `scope`
/// issued, so a token cannot be carried from one listing to another's, nor from one
/// region's endpoint to another's ([`list_cursor_scope`]).
///
/// A token is one this `scope` issued only when it is byte for byte what
/// [`encode_list_cursor`] writes for the position it decodes to. Parsing a decimal is
/// laxer than writing one — `str::parse` also reads a leading `+`, leading zeros and a
/// signed zero, none of which the encoder can ever produce — so the decoded position is
/// re-encoded and compared with the token the client sent: every spelling but the
/// canonical one is a fabrication, and is refused rather than read as a page position.
pub fn decode_list_cursor(scope: &str, token: &str) -> Option<(i64, u64)> {
    let rest = token.strip_prefix(scope)?.strip_prefix(':')?;
    let mut parts = rest.split(':');
    let (Some(created_at_millis), Some(sequence), None) = (parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let position: (i64, u64) = (created_at_millis.parse().ok()?, sequence.parse().ok()?);
    (encode_list_cursor(scope, position.0, position.1) == token).then_some(position)
}

/// The scope [`encode_list_cursor`] tags a cursor with: the `listing` that minted it AND
/// the `region` of the endpoint that minted it.
///
/// A cursor is a POSITION in one newest-first order, not the identity of a row, so it names
/// a place only in the record set that order was computed over — and every record set paged
/// here is one region's, because a resource of this regional service exists only at the
/// endpoint that created it (see the module docs). A position computed over one region's
/// records therefore names a different place, or no place at all, in another's order, so the
/// region belongs in the scope exactly as the listing does: with it there,
/// [`decode_list_cursor`] refuses a token offered to an endpoint that did not issue it,
/// instead of resuming a listing it never positioned.
///
/// One builder for every scope below, so no listing's cursor can come to be read anywhere
/// but in the one order it names.
///
/// A scope may therefore contain the `:` the cursor's components are joined by, which costs
/// the decode nothing: [`decode_list_cursor`] matches the scope as a WHOLE prefix and then
/// requires exactly the two numeric components after it. Where a scope carries a parent
/// resource as well, that part is a `ResourceIdString` (`^[0-9a-z]+$`), so only the region
/// can hold a `:` and a scope is still the one string the listing it belongs to builds.
fn list_cursor_scope(listing: &str, region: &str) -> String {
    format!("{listing}:{region}")
}

/// The scope [`encode_list_cursor`] tags a `ListJobTemplates` cursor with.
pub fn list_job_templates_scope(region: &str) -> String {
    list_cursor_scope("jobtemplates", region)
}

/// The scope [`encode_list_cursor`] tags a `ListSecurityConfigurations` cursor with.
pub fn list_security_configurations_scope(region: &str) -> String {
    list_cursor_scope("securityconfigurations", region)
}

/// The scope [`encode_list_cursor`] tags a `ListVirtualClusters` cursor with.
pub fn list_virtual_clusters_scope(region: &str) -> String {
    list_cursor_scope("virtualclusters", region)
}

/// The scope [`encode_list_cursor`] tags a `ListManagedEndpoints` cursor with: one cluster's
/// endpoint listing at one region's endpoint, so a cursor is never read as a position in
/// another cluster's order, nor in the SAME cluster's job-run listing.
pub fn list_managed_endpoints_scope(region: &str, virtual_cluster_id: &str) -> String {
    format!(
        "{}:{virtual_cluster_id}",
        list_cursor_scope("endpoints", region)
    )
}

/// The scope [`encode_list_cursor`] tags a `ListJobRuns` cursor with: one cluster's job-run
/// listing at one region's endpoint, on the same terms as
/// [`list_managed_endpoints_scope`].
pub fn list_job_runs_scope(region: &str, virtual_cluster_id: &str) -> String {
    format!(
        "{}:{virtual_cluster_id}",
        list_cursor_scope("jobruns", region)
    )
}

/// The place one listed record occupies in the newest-first order every `@paginated` list
/// in this service reports: its stored `createdAt` in milliseconds, and the creation
/// sequence that breaks a tie between two records stored in the same millisecond, so the
/// order is total and a page boundary never depends on hash order.
///
/// Implemented by each record a list pages over, so [`page_of`] — not one copy per
/// handler — is the only place the order, the cursor scan and the page bounds are written.
pub trait ListPosition {
    fn list_position(&self) -> (i64, u64);
}

impl ListPosition for VirtualClusterRecord {
    fn list_position(&self) -> (i64, u64) {
        (self.created_at_millis, self.sequence)
    }
}

impl ListPosition for JobRunRecord {
    fn list_position(&self) -> (i64, u64) {
        (self.created_at_millis, self.sequence)
    }
}

impl ListPosition for JobTemplateRecord {
    fn list_position(&self) -> (i64, u64) {
        (self.created_at_millis, self.sequence)
    }
}

impl ListPosition for SecurityConfigurationRecord {
    fn list_position(&self) -> (i64, u64) {
        (self.created_at_millis, self.sequence)
    }
}

impl ListPosition for ManagedEndpointRecord {
    fn list_position(&self) -> (i64, u64) {
        (self.created_at_millis, self.sequence)
    }
}

/// Why a `@paginated` list cannot serve the page a request asked for. Both are invalid
/// requests: each list refuses them with its OWN modelled `ValidationException`, carrying
/// [`PageRefusal::message`], so the wording cannot differ between two listings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageRefusal {
    /// `maxResults` named a page of fewer than one item, carrying the value requested.
    MaxResultsBelowOne(i32),
    /// `nextToken` is not a cursor this listing's scope issued.
    UnknownNextToken,
}

impl PageRefusal {
    /// The `message` the refusing `ValidationException` carries.
    pub fn message(&self) -> String {
        match self {
            Self::MaxResultsBelowOne(limit) => {
                format!("maxResults must be at least 1, but {limit} was requested")
            }
            Self::UnknownNextToken => "nextToken is not a valid pagination token".to_string(),
        }
    }
}

/// One page of a `@paginated` list: the records to report, in the order they go on the
/// wire, and the `nextToken` that resumes after them.
#[derive(Debug)]
pub struct ListPage<'a, R> {
    pub records: Vec<&'a R>,
    pub next_token: Option<String>,
}

/// The page-bounds rule every `@paginated` list in this service follows, over the records
/// a listing's filters already kept:
///
/// * `maxResults` is the page size, and a page of fewer than one item is not a page, so it
///   is refused rather than silently read as "no limit";
/// * the order is newest first, by [`ListPosition`];
/// * `nextToken` is the POSITION in that order the next page starts at, not the identity of
///   a row, so a record whose state changed between pages cannot invalidate a token the
///   client already holds: resuming reports everything ordered at or after the cursor, with
///   no repeats and no gaps. A token no [`decode_list_cursor`] of this `scope` accepts is
///   refused, so a cursor minted by another listing — or by another region's endpoint,
///   whose records are a different set in a different order — is never read as a position
///   in this one ([`list_cursor_scope`]);
/// * a `nextToken` comes back only while records remain after the page.
///
/// One rule for every paginated list here, beside the one cursor encoder, so the two halves
/// of the pagination contract cannot drift apart.
pub fn page_of<'a, R: ListPosition>(
    scope: &str,
    records: impl IntoIterator<Item = &'a R>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<ListPage<'a, R>, PageRefusal> {
    let page_size = match max_results {
        Some(limit) if limit < 1 => return Err(PageRefusal::MaxResultsBelowOne(limit)),
        Some(limit) => Some(usize::try_from(limit).unwrap_or(usize::MAX)),
        None => None,
    };

    let mut ordered: Vec<&'a R> = records.into_iter().collect();
    ordered.sort_by_key(|record| std::cmp::Reverse(record.list_position()));

    let start = match next_token {
        None => 0,
        Some(token) => {
            let cursor = decode_list_cursor(scope, token).ok_or(PageRefusal::UnknownNextToken)?;
            ordered
                .iter()
                .position(|record| record.list_position() <= cursor)
                .unwrap_or(ordered.len())
        }
    };

    let end = page_size.map_or(ordered.len(), |size| {
        start.saturating_add(size).min(ordered.len())
    });
    let next_token = ordered.get(end).map(|record| {
        let (created_at_millis, sequence) = record.list_position();
        encode_list_cursor(scope, created_at_millis, sequence)
    });

    Ok(ListPage {
        records: ordered[start..end].to_vec(),
        next_token,
    })
}

/// The `createdBefore` / `createdAfter` rule every `@paginated` list in this service
/// applies to a record's stored `createdAt`: both bound it STRICTLY, so the two together
/// name an open interval and neither ever keeps the record created exactly at a bound. A
/// bound the request omitted constrains nothing.
///
/// One rule for every paginated list here, beside the one page rule ([`page_of`]), so the
/// halves of a listing's contract cannot drift apart: a bound is exclusive for every
/// listing or for none.
pub fn within_created_bounds(
    created_at_millis: i64,
    created_before: Option<&::aws_smithy_types::DateTime>,
    created_after: Option<&::aws_smithy_types::DateTime>,
) -> bool {
    created_before.is_none_or(|bound| created_at_millis < millis_of(bound))
        && created_after.is_none_or(|bound| created_at_millis > millis_of(bound))
}

/// A collection a request can carry in a structure member — a map or a list — seen only
/// as "does it carry an entry", which is all [`supplied`] has to ask of it.
pub trait RequestCollection {
    /// Whether this collection carries no entry at all.
    fn carries_no_entry(&self) -> bool;
}

impl<V> RequestCollection for HashMap<String, V> {
    fn carries_no_entry(&self) -> bool {
        self.is_empty()
    }
}

impl<T> RequestCollection for [T] {
    fn carries_no_entry(&self) -> bool {
        self.is_empty()
    }
}

/// What a collection sitting in a request MEMBER actually supplies: `Some` only where the
/// request carried an entry in it.
///
/// A collection a request omitted and one it sent EMPTY are the same request — that is
/// what [`request_fingerprint`] digests alike — so a handler cannot read a meaning into
/// the mere PRESENCE of such a member: one carrying no entry supplies no value, names no
/// filter and asks for nothing, so the request has to be answered exactly as the one that
/// left the member out is. Every branch in this service that asks whether a collection was
/// supplied asks it HERE, so the two spellings of one request cannot come to mean the same
/// thing in one operation and different things in another.
pub fn supplied<C: RequestCollection + ?Sized>(collection: Option<&C>) -> Option<&C> {
    collection.filter(|collection| !collection.carries_no_entry())
}

/// The `states` rule every list in this service that carries one applies to a record's
/// stored state: the record is kept when its state is one of the states the request
/// listed. A filter the request omitted constrains nothing, and nor does one that lists no
/// state at all — an empty list names no state to keep out ([`supplied`]).
///
/// One rule for every listing that filters by state, beside [`within_created_bounds`], so
/// an empty list cannot come to mean "keep nothing" in one listing and "keep everything"
/// in another.
pub fn within_listed_states(state: &str, states: Option<&[String]>) -> bool {
    supplied(states).is_none_or(|states| states.iter().any(|listed| listed == state))
}

/// `arn:aws:emr-containers:{region}:{account}:/virtualclusters/{id}` — the
/// `VirtualClusterArn` shape's pattern.
pub fn virtual_cluster_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("/virtualclusters/{id}"))
}

/// `arn:aws:emr-containers:{region}:{account}:/virtualclusters/{vc}/jobruns/{id}` — the
/// `JobArn` shape's pattern.
pub fn job_run_arn(region: &str, virtual_cluster_id: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("/virtualclusters/{virtual_cluster_id}/jobruns/{id}"),
    )
}

/// `arn:aws:emr-containers:{region}:{account}:/jobtemplates/{id}` — the
/// `JobTemplateArn` shape's pattern.
pub fn job_template_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("/jobtemplates/{id}"))
}

/// `arn:aws:emr-containers:{region}:{account}:/securityconfigurations/{id}` — the
/// `SecurityConfigurationArn` shape's pattern.
pub fn security_configuration_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("/securityconfigurations/{id}"),
    )
}

/// `arn:aws:emr-containers:{region}:{account}:/virtualclusters/{vc}/endpoints/{id}` — the
/// `EndpointArn` shape's pattern.
pub fn managed_endpoint_arn(region: &str, virtual_cluster_id: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("/virtualclusters/{virtual_cluster_id}/endpoints/{id}"),
    )
}

/// How many characters a refusal of this service carries in its `message`.
///
/// Every error the model declares here targets `String1024` (len 1..1024) for that member,
/// whichever operation raises it — `ResourceNotFoundException`, `ValidationException`,
/// `InternalServerException` and `RequestThrottledException` alike (smithy_query: shape
/// `ResourceNotFoundException`) — so ONE bound holds for every message this service words.
pub const MESSAGE_MAX_CHARACTERS: usize = 1024;

/// `message`, held to the characters the member that carries it can hold.
///
/// A refusal is worded from the request, and a request may carry a single value longer
/// than the whole message bound — `GetManagedEndpointSessionCredentials` names its
/// resources with `String2048` identifiers, a job template parameter takes a `String1024`
/// value — so a message built around one is clipped here rather than serialized at a
/// length the shape it is declared as cannot hold. Clipping counts CHARACTERS, the unit
/// every modelled `@length` is in, so a multi-byte value is never cut mid-character.
pub fn bounded_message(message: String) -> String {
    if message.chars().count() <= MESSAGE_MAX_CHARACTERS {
        return message;
    }
    message.chars().take(MESSAGE_MAX_CHARACTERS).collect()
}

/// A refusal worded AROUND the value it reports, held to that same bound: the wording
/// stays whole and the VALUE gives up the characters there is no room for, so a message
/// about a very long value still says what it is about rather than ending mid-sentence.
pub fn message_about(prefix: &str, value: &str, suffix: &str) -> String {
    let wording = prefix.chars().count() + suffix.chars().count();
    let mut message = String::new();
    message.push_str(prefix);
    message.extend(
        value
            .chars()
            .take(MESSAGE_MAX_CHARACTERS.saturating_sub(wording)),
    );
    message.push_str(suffix);
    // The wording on its own could be longer than the bound, which no clipping of the
    // value it surrounds can bring back inside it.
    bounded_message(message)
}

/// A resource kind this service refuses a request for when the request names one that is
/// not there.
///
/// The wording of such a refusal belongs to the ERROR CLASS that carries it, not to the
/// resource:
///
/// * a `ResourceNotFoundException` — the class an operation that MODELS a not-found error
///   reports a missing resource in — is worded from the identifier the request named the
///   resource by, in ONE form: `<Resource type> <identifier> doesn't exist.`
///   ([`ResourceKind::not_found_message`]). The resource type is spelled the way the
///   service spells it in a message, which is NOT derivable from the shape name, so each
///   kind carries its own spelling here.
/// * a `ValidationException` — the only client error an operation that models no not-found
///   error has to report it in — is worded as a SHORT FIXED sentence carrying no
///   identifier at all ([`ResourceKind::validation_not_found_message`]).
///
/// Every operation that reports a resource missing takes its wording from this type —
/// beside [`PageRefusal::message`], the other wording this service shares — so the message
/// cannot differ between two operations reporting the same kind of resource missing in the
/// same error class, nor drift from the forms above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    VirtualCluster,
    JobRun,
    ManagedEndpoint,
    JobTemplate,
    SecurityConfiguration,
}

impl ResourceKind {
    /// The resource type as this service names it in a message.
    pub fn resource_type(self) -> &'static str {
        match self {
            Self::VirtualCluster => "Virtual cluster",
            Self::JobRun => "Job run",
            // The model calls the resource a managed endpoint HOSTS an `Endpoint` (its
            // shape, its `endpoint` output member and its `endpointIdentifier` input).
            Self::ManagedEndpoint => "Endpoint",
            Self::JobTemplate => "Job Template",
            Self::SecurityConfiguration => "Security Configuration",
        }
    }

    /// The `message` a refusal carries for an `identifier` that names no such resource of
    /// this region: `<Resource type> <identifier> doesn't exist.`, with the identifier as
    /// the request spelled it, held to what the member carrying the message can hold
    /// ([`message_about`]) — an identifier a request names a resource by is itself allowed
    /// to be longer than the message.
    pub fn not_found_message(self, identifier: &str) -> String {
        message_about(
            &format!("{} ", self.resource_type()),
            identifier,
            " doesn't exist.",
        )
    }

    /// The `message` a refusal carries when the request names no such resource of this
    /// region and the operation has to report it through its modelled `ValidationException`
    /// because it models no not-found error at all (`DeleteJobTemplate`,
    /// `DeleteSecurityConfiguration`, `DeleteVirtualCluster`, `DeleteManagedEndpoint`,
    /// `CancelJobRun`, `ListJobRuns`, `ListManagedEndpoints` — smithy_query: digest).
    ///
    /// That class is worded as a short sentence of FIXED text, with no identifier
    /// interpolated into it, which is a different sentence from the not-found wording a
    /// read of the same resource answers with ([`not_found_message`](Self::not_found_message)).
    /// The sentences are not one template either — a job template's says the template does
    /// not exist, a security configuration's says the configuration was not found — so each
    /// kind carries the sentence this service words for it. A kind for which only the
    /// not-found wording of the class above is known keeps it, rather than a short sentence
    /// invented here: a message this service never emits would be as wrong as the one it
    /// replaces.
    pub fn validation_not_found_message(self, identifier: &str) -> String {
        match self {
            Self::JobTemplate => "Template does not exist".to_string(),
            Self::SecurityConfiguration => "Security configuration not found".to_string(),
            Self::VirtualCluster | Self::JobRun | Self::ManagedEndpoint => {
                self.not_found_message(identifier)
            }
        }
    }
}

/// The `message` a refusal carries when the request names a virtual cluster that IS
/// stored but whose `VirtualClusterState` (RUNNING, TERMINATING, TERMINATED, ARRESTED —
/// smithy_query: shape `VirtualClusterState`) forbids the call:
/// `Virtual cluster state <state> is not valid to create resource <Operation>.`
///
/// The variable parts are the cluster's CURRENT state and the name of the operation being
/// refused; the cluster's own id is not part of this wording, which is what separates it
/// from a not-found refusal ([`ResourceKind::not_found_message`]) — a state refusal says
/// which state blocked the call, not which resource was asked for.
///
/// Every operation of this service that refuses a call because the cluster's state leaves
/// it nothing to do takes its wording from here, so the two cannot drift apart.
pub fn virtual_cluster_state_refusal_message(state: &str, operation: &str) -> String {
    bounded_message(format!(
        "Virtual cluster state {state} is not valid to create resource {operation}."
    ))
}

/// The `message` `CancelJobRun` carries for a run whose `JobRunState` is one a
/// cancellation can no longer reach ([`job_state_is_terminal`]): `Job run <id> is not in a
/// cancellable state`.
///
/// The only variable part is the job run id the request named, held to what the member
/// carrying the message can hold ([`message_about`]); the run's current state is NOT
/// interpolated — a state refusal of a job run reports that the run is past cancelling,
/// not which terminal state it reached (a reader that wants the state calls
/// `DescribeJobRun`).
pub fn job_run_not_cancellable_message(identifier: &str) -> String {
    message_about(
        "Job run ",
        identifier,
        " is not in a cancellable state",
    )
}

/// The resource a tagging call's `resourceArn` (an `RsiArn`) names.
///
/// `RsiArn`'s pattern (`^arn:(aws[a-zA-Z0-9-]*):emr-containers:.+:(\d{12}):/virtualclusters/.+$`)
/// admits the virtual cluster itself and the resources that live UNDER one, which is
/// exactly the set of ARN shapes the model spells with a `/virtualclusters/` path:
/// `VirtualClusterArn`, `JobArn` and `EndpointArn`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    VirtualCluster {
        id: String,
    },
    JobRun {
        virtual_cluster_id: String,
        id: String,
    },
    ManagedEndpoint {
        virtual_cluster_id: String,
        id: String,
    },
}

/// Whether an ARN path segment is an id segment this service's resource ARNs admit:
/// `VirtualClusterArn`, `JobArn` and `EndpointArn` all spell theirs `[0-9a-zA-Z]+`.
fn is_arn_id_segment(segment: &str) -> bool {
    !segment.is_empty() && segment.chars().all(|c| c.is_ascii_alphanumeric())
}

impl TaggedResource {
    /// The ARN this service mints for this resource at `region`'s endpoint — the one the
    /// resource's own record carries, built by the same builders that stamped it
    /// ([`virtual_cluster_arn`], [`job_run_arn`], [`managed_endpoint_arn`]).
    pub fn arn(&self, region: &str) -> String {
        match self {
            Self::VirtualCluster { id } => virtual_cluster_arn(region, id),
            Self::JobRun {
                virtual_cluster_id,
                id,
            } => job_run_arn(region, virtual_cluster_id, id),
            Self::ManagedEndpoint {
                virtual_cluster_id,
                id,
            } => managed_endpoint_arn(region, virtual_cluster_id, id),
        }
    }
}

/// The resource a tagging call's `resourceArn` names at the endpoint the request reached,
/// for a request signed for `region`.
///
/// A resource reference is one this endpoint can act on only when it is an ARN THIS service
/// mints here: the `aws` partition, the `emr-containers` service, this account, the region
/// the request is signed for, and a resource path of one of the three forms this service's
/// resource ARNs take ([`TaggedResource::arn`]). An ARN of another partition, another
/// account or another region names a resource some other endpoint holds, which this one
/// could never have minted and can never resolve — so it is an invalid resource reference
/// rather than a resource that happens to be missing, exactly as a path naming no resource
/// form this service models is ([`parse_tagged_resource_arn`]).
///
/// `None` is one half of the invalid-reference class the three tagging operations refuse
/// with [`INVALID_RESOURCE_ARN_MESSAGE`]; `Some` leaves the other half to the store, whether
/// the resource this reference names is stored at all.
pub fn endpoint_tagged_resource(region: &str, arn: &str) -> Option<TaggedResource> {
    let resource = parse_tagged_resource_arn(arn)?;
    (resource.arn(region) == arn).then_some(resource)
}

/// The resource an `RsiArn` names, read out of the ARN's resource path.
///
/// `RsiArn`'s own pattern — refused at the boundary before any handler runs — already
/// fixes the ARN's envelope (the `arn:` prefix, an `aws*` partition, the
/// `emr-containers` service, a 12-digit account and a `/virtualclusters/` resource
/// path), so what is left to read is which resource that path names.
///
/// `None` when the path is none of the three forms the model declares: an ARN that could
/// not be ANY resource this service models is an invalid `resourceArn`, which is a
/// different answer from a well-formed ARN whose resource is simply not there.
pub fn parse_tagged_resource_arn(arn: &str) -> Option<TaggedResource> {
    // `arn:{partition}:{service}:{region}:{account}:{resource}`: the resource is
    // everything after the fifth `:`, so it is read with a bounded split rather than by
    // taking the last field — a region or a resource path may itself contain a `:`.
    let resource = arn.splitn(6, ':').nth(5)?;
    let path = resource.strip_prefix("/virtualclusters/")?;
    let mut segments = path.split('/');
    let virtual_cluster_id = segments.next().filter(|segment| is_arn_id_segment(segment))?;
    match (segments.next(), segments.next(), segments.next()) {
        // `/virtualclusters/{id}` — the cluster itself.
        (None, _, _) => Some(TaggedResource::VirtualCluster {
            id: virtual_cluster_id.to_string(),
        }),
        (Some("jobruns"), Some(id), None) if is_arn_id_segment(id) => Some(TaggedResource::JobRun {
            virtual_cluster_id: virtual_cluster_id.to_string(),
            id: id.to_string(),
        }),
        (Some("endpoints"), Some(id), None) if is_arn_id_segment(id) => {
            Some(TaggedResource::ManagedEndpoint {
                virtual_cluster_id: virtual_cluster_id.to_string(),
                id: id.to_string(),
            })
        }
        _ => None,
    }
}

/// The input member a tagging call names the resource by.
///
/// `TagResource`, `UntagResource` and `ListTagsForResource` are the only operations of this
/// service whose input declares one (smithy_query: digest), and on all three it is a
/// top-level `@httpLabel` string — so this is also the whole dotted path a boundary
/// violation about it reports.
pub const RESOURCE_ARN_MEMBER: &str = "resourceArn";

/// The `message` a refusal carries when a tagging call's `resourceArn` does not resolve to
/// a resource this endpoint holds — whichever way it fails to: it breaks the `RsiArn`
/// length or pattern the model declares (refused at the boundary, before any handler runs),
/// or it is not an ARN this service mints here, because its partition, account or region is
/// another endpoint's or its resource path is none of the forms this service's resource ARNs
/// take ([`endpoint_tagged_resource`]), or it is one this endpoint does mint and the
/// resource it names is not stored.
///
/// The three `/tags/{resourceArn}` operations report that whole class as ONE refusal: the
/// resource reference is what they validate, and a reference that resolves to nothing is an
/// unusable reference rather than a resource the caller may wait for. That is why it is the
/// class's own sentence of FIXED text, with nothing of the request interpolated into it, and
/// not the identifier-bearing wording an operation that names its resource by ID answers
/// with ([`ResourceKind::not_found_message`]).
pub const INVALID_RESOURCE_ARN_MESSAGE: &str = "Invalid input resource arn";

/// The error class an unusable `resourceArn` is refused in: the modelled
/// `ValidationException` all three tagging operations declare (smithy_query: operations
/// `TagResource`, `UntagResource`, `ListTagsForResource`), at HTTP 400.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// How this service words a refusal the framework raised before any handler ran, for the
/// one member it words itself: a tagging call's `resourceArn`.
///
/// `RsiArn`'s `@length` 60..500 and its `@pattern` are enforced at the boundary, so a
/// `resourceArn` that is not even shaped like one of this service's resource ARNs never
/// reaches a handler — and the framework's own envelope would report it as a generic
/// constraint violation naming the member, the bound and the value. This service reports an
/// unusable resource reference as [`INVALID_RESOURCE_ARN_MESSAGE`] instead, so the boundary
/// refusal and the handlers' own ([`endpoint_tagged_resource`]) are the same sentence in the
/// same error class however the reference failed.
///
/// `None` for every other refusal, which keeps the framework's wording.
pub fn resource_arn_refusal(
    refusal: &::smithy_server_runtime::Refusal,
) -> Option<::smithy_server_runtime::Refused> {
    let ::smithy_server_runtime::Refusal::Invalid(violations) = refusal else {
        return None;
    };
    // The resource the call is about is read before anything else about the request, so a
    // refusal that names the `resourceArn` at all is a refusal of the reference.
    violations
        .iter()
        .any(|violation| violation.path == RESOURCE_ARN_MEMBER)
        .then(|| ::smithy_server_runtime::Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: INVALID_RESOURCE_ARN_MESSAGE.to_string(),
        })
}

/// Whether a request-supplied identifier is written as an ARN rather than as a bare id.
///
/// An `arn:` prefix is what tells the two forms apart: `ResourceIdString` is
/// `^[0-9a-z]+$`, so no id of this service can begin with one.
fn looks_like_an_arn(identifier: &str) -> bool {
    identifier.starts_with("arn:")
}

/// A fresh session token for connecting a client to one managed endpoint, for
/// `GetManagedEndpointSessionCredentials` ("Generate a session token to connect to a
/// managed endpoint").
///
/// The token authenticates to the ENDPOINT's gateway, not to this emulator, so it must
/// never be accepted as a SigV4 credential — but it is still a secret handed to a caller,
/// and nothing in a service may construct a secret itself. It is therefore minted by the
/// one component that generates secrets, the emulator's credential store, and then removed
/// from that store again: the caller gets the secret, the emulator keeps no credential it
/// would accept, and nothing accumulates per call.
///
/// `None` when the store minted no session-token secret, which leaves the operation to
/// refuse rather than report a token it made up.
pub fn mint_session_token(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let token = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .map(str::to_string);
    // The session token is not a credential of this emulator; see above.
    let _ = store.delete(&minted.id);
    token
}

/// The region to build an ARN in and to resolve a resource in for this request: the
/// region of its SigV4 credential scope, so an ARN names the region the caller signed
/// for and an id resolves only among that region's resources.
pub fn request_region(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of the IAM principal every caller of this emulator authenticates as, and so
/// the value of a "who created this" member (`JobTemplate$createdBy`,
/// `SecurityConfiguration$createdBy`, `JobRun$createdBy`: "The user who created …").
///
/// `RequestIdentityUserArn`'s pattern is
/// `^arn:(aws[a-zA-Z0-9-]*):(iam|sts)::(\d{12})?:[\w/+=,.@-]+$`, so the ARN carries no
/// region — `::engine::emulator_arn` with an empty region produces exactly that form, and
/// the result is 44 characters, inside the shape's 20..2048 length.
///
/// It names the one IAM user this emulator publishes — the very principal its
/// `sts:GetCallerIdentity` answers, built the same way — so the principal a response
/// reports is one the caller can look up. Deliberately NOT derived from the request's
/// access key id: a member AWS fills from the AUTHENTICATED identity has to name an
/// identity the identity service knows, and a per-request name spelled out of an access
/// key id is not one.
pub fn caller_identity_arn() -> String {
    ::engine::emulator_arn("iam", "", "user/emulator")
}

/// A status change the clock owes a resource: at [`due_at`](Self::due_at) its state
/// becomes [`next_state`](Self::next_state).
///
/// The monotonic `Instant` decides when it comes due; the wall-clock millisecond is what
/// a `finishedAt` reports, so the reported instant is the one the transition was owed at
/// rather than whenever a caller next looked.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    pub due_at: Instant,
    pub due_at_millis: i64,
    pub next_state: &'static str,
}

impl PendingTransition {
    /// A transition to `next_state`, due [`TRANSITION_SPAN`] from now.
    pub fn due(next_state: &'static str) -> Self {
        Self::due_in(next_state, TRANSITION_SPAN)
    }

    /// A transition to `next_state`, due `span` from now — for a change the service owes
    /// on a span of its own rather than on the status-step span every resource takes.
    pub fn due_in(next_state: &'static str, span: Duration) -> Self {
        Self {
            due_at: Instant::now()
                .checked_add(span)
                .unwrap_or_else(Instant::now),
            due_at_millis: now_millis() + i64::try_from(span.as_millis()).unwrap_or(0),
            next_state,
        }
    }
}

/// The Kubernetes namespace a `ContainerProvider` names, when it names one.
///
/// `ContainerProvider$info` targets the `ContainerInfo` union, whose only member is
/// `eksInfo`, and `EksInfo$namespace` is itself optional, so a provider may carry no
/// namespace at all — and then names none to map a virtual cluster onto.
pub fn provider_namespace(provider: &ContainerProvider) -> Option<&str> {
    match provider.info.as_ref()? {
        ContainerInfo::eksInfo(eks_info) => eks_info.namespace.as_deref(),
    }
}

/// One virtual cluster, as `CreateVirtualCluster` persisted it.
#[derive(Debug, Clone)]
pub struct VirtualClusterRecord {
    pub id: String,
    /// The region whose endpoint minted this cluster — the one its [`arn`](Self::arn)
    /// carries. Every read and every mutate resolves an id only among the records of the
    /// region the request is signed for, so a resource exists exactly where it was
    /// created.
    pub region: String,
    pub name: String,
    pub arn: String,
    pub state: String,
    pub container_provider: ContainerProvider,
    pub created_at_millis: i64,
    pub tags: HashMap<String, String>,
    pub security_configuration_id: Option<String>,
    pub session_enabled: Option<bool>,
    pub scheduler_configuration: Option<SchedulerConfiguration>,
    pub client_token: String,
    /// [`request_fingerprint`] of the `CreateVirtualCluster` request that minted this
    /// cluster, so a later call carrying the same `clientToken` is answered as a replay
    /// only when it is the same request.
    pub request_fingerprint: String,
    /// The `UpdateVirtualCluster` `clientToken`s this cluster has already applied, most
    /// recent last, at most [`MAX_RETAINED_UPDATE_TOKENS`] of them.
    pub update_client_tokens: Vec<String>,
    /// Creation sequence, so clusters minted within one millisecond still have a total
    /// order for `ListVirtualClusters` to page through.
    pub sequence: u64,
    pub pending: Option<PendingTransition>,
}

impl VirtualClusterRecord {
    /// Whether an `UpdateVirtualCluster` carrying `client_token` has already been applied
    /// to this cluster, inside the window [`MAX_RETAINED_UPDATE_TOKENS`] keeps.
    pub fn has_applied_update_token(&self, client_token: &str) -> bool {
        self.update_client_tokens
            .iter()
            .any(|applied| applied == client_token)
    }

    /// Remember that an update carrying `client_token` has been applied, dropping the
    /// oldest remembered token once the window is full.
    fn remember_update_token(&mut self, client_token: &str) {
        if self.has_applied_update_token(client_token) {
            return;
        }
        self.update_client_tokens.push(client_token.to_string());
        if self.update_client_tokens.len() > MAX_RETAINED_UPDATE_TOKENS {
            self.update_client_tokens.remove(0);
        }
    }
    /// Materialise this cluster's transition if the clock has made it due.
    fn settle(&mut self, now: Instant) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if now < pending.due_at {
            self.pending = Some(pending);
            return;
        }
        self.state = pending.next_state.to_string();
    }

    /// The modelled `VirtualCluster` this record represents.
    ///
    /// `schedulerStatus` is not a property of this record — it is the live count of the
    /// cluster's job runs — so it is counted out of the store by
    /// [`ComAmazonawsEmrcontainersAwschicagowebserviceState::cluster_scheduler_status`]
    /// and handed in, which is why this is not a parameterless rendering.
    fn to_virtual_cluster(&self, scheduler_status: SchedulerStatus) -> VirtualCluster {
        VirtualCluster {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            state: Some(self.state.clone()),
            container_provider: Some(self.container_provider.clone()),
            created_at: Some(date_time(self.created_at_millis)),
            tags: Some(self.tags.clone()),
            security_configuration_id: self.security_configuration_id.clone(),
            session_enabled: self.session_enabled,
            scheduler_configuration: self.scheduler_configuration.clone(),
            scheduler_status: Some(scheduler_status),
        }
    }
}

/// One job run, as `StartJobRun` persisted it.
#[derive(Debug, Clone)]
pub struct JobRunRecord {
    pub id: String,
    /// The region whose endpoint accepted this run, on the terms
    /// [`VirtualClusterRecord::region`] states. It is the region of the cluster the run
    /// was submitted to: a run can only be started through the endpoint that cluster
    /// exists at.
    pub region: String,
    pub name: Option<String>,
    pub virtual_cluster_id: String,
    pub arn: String,
    pub state: String,
    pub client_token: String,
    /// [`request_fingerprint`] of the `StartJobRun` request that submitted this run, so a
    /// later call carrying the same `clientToken` is answered as a replay only when it is
    /// the same submission.
    pub request_fingerprint: String,
    pub execution_role_arn: Option<String>,
    pub release_label: Option<String>,
    pub job_driver: Option<JobDriver>,
    pub configuration_overrides: Option<ConfigurationOverrides>,
    pub retry_policy_configuration: Option<RetryPolicyConfiguration>,
    pub created_at_millis: i64,
    pub finished_at_millis: Option<i64>,
    pub state_details: Option<String>,
    /// Why this run failed, a value of the modelled `FailureReason` enum, recorded when
    /// the run settles into `FAILED` and absent for every other state.
    pub failure_reason: Option<String>,
    /// How many attempts this service has made on the run's driver — what
    /// `RetryPolicyExecution$currentAttemptCount` ("the current number of attempts made
    /// on the driver of the job") reports.
    ///
    /// `0` while the run is still waiting for a slot: a waiting run has had no attempt
    /// made on it. The admission that starts the run counts one, and the driver is not
    /// re-driven past that — nothing executes it, so there is no further attempt to count.
    /// A run that configured no retry policy reports no execution at all; the count is
    /// still kept, so the one rule serves both.
    pub attempt_count: i32,
    pub tags: HashMap<String, String>,
    /// The principal that submitted this run ([`caller_identity_arn`]), recorded at
    /// `StartJobRun` so every later read attributes the run to the same identity.
    pub created_by: String,
    /// Creation sequence, so job runs minted within one millisecond still have a total
    /// order for `ListJobRuns` to page through.
    pub sequence: u64,
    pub pending: Option<PendingTransition>,
}

/// The transition a run's admission owes: the END of the run's driver.
///
/// A scheduled transition names the state it lands in, and the state the end of a driver
/// lands in is not known until it falls due ([`job_run_outcome`]) — so what is scheduled
/// is the end of a run whose compute carries it through, and the other end is reached by
/// materialising this one against a compute that has gone.
const JOB_DRIVER_END: &str = JOB_COMPLETED;

impl JobRunRecord {
    /// Materialise this job run's transition if the clock has made it due.
    ///
    /// Every state a run is moved into brings the detail of that state with it
    /// ([`job_run_state_details`]), and a transition into a terminal state is what sets
    /// `finishedAt`, which reports the instant the transition was owed at.
    ///
    /// The admission that starts a run is also what makes its OUTCOME due: a run AWS
    /// hands back in a non-terminal state reaches a terminal one, so once this run is
    /// `RUNNING` the clock owes it the end of its driver [`JOB_OUTCOME_SPAN`] later.
    ///
    /// WHICH end it reaches is decided when that end falls due rather than when it was
    /// scheduled, out of `compute_available` — whether the virtual cluster this run was
    /// admitted to is still available to finish it ([`job_run_outcome`]) — so a run that
    /// outlives its compute ends differently from one whose compute carried it through.
    fn settle(&mut self, now: Instant, compute_available: bool) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if now < pending.due_at {
            self.pending = Some(pending);
            return;
        }
        let mut failure_reason = None;
        let next_state = if pending.next_state == JOB_DRIVER_END {
            let (outcome, reason) = job_run_outcome(compute_available);
            failure_reason = reason;
            outcome
        } else {
            pending.next_state
        };
        self.state = next_state.to_string();
        self.state_details = job_run_state_details(next_state).map(str::to_string);
        if next_state == JOB_RUNNING {
            // The admission starts an attempt on the driver, and owes its outcome.
            self.attempt_count = self.attempt_count.saturating_add(1);
            self.pending = Some(PendingTransition::due_in(JOB_DRIVER_END, JOB_OUTCOME_SPAN));
        }
        if job_state_is_terminal(next_state) {
            self.finished_at_millis = Some(pending.due_at_millis);
            self.failure_reason = failure_reason.map(str::to_string);
        }
    }

    /// How the retry policy this run carries has been executed, as
    /// `JobRun$retryPolicyExecution` reports it: the attempts made on its driver.
    ///
    /// Absent for a run submitted without a `retryPolicyConfiguration` — the member is
    /// "the current status of the retry policy executed on the job", and a run that
    /// configured no retry policy has none to report a status for.
    fn retry_policy_execution(&self) -> Option<RetryPolicyExecution> {
        self.retry_policy_configuration
            .as_ref()
            .map(|_| RetryPolicyExecution {
                current_attempt_count: self.attempt_count,
            })
    }

    /// The modelled `JobRun` this record represents.
    pub fn to_job_run(&self) -> JobRun {
        JobRun {
            id: Some(self.id.clone()),
            name: self.name.clone(),
            virtual_cluster_id: Some(self.virtual_cluster_id.clone()),
            arn: Some(self.arn.clone()),
            state: Some(self.state.clone()),
            client_token: Some(self.client_token.clone()),
            execution_role_arn: self.execution_role_arn.clone(),
            release_label: self.release_label.clone(),
            configuration_overrides: self.configuration_overrides.clone(),
            job_driver: self.job_driver.clone(),
            created_at: Some(date_time(self.created_at_millis)),
            created_by: Some(self.created_by.clone()),
            finished_at: self.finished_at_millis.map(date_time),
            state_details: self.state_details.clone(),
            failure_reason: self.failure_reason.clone(),
            tags: Some(self.tags.clone()),
            retry_policy_configuration: self.retry_policy_configuration.clone(),
            retry_policy_execution: self.retry_policy_execution(),
        }
    }
}

/// Append one component of a fingerprint rendering, behind its own byte length.
///
/// The length prefix is what keeps the rendering UNAMBIGUOUS: without it a value could
/// run into its neighbour (`"a" + "bc"` and `"ab" + "c"` render alike) and two different
/// requests would fingerprint the same, so a changed parameter could be answered as a
/// replay.
fn push_part(rendered: &mut String, part: &str) {
    rendered.push_str(&part.len().to_string());
    rendered.push(':');
    rendered.push_str(part);
    rendered.push(';');
}

/// The one-character tag a rendered value carries ahead of itself, so a member a second
/// request spells with a value of another KIND cannot render like the value it replaced.
const KIND_STRING: char = 's';
const KIND_NUMBER: char = 'n';
const KIND_BOOLEAN: char = 'b';
const KIND_TIMESTAMP: char = 't';
const KIND_BLOB: char = 'x';
const KIND_STRUCTURE: char = 'r';
const KIND_LIST: char = 'l';
const KIND_MAP: char = 'm';
const KIND_DOCUMENT: char = 'd';
const KIND_NULL: char = 'z';

/// One level of a [`RequestRendering`].
enum Frame {
    /// Children render in the order they were written: a structure's members, or a
    /// list's elements, whose order is part of the list's value.
    Ordered {
        rendered: String,
        /// Whether these children are a structure's MEMBERS. Only a member may be left
        /// unwritten, so only a member position can read an omitted value back out of a
        /// rendering that does not carry it; dropping a list element or a map value
        /// instead would renumber the list or leave its key without a value.
        members: bool,
    },
    /// A map's entries, held until the whole map is rendered so they can be put in key
    /// order rather than in hash iteration order. The walk writes an entry as its key
    /// followed by its value, so a write arriving while no key is held IS the next key.
    Keyed {
        entries: Vec<(String, String)>,
        pending_key: Option<String>,
    },
}

impl Frame {
    /// A frame for a structure's members.
    fn members() -> Self {
        Frame::Ordered {
            rendered: String::new(),
            members: true,
        }
    }

    /// A frame for a list's elements.
    fn elements() -> Self {
        Frame::Ordered {
            rendered: String::new(),
            members: false,
        }
    }

    /// A frame for a map's entries.
    fn entries() -> Self {
        Frame::Keyed {
            entries: Vec::new(),
            pending_key: None,
        }
    }

    /// This frame's finished rendering.
    fn into_rendering(self) -> String {
        match self {
            Frame::Ordered { rendered, .. } => rendered,
            Frame::Keyed { mut entries, .. } => {
                entries.sort();
                let mut rendered = String::new();
                for (key, value) in entries {
                    push_part(&mut rendered, &key);
                    push_part(&mut rendered, &value);
                }
                rendered
            }
        }
    }
}

/// A canonical rendering of a request, built by walking the request value itself.
///
/// Every generated input struct implements
/// [`SerializableStruct`](aws_smithy_schema::serde::SerializableStruct), whose
/// `serialize_members` threads each member's `&Schema` through every write — the same
/// visitor the boundary's constraint walk is driven by. So this is ONE rendering good for
/// every member of every input shape, rather than a per-operation list of members that a
/// model change could leave behind.
///
/// Two renderings are equal exactly when the two requests carried the same members with
/// the same values:
///
/// * every component is written behind its own byte length ([`push_part`]), so no value
///   can run into its neighbour;
/// * a structure member renders behind its member name, so two requests that fill
///   different members of one shape differ even where the value is the same;
/// * a list renders in request order and a map in key order, so one request never
///   renders two ways;
/// * a collection the request omitted and one it sent EMPTY render alike, because a
///   handler persists the same record for both. A structure is not collapsed that way: an
///   empty one is a member the request carried.
///
/// The walk descends as the request nests, which is the depth the protocol boundary
/// already deserialized and constraint-checked this same value at.
struct RequestRendering {
    /// The nesting being built, innermost last. The bottom frame is the request itself.
    frames: Vec<Frame>,
}

impl RequestRendering {
    fn new() -> Self {
        Self {
            frames: vec![Frame::members()],
        }
    }

    /// Whether what is being emitted now sits in a structure MEMBER.
    fn in_member_position(&self) -> bool {
        matches!(self.frames.last(), Some(Frame::Ordered { members: true, .. }))
    }

    /// Emit one finished value into the frame being built.
    fn emit(&mut self, member: Option<&str>, value: &str) {
        let Some(frame) = self.frames.last_mut() else {
            return;
        };
        match frame {
            Frame::Ordered { rendered, .. } => {
                push_part(rendered, member.unwrap_or(""));
                push_part(rendered, value);
            }
            Frame::Keyed {
                entries,
                pending_key,
            } => match pending_key.take() {
                None => *pending_key = Some(value.to_string()),
                Some(key) => entries.push((key, value.to_string())),
            },
        }
    }

    /// Emit one leaf: its kind tag, then the value as text.
    fn leaf(&mut self, schema: &Schema, kind: char, value: &str) {
        let mut rendered = String::with_capacity(value.len() + 1);
        rendered.push(kind);
        rendered.push_str(value);
        self.emit(schema.member_name(), &rendered);
    }

    /// Render one container's children into their own frame, then emit the result.
    ///
    /// `collapse_empty` is what makes an absent collection and an empty one render alike,
    /// and it applies only where the container sits in a structure MEMBER: that is the
    /// only position a request can leave unwritten.
    fn container(
        &mut self,
        schema: &Schema,
        kind: char,
        frame: Frame,
        collapse_empty: bool,
        write_children: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.frames.push(frame);
        let outcome = write_children(self);
        let children = self
            .frames
            .pop()
            .map(Frame::into_rendering)
            .unwrap_or_default();
        outcome?;
        if collapse_empty && children.is_empty() && self.in_member_position() {
            return Ok(());
        }
        let mut rendered = String::with_capacity(children.len() + 1);
        rendered.push(kind);
        rendered.push_str(&children);
        self.emit(schema.member_name(), &rendered);
        Ok(())
    }

    /// The finished rendering of the whole request.
    fn finish(mut self) -> String {
        self.frames
            .pop()
            .map(Frame::into_rendering)
            .unwrap_or_default()
    }
}

impl ShapeSerializer for RequestRendering {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.container(
            schema,
            KIND_STRUCTURE,
            Frame::members(),
            false,
            &|children| value.serialize_members(children),
        )
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.container(
            schema,
            KIND_LIST,
            Frame::elements(),
            true,
            write_elements,
        )
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.container(
            schema,
            KIND_MAP,
            Frame::entries(),
            true,
            write_entries,
        )
    }

    fn write_boolean(&mut self, schema: &Schema, value: bool) -> Result<(), SerdeError> {
        self.leaf(schema, KIND_BOOLEAN, &value.to_string());
        Ok(())
    }

    fn write_byte(&mut self, schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.leaf(schema, KIND_NUMBER, &value.to_string());
        Ok(())
    }

    fn write_short(&mut self, schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.leaf(schema, KIND_NUMBER, &value.to_string());
        Ok(())
    }

    fn write_integer(&mut self, schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.leaf(schema, KIND_NUMBER, &value.to_string());
        Ok(())
    }

    fn write_long(&mut self, schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.leaf(schema, KIND_NUMBER, &value.to_string());
        Ok(())
    }

    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.leaf(schema, KIND_NUMBER, &value.to_string());
        Ok(())
    }

    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.leaf(schema, KIND_NUMBER, &value.to_string());
        Ok(())
    }

    fn write_big_integer(&mut self, schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        self.leaf(schema, KIND_NUMBER, value.as_ref());
        Ok(())
    }

    fn write_big_decimal(&mut self, schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        self.leaf(schema, KIND_NUMBER, value.as_ref());
        Ok(())
    }

    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        self.leaf(schema, KIND_STRING, value);
        Ok(())
    }

    fn write_blob(&mut self, schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        // The bytes themselves, not their length: two blobs of one size are two values.
        self.leaf(schema, KIND_BLOB, &hex::encode(value));
        Ok(())
    }

    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        self.leaf(schema, KIND_TIMESTAMP, &millis_of(value).to_string());
        Ok(())
    }

    fn write_document(&mut self, schema: &Schema, value: &Document) -> Result<(), SerdeError> {
        let mut rendered = String::new();
        push_document(&mut rendered, value);
        self.leaf(schema, KIND_DOCUMENT, &rendered);
        Ok(())
    }

    fn write_null(&mut self, schema: &Schema) -> Result<(), SerdeError> {
        self.leaf(schema, KIND_NULL, "");
        Ok(())
    }
}

/// Append a free-form `Document`, which carries no schema to walk, so its own shape is
/// what is rendered: an object's members in key order, on the same terms as a modelled
/// map's, and an array's items in order.
///
/// No input shape of this service targets a document, so nothing reaches this today; it
/// is here because [`request_fingerprint`] is the rendering of a request for every
/// operation, and a renderer that silently dropped a member's value would answer a
/// different request as a replay.
fn push_document(rendered: &mut String, document: &Document) {
    match document {
        Document::Null => push_part(rendered, &KIND_NULL.to_string()),
        Document::Bool(value) => push_part(rendered, &format!("{KIND_BOOLEAN}{value}")),
        Document::Number(number) => {
            let value = match number {
                ::aws_smithy_types::Number::PosInt(value) => value.to_string(),
                ::aws_smithy_types::Number::NegInt(value) => value.to_string(),
                ::aws_smithy_types::Number::Float(value) => value.to_string(),
            };
            push_part(rendered, &format!("{KIND_NUMBER}{value}"));
        }
        Document::String(value) => push_part(rendered, &format!("{KIND_STRING}{value}")),
        Document::Array(items) => {
            push_part(rendered, &format!("{KIND_LIST}{}", items.len()));
            for item in items {
                push_document(rendered, item);
            }
        }
        Document::Object(members) => {
            push_part(rendered, &format!("{KIND_MAP}{}", members.len()));
            let mut sorted: Vec<(&String, &Document)> = members.iter().collect();
            sorted.sort_unstable_by_key(|(name, _)| *name);
            for (name, member) in sorted {
                push_part(rendered, name);
                push_document(rendered, member);
            }
        }
    }
}

/// A fixed-size fingerprint of EVERY member a request carried.
///
/// `clientToken` is `@idempotencyToken`, which means "the same token names the same
/// request": a second call carrying a token already spent is a replay only when it
/// really is the same request, and this digest is how that is decided. It is taken from
/// the request value itself ([`RequestRendering`]), so every operation whose input
/// carries such a token decides a replay the same way and no member is left out of the
/// comparison.
///
/// SHA-256, so the size is fixed however large the request was.
pub fn request_fingerprint(input: &dyn SerializableStruct) -> String {
    let mut rendering = RequestRendering::new();
    // Nothing in this walk can fail — every write appends to a `String` — so this is the
    // serializer error channel going unused. A failure is folded INTO the rendering
    // rather than dropped, so a request this could not render whole never fingerprints
    // like one it could.
    let outcome = input.serialize_members(&mut rendering);
    let mut rendered = rendering.finish();
    if let Err(error) = outcome {
        push_part(&mut rendered, &error.to_string());
    }

    let mut hasher = Sha256::new();
    hasher.update(rendered.as_bytes());
    hex::encode(hasher.finalize())
}

/// One job template, as `CreateJobTemplate` persisted it.
///
/// A job template has no lifecycle state: EMR on EKS stores the `StartJobRun` values and
/// nothing about it transitions, so there is no `state` member and no pending transition
/// here.
#[derive(Debug, Clone)]
pub struct JobTemplateRecord {
    pub id: String,
    /// The region whose endpoint minted this template, on the terms
    /// [`VirtualClusterRecord::region`] states.
    pub region: String,
    pub name: String,
    pub arn: String,
    pub created_at_millis: i64,
    pub tags: HashMap<String, String>,
    pub job_template_data: JobTemplateData,
    pub kms_key_arn: Option<String>,
    pub client_token: String,
    /// The principal that created this template ([`caller_identity_arn`]), recorded at
    /// `CreateJobTemplate` so every later read attributes it to the same identity.
    pub created_by: String,
    /// [`request_fingerprint`] of the request that minted this template, so a later
    /// call carrying the same `clientToken` is answered as a replay only when it is the
    /// same request.
    pub request_fingerprint: String,
    /// Creation sequence, so templates minted within one millisecond still have a total
    /// order for `ListJobTemplates` to page through.
    pub sequence: u64,
}

impl JobTemplateRecord {
    /// The modelled `JobTemplate` this record represents.
    ///
    /// `decryptionError` stays absent: a `kmsKeyArn` is persisted as submitted and
    /// nothing is encrypted, so no decryption ever failed.
    pub fn to_job_template(&self) -> JobTemplate {
        JobTemplate {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(date_time(self.created_at_millis)),
            created_by: Some(self.created_by.clone()),
            tags: Some(self.tags.clone()),
            job_template_data: self.job_template_data.clone(),
            kms_key_arn: self.kms_key_arn.clone(),
            decryption_error: None,
        }
    }
}

/// One security configuration, as `CreateSecurityConfiguration` persisted it.
///
/// It carries no state member: EMR on EKS models a security configuration as a template,
/// with no lifecycle status, so there is no transition for the clock to owe one.
#[derive(Debug, Clone)]
pub struct SecurityConfigurationRecord {
    pub id: String,
    /// The region whose endpoint minted this configuration, on the terms
    /// [`VirtualClusterRecord::region`] states.
    pub region: String,
    pub name: String,
    pub arn: String,
    pub created_at_millis: i64,
    pub security_configuration_data: SecurityConfigurationData,
    pub container_provider: Option<ContainerProvider>,
    pub tags: HashMap<String, String>,
    pub client_token: String,
    /// [`request_fingerprint`] of the `CreateSecurityConfiguration` request that minted
    /// this configuration, so a later call carrying the same `clientToken` is answered as
    /// a replay only when it is the same request.
    pub request_fingerprint: String,
    /// The principal that created this configuration ([`caller_identity_arn`]), recorded
    /// at `CreateSecurityConfiguration` so every later read attributes it to the same
    /// identity.
    pub created_by: String,
    /// Creation sequence, so configurations minted within one millisecond still have a
    /// total order for `ListSecurityConfigurations` to page through.
    pub sequence: u64,
}

impl SecurityConfigurationRecord {
    /// The modelled `SecurityConfiguration` this record represents.
    ///
    /// `containerProvider` is reported from the record `CreateSecurityConfiguration`
    /// persisted it on: the create accepts one, and AWS echoes it back on every read of
    /// the configuration even though the on-disk model does not declare the member — see
    /// the shape in `ops::types::shared_types` for why it is carried there.
    pub fn to_security_configuration(&self) -> SecurityConfiguration {
        SecurityConfiguration {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(date_time(self.created_at_millis)),
            created_by: Some(self.created_by.clone()),
            security_configuration_data: Some(self.security_configuration_data.clone()),
            tags: Some(self.tags.clone()),
            container_provider: self.container_provider.clone(),
        }
    }
}

/// One managed endpoint, as `CreateManagedEndpoint` persisted it.
#[derive(Debug, Clone)]
pub struct ManagedEndpointRecord {
    pub id: String,
    /// The region whose endpoint created this managed endpoint, on the terms
    /// [`VirtualClusterRecord::region`] states. It is the region of the cluster it
    /// gateways into.
    pub region: String,
    pub name: String,
    pub virtual_cluster_id: String,
    pub arn: String,
    /// The endpoint's `type` (an `EndpointType`, free-form `\S` in the model), spelled
    /// without the keyword so the record needs no raw identifier.
    pub endpoint_type: String,
    pub state: String,
    pub release_label: String,
    pub execution_role_arn: String,
    pub certificate_arn: Option<String>,
    pub configuration_overrides: Option<ConfigurationOverrides>,
    /// `CreateManagedEndpoint$sessionIdleTimeoutInMinutes`, persisted as submitted. The
    /// `Endpoint` shape declares no member for it, so no read reports it; it is kept
    /// because it is part of the endpoint this request asked for.
    pub session_idle_timeout_in_minutes: i32,
    pub created_at_millis: i64,
    pub tags: HashMap<String, String>,
    pub client_token: String,
    /// [`request_fingerprint`] of the `CreateManagedEndpoint` request that minted this
    /// endpoint, so a later call carrying the same `clientToken` is answered as a replay
    /// only when it is the same request.
    pub request_fingerprint: String,
    /// Creation sequence, so endpoints minted within one millisecond still have a total
    /// order for `ListManagedEndpoints` to page through.
    pub sequence: u64,
    /// What this endpoint is reached THROUGH, as `CreateManagedEndpoint` provisioned it:
    /// the gateway's address, the certificate that secures it and the network placement it
    /// runs in ([`EndpointConnection`]). Settled once, at create, so every read of the
    /// endpoint reports the same connection.
    pub connection: EndpointConnection,
    pub pending: Option<PendingTransition>,
}

impl ManagedEndpointRecord {
    /// Materialise this endpoint's transition if the clock has made it due. `Endpoint`
    /// declares no "finished" instant, so a transition only moves the state.
    fn settle(&mut self, now: Instant) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if now < pending.due_at {
            self.pending = Some(pending);
            return;
        }
        self.state = pending.next_state.to_string();
    }

    /// The modelled `Endpoint` this record represents.
    ///
    /// The connection members are reported from the [`EndpointConnection`] the create
    /// provisioned this endpoint with: `serverUrl` (the address that leads back to this
    /// emulator), `certificateAuthority` (the certificate generated for that address) and
    /// the `securityGroup` / `subnetIds` the container cluster's OWNER reported for it —
    /// so a client reading this endpoint has what it needs to reach it, and two reads
    /// agree.
    ///
    /// The members this emulator leaves ABSENT are the ones nothing here has to report:
    ///
    /// * `securityGroup` / `subnetIds` when the placement was not resolved — an endpoint
    ///   reports the network its owner holds and never one of its own (see
    ///   [`ContainerClusterNetwork`]).
    /// * `certificateAuthority$certificateArn` — see [`mint_endpoint_certificate`]: no ACM
    ///   record answers for material this service generated, so nothing names it.
    /// * `authProxyUrl` — "the authentication proxy URL of the endpoint": nothing in this
    ///   emulator proxies authentication in front of the gateway, so there is no second
    ///   address to report; the gateway's own is `serverUrl`.
    /// * `stateDetails` / `failureReason` — "additional details of the endpoint state" and
    ///   "the reasons why the endpoint has failed": nothing here failed.
    pub fn to_endpoint(&self) -> Endpoint {
        Endpoint {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            virtual_cluster_id: Some(self.virtual_cluster_id.clone()),
            r#type: Some(self.endpoint_type.clone()),
            state: Some(self.state.clone()),
            release_label: Some(self.release_label.clone()),
            execution_role_arn: Some(self.execution_role_arn.clone()),
            certificate_arn: self.certificate_arn.clone(),
            certificate_authority: Some(Certificate {
                certificate_arn: None,
                certificate_data: Some(self.connection.certificate_data.clone()),
            }),
            configuration_overrides: self.configuration_overrides.clone(),
            server_url: Some(self.connection.server_url.clone()),
            auth_proxy_url: None,
            created_at: Some(date_time(self.created_at_millis)),
            security_group: self.connection.security_group.clone(),
            subnet_ids: (!self.connection.subnet_ids.is_empty())
                .then(|| self.connection.subnet_ids.clone()),
            state_details: None,
            failure_reason: None,
            tags: Some(self.tags.clone()),
        }
    }
}

/// One managed-endpoint session, as `GetManagedEndpointSessionCredentials` issued it.
///
/// The operation does not report a resource: it MINTS one ("Generate a session token to
/// connect to a managed endpoint"), so the session is persisted and the response is read
/// back out of the stored record. Nothing else reads a session: it exists so the
/// `@idempotencyToken` replay of the request that minted it answers with the SAME session,
/// and so the store stays bounded.
#[derive(Debug, Clone)]
pub struct EndpointSessionRecord {
    /// The `ResourceIdString` the response reports as `id` ("The identifier of the session
    /// token returned"), minted by [`ComAmazonawsEmrcontainersAwschicagowebserviceState::mint_id`].
    pub id: String,
    /// The region whose endpoint issued this session, on the terms
    /// [`VirtualClusterRecord::region`] states.
    pub region: String,
    pub virtual_cluster_id: String,
    pub endpoint_id: String,
    /// `executionRoleArn` — "The IAM Execution Role ARN that will be used by the job run" —
    /// as the request asked for it. The emulator resolves nothing against IAM, so it is
    /// recorded as part of the session this call asked for rather than checked.
    pub execution_role_arn: String,
    /// The `credentialType` the session was issued for
    /// ([`SESSION_CREDENTIAL_TYPE_TOKEN`] is the only one this operation accepts).
    pub credential_type: String,
    /// `logContext` — "String identifier used to separate sections of the execution logs
    /// uploaded to S3" — when the request carried one.
    pub log_context: Option<String>,
    /// The session token handed to the caller, minted by the emulator's credential store
    /// ([`mint_session_token`]).
    ///
    /// Held here ONLY so a replay of the request that minted it answers with the same
    /// session rather than a second one; it goes nowhere else — no file, no log — and
    /// leaves the store with the record when the session expires.
    pub token: String,
    /// How long the session was issued for, in the `durationInSeconds` the request named or
    /// [`DEFAULT_SESSION_DURATION_SECONDS`] when it named none.
    pub duration_seconds: i32,
    pub issued_at_millis: i64,
    /// The wall-clock instant the response reports as `expiresAt`.
    pub expires_at_millis: i64,
    /// The monotonic instant the session expires at — what actually decides expiry, the way
    /// [`PendingTransition::due_at`] decides a status transition, so a wall clock stepped
    /// between the two calls cannot keep an expired session alive.
    pub expires_at: Instant,
    /// The `clientToken` this session was issued under, when the request carried one: the
    /// member is optional, so a request naming none is simply not replayable.
    pub client_token: Option<String>,
    /// [`request_fingerprint`] of the request that minted this session, so a later call
    /// carrying the same `clientToken` is answered as a replay only when it is the same
    /// request.
    pub request_fingerprint: String,
    /// Issue sequence, so sessions minted within one millisecond still have a total order
    /// for the oldest-first reclaim to walk.
    pub sequence: u64,
}

impl EndpointSessionRecord {
    /// The modelled response this session answers with.
    ///
    /// The model declares TWO members for the credentials this call returns — `credentials`
    /// ("The structure containing the session credentials") and `endpointCredentials` ("The
    /// session credentials that the operation returns") — and there is one session token to
    /// report, so both name it: a client reading either member reaches the same session.
    pub fn to_session_credentials(&self) -> GetManagedEndpointSessionCredentialsOutput {
        GetManagedEndpointSessionCredentialsOutput {
            id: Some(self.id.clone()),
            credentials: Some(Credentials::Token(self.token.clone())),
            endpoint_credentials: Some(Credentials::Token(self.token.clone())),
            expires_at: Some(date_time(self.expires_at_millis)),
        }
    }
}

/// Per-service state for com-amazonaws-emrcontainers-awschicagowebservice. A handler reaches it through [`service_state`].
///
/// # Bounding the store
///
/// Every record set is CAPPED ([`MAX_VIRTUAL_CLUSTERS`], [`MAX_JOB_RUNS`],
/// [`MAX_JOB_TEMPLATES`], [`MAX_SECURITY_CONFIGURATIONS`], [`MAX_MANAGED_ENDPOINTS`],
/// [`MAX_ENDPOINT_SESSIONS`]) and
/// retired
/// records are reclaimed oldest-first when a cap is reached, so the store cannot grow
/// with the number of requests served: a caller that creates resources in a loop ends up
/// recycling the same bounded set of slots, and once every slot holds a LIVE resource the
/// next create is refused with the modelled `ValidationException` instead of allocating.
/// Each record's own size is bounded by the model's length constraints on the members it
/// stores and, above those, by the server's request-body cap.
#[derive(Debug, Default)]
pub struct ComAmazonawsEmrcontainersAwschicagowebserviceState {
    /// Virtual clusters by `id`. A terminated cluster stays here while it has a slot: EMR
    /// on EKS keeps it describable rather than answering 404 once it is gone.
    ///
    /// Every record set below holds the resources of EVERY region this emulator has been
    /// called for, each record carrying the region it was created in (`region`), and a
    /// lookup is region-scoped: EMR on EKS is a regional service, so an id resolves only
    /// against the records of the region the request is signed for.
    pub virtual_clusters: HashMap<String, VirtualClusterRecord>,
    /// Job runs by `id`.
    pub job_runs: HashMap<String, JobRunRecord>,
    /// Security configurations by `id`, capped at [`MAX_SECURITY_CONFIGURATIONS`].
    pub security_configurations: HashMap<String, SecurityConfigurationRecord>,
    /// Job-run ids per virtual cluster, in creation order.
    pub job_runs_by_cluster: HashMap<String, Vec<String>>,
    /// Managed endpoints by `id`, capped at [`MAX_MANAGED_ENDPOINTS`].
    pub managed_endpoints: HashMap<String, ManagedEndpointRecord>,
    /// Managed-endpoint ids per virtual cluster, in creation order.
    pub managed_endpoints_by_cluster: HashMap<String, Vec<String>>,
    /// Managed-endpoint sessions by `id`, capped at [`MAX_ENDPOINT_SESSIONS`]. A session
    /// leaves on its own expiry ([`Self::settle`]).
    pub endpoint_sessions: HashMap<String, EndpointSessionRecord>,
    /// Job templates by `id`. Capped by [`MAX_JOB_TEMPLATES`]; a template leaves only
    /// when `DeleteJobTemplate` removes it.
    pub job_templates: HashMap<String, JobTemplateRecord>,
    /// `(region, idempotency scope, clientToken) -> minted resource id`, so a replayed
    /// `@idempotencyToken` returns the resource the first call minted instead of a
    /// second one. The region is part of the key because the resource a token names
    /// lives in one region: the same token replayed against another region's endpoint is
    /// a different request there, and mints that region's own resource. One entry per
    /// stored record, inserted and reclaimed with it, so this map is bounded by the caps
    /// above.
    pub idempotency: HashMap<(String, String, String), String>,
    /// The ids of the clusters that owe the clock a transition, so [`Self::settle`] walks
    /// only those rather than the whole store.
    pub pending_clusters: HashSet<String>,
    /// The ids of the job runs that owe the clock a transition.
    pub pending_job_runs: HashSet<String>,
    /// The ids of the managed endpoints that owe the clock a transition.
    pub pending_managed_endpoints: HashSet<String>,
    /// Monotonic counter behind [`Self::mint_id`].
    pub id_counter: u64,
}

impl ComAmazonawsEmrcontainersAwschicagowebserviceState {
    /// Mint a fresh `ResourceIdString`: [`ID_WIDTH`] base-36 characters, so it matches
    /// `^[0-9a-z]+$`.
    ///
    /// The value mixes the wall clock with a monotonic counter, so two ids minted in the
    /// same millisecond still differ and an id is never reused.
    pub fn mint_id(&mut self) -> String {
        self.id_counter = self.id_counter.wrapping_add(1);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let mut value = nanos
            .wrapping_mul(1_000_003)
            .wrapping_add(u128::from(self.id_counter));
        let mut digits = Vec::with_capacity(ID_WIDTH);
        for _ in 0..ID_WIDTH {
            digits.push(char::from(ID_ALPHABET[(value % 36) as usize]));
            value /= 36;
        }
        digits.into_iter().rev().collect()
    }

    /// Materialise every status transition the clock has already made due.
    ///
    /// Called first by every handler — including the reporting ones, which may write
    /// only this: the change was owed at the same instant whether or not anybody called.
    ///
    /// A due start is the one transition that also needs scheduler capacity, so the job
    /// runs are walked oldest submission first and a start waits while the cluster's
    /// `maxConcurrentJobRuns` is fully taken; every other transition answers to the clock
    /// alone.
    ///
    /// It walks only the records that actually owe a transition, so one call costs the
    /// number of transitions in flight, not the size of the store.
    pub fn settle(&mut self) {
        let now = Instant::now();
        for id in std::mem::take(&mut self.pending_clusters) {
            let still_pending = match self.virtual_clusters.get_mut(&id) {
                Some(record) => {
                    record.settle(now);
                    record.pending.is_some()
                }
                None => false,
            };
            if still_pending {
                self.pending_clusters.insert(id);
            }
        }
        // Job runs oldest submission first, so a start that needs a free running slot
        // takes one in the order the runs were submitted rather than in whatever order
        // the pending set happens to hash.
        let mut queued: Vec<(i64, u64, String)> = std::mem::take(&mut self.pending_job_runs)
            .into_iter()
            .filter_map(|id| {
                let record = self.job_runs.get(&id)?;
                Some((record.created_at_millis, record.sequence, id))
            })
            .collect();
        queued.sort();
        for (_, _, id) in queued {
            // A START the clock has made due still waits while the cluster's configured
            // concurrency is fully taken: `maxConcurrentJobRuns` caps the runs that may
            // be RUNNING at once, and "as running slots free up, queued job runs start
            // automatically". The run keeps the transition it already owes, with its due
            // instant in the past, so the first call after a slot frees starts it. Every
            // other transition (a cancellation, a teardown) answers to the clock alone.
            let Some(record) = self.job_runs.get(&id) else {
                continue;
            };
            let due_transition = record
                .pending
                .as_ref()
                .filter(|transition| now >= transition.due_at)
                .map(|transition| transition.next_state);
            let waiting_for_a_slot = due_transition == Some(JOB_RUNNING)
                && !self.cluster_has_free_running_slot(&record.virtual_cluster_id);
            // The end of a run's driver lands in the state the compute it was admitted to
            // accounts for, so the cluster that hosts the run is read here, before the run
            // is moved, and only for the transition that end is (see
            // [`JobRunRecord::settle`]).
            let compute_available = due_transition == Some(JOB_DRIVER_END)
                && self.job_run_compute_is_available(record);
            if waiting_for_a_slot {
                self.pending_job_runs.insert(id);
                continue;
            }
            let still_pending = match self.job_runs.get_mut(&id) {
                Some(record) => {
                    record.settle(now, compute_available);
                    record.pending.is_some()
                }
                None => false,
            };
            if still_pending {
                self.pending_job_runs.insert(id);
            }
        }
        // A managed endpoint's CREATING -> ACTIVE step answers to the clock alone: it
        // takes no scheduler slot and nothing else gates it.
        for id in std::mem::take(&mut self.pending_managed_endpoints) {
            let still_pending = match self.managed_endpoints.get_mut(&id) {
                Some(record) => {
                    record.settle(now);
                    record.pending.is_some()
                }
                None => false,
            };
            if still_pending {
                self.pending_managed_endpoints.insert(id);
            }
        }
        // A session's own expiry is the change the clock owes it: past its `expiresAt` the
        // token authenticates nothing, so the record goes — owed at that instant whether or
        // not anybody called. Every stored session owes exactly this one change, so walking
        // them all walks only records with a change in flight, and the walk is capped by
        // [`MAX_ENDPOINT_SESSIONS`].
        let expired: Vec<String> = self
            .endpoint_sessions
            .values()
            .filter(|record| now >= record.expires_at)
            .map(|record| record.id.clone())
            .collect();
        for id in expired {
            self.remove_endpoint_session(&id);
        }
    }

    /// How many of this cluster's stored job runs are in a state `wanted` accepts, as the
    /// `NonNegativeInteger` the model counts them in.
    ///
    /// The runs are read with the region of the cluster itself, so a count is taken over
    /// exactly the runs that cluster's endpoint holds; an id that names no cluster holds no
    /// run to count.
    fn cluster_run_count(&self, virtual_cluster_id: &str, wanted: fn(&str) -> bool) -> i32 {
        let Some(region) = self
            .virtual_clusters
            .get(virtual_cluster_id)
            .map(|record| record.region.as_str())
        else {
            return 0;
        };
        let counted = self
            .cluster_job_runs(region, virtual_cluster_id)
            .filter(|record| wanted(&record.state))
            .count();
        i32::try_from(counted).unwrap_or(i32::MAX)
    }

    /// The current in-queue and concurrent job-run counts for this cluster
    /// (`VirtualCluster$schedulerStatus`), counted out of the job runs this service holds
    /// for it: the `PENDING`/`SUBMITTED` runs are the queue, the `RUNNING` runs are the
    /// concurrency in use.
    ///
    /// Reported for every cluster, whatever its `schedulerConfiguration`: the counts are a
    /// fact about the stored runs rather than a property of a limit, and the model gives
    /// both members the default `0`, so a cluster holding no run reports zeros.
    pub fn cluster_scheduler_status(&self, virtual_cluster_id: &str) -> SchedulerStatus {
        SchedulerStatus {
            current_in_queue_job_runs: self
                .cluster_run_count(virtual_cluster_id, job_state_is_in_queue),
            current_concurrent_job_runs: self
                .cluster_run_count(virtual_cluster_id, |state| state == JOB_RUNNING),
        }
    }

    /// Whether the compute a stored job run was admitted to is still available to finish
    /// it: the virtual cluster the run names is still held at the run's own region and
    /// still `RUNNING`.
    ///
    /// A job run is work submitted to ONE virtual cluster's compute
    /// (`POST /virtualclusters/{virtualClusterId}/jobruns`), so this is what decides the
    /// end its driver reaches ([`job_run_outcome`]): a cluster past `RUNNING` is being or
    /// has been torn down, and a cluster this endpoint no longer holds is gone, and in
    /// neither case is there compute left to carry the run's driver to its end.
    fn job_run_compute_is_available(&self, record: &JobRunRecord) -> bool {
        self.virtual_clusters
            .get(&record.virtual_cluster_id)
            .is_some_and(|cluster| {
                cluster.region == record.region && cluster.state == CLUSTER_RUNNING
            })
    }

    /// Whether this cluster may take one more RUNNING job run right now.
    ///
    /// `schedulerConfiguration.maxConcurrentJobRuns` is "the maximum number of job runs
    /// that can be in the `RUNNING` state at any time for the virtual cluster"; a cluster
    /// that configures none is never gated ("if you omit this field, the service applies
    /// no concurrency limit"), and nor is an id that names no cluster — a run whose
    /// cluster is gone is not held back by a limit nobody set.
    pub fn cluster_has_free_running_slot(&self, virtual_cluster_id: &str) -> bool {
        let limit = self
            .virtual_clusters
            .get(virtual_cluster_id)
            .and_then(|record| record.scheduler_configuration.as_ref())
            .and_then(|configuration| configuration.max_concurrent_job_runs);
        match limit {
            Some(limit) => {
                self.cluster_run_count(virtual_cluster_id, |state| state == JOB_RUNNING) < limit
            }
            None => true,
        }
    }

    /// This cluster's `schedulerConfiguration.maxInQueueJobRuns`, but only when its queue
    /// already holds that many runs: "when the queue is full, the service rejects
    /// `StartJobRun` requests with a `ValidationException`", and the caller needs the
    /// limit to say so.
    ///
    /// `None` when the cluster sets no queue-depth limit ("if you omit this field, the
    /// service applies no queue-depth limit"), when its queue still has room, or when the
    /// id names no cluster.
    pub fn cluster_full_queue_limit(&self, virtual_cluster_id: &str) -> Option<i32> {
        let limit = self
            .virtual_clusters
            .get(virtual_cluster_id)?
            .scheduler_configuration
            .as_ref()?
            .max_in_queue_job_runs?;
        (self.cluster_run_count(virtual_cluster_id, job_state_is_in_queue) >= limit)
            .then_some(limit)
    }

    /// Make room for one more virtual cluster, reclaiming the oldest RETIRED ones (a
    /// TERMINATED cluster holding no job run that has not finished, together with the
    /// finished runs it still holds) when the cap is reached.
    ///
    /// `false` when every slot holds a live cluster: the caller must then refuse the
    /// create rather than grow the store.
    pub fn reserve_virtual_cluster(&mut self) -> bool {
        while self.virtual_clusters.len() >= MAX_VIRTUAL_CLUSTERS {
            // A retired cluster is one that is TERMINATED and holds no job run that has
            // not finished. Its remaining (finished) runs are reclaimed WITH it, so a
            // cluster slot is never held hostage by a run that is already over.
            let retired = self
                .virtual_clusters
                .values()
                .filter(|record| {
                    record.state == CLUSTER_TERMINATED
                        && self
                            .cluster_job_runs(&record.region, &record.id)
                            .all(|run| job_state_is_terminal(&run.state))
                })
                .min_by_key(|record| (record.created_at_millis, record.sequence))
                .map(|record| {
                    (
                        record.id.clone(),
                        record.region.clone(),
                        record.client_token.clone(),
                    )
                });
            let Some((id, region, client_token)) = retired else {
                return false;
            };
            for run_id in self.job_runs_by_cluster.remove(&id).unwrap_or_default() {
                self.pending_job_runs.remove(&run_id);
                if let Some(run) = self.job_runs.remove(&run_id) {
                    self.idempotency.remove(&(
                        run.region,
                        start_job_run_scope(&id),
                        run.client_token,
                    ));
                }
            }
            // A managed endpoint lives UNDER a virtual cluster, so the cluster's endpoints
            // are reclaimed with it rather than left naming a cluster that is gone.
            for endpoint_id in self
                .managed_endpoints_by_cluster
                .remove(&id)
                .unwrap_or_default()
            {
                self.pending_managed_endpoints.remove(&endpoint_id);
                if let Some(endpoint) = self.managed_endpoints.remove(&endpoint_id) {
                    self.idempotency.remove(&(
                        endpoint.region,
                        create_managed_endpoint_scope(&id),
                        endpoint.client_token,
                    ));
                }
            }
            self.virtual_clusters.remove(&id);
            self.pending_clusters.remove(&id);
            self.idempotency.remove(&(
                region,
                CREATE_VIRTUAL_CLUSTER_SCOPE.to_string(),
                client_token,
            ));
        }
        true
    }

    /// Make room for one more job run, reclaiming the oldest RETIRED ones (a run in a
    /// terminal state) when the cap is reached.
    ///
    /// `false` when every slot holds a run that has not finished.
    pub fn reserve_job_run(&mut self) -> bool {
        while self.job_runs.len() >= MAX_JOB_RUNS {
            let retired = self
                .job_runs
                .values()
                .filter(|record| job_state_is_terminal(&record.state))
                .min_by_key(|record| (record.created_at_millis, record.sequence))
                .map(|record| {
                    (
                        record.id.clone(),
                        record.region.clone(),
                        record.virtual_cluster_id.clone(),
                        record.client_token.clone(),
                    )
                });
            let Some((id, region, virtual_cluster_id, client_token)) = retired else {
                return false;
            };
            self.job_runs.remove(&id);
            self.pending_job_runs.remove(&id);
            if let Some(ids) = self.job_runs_by_cluster.get_mut(&virtual_cluster_id) {
                ids.retain(|stored| stored != &id);
            }
            self.idempotency.remove(&(
                region,
                start_job_run_scope(&virtual_cluster_id),
                client_token,
            ));
        }
        true
    }

    /// Store a freshly minted virtual cluster, with the idempotency entry its
    /// `clientToken` earns and its pending transition (if any) registered. One function,
    /// so a record can never be stored without them.
    pub fn insert_virtual_cluster(&mut self, record: VirtualClusterRecord) {
        if record.pending.is_some() {
            self.pending_clusters.insert(record.id.clone());
        }
        self.idempotency.insert(
            (
                record.region.clone(),
                CREATE_VIRTUAL_CLUSTER_SCOPE.to_string(),
                record.client_token.clone(),
            ),
            record.id.clone(),
        );
        self.virtual_clusters.insert(record.id.clone(), record);
    }

    /// Store a freshly minted job run, with its per-cluster index entry, its idempotency
    /// entry and its pending transition.
    pub fn insert_job_run(&mut self, record: JobRunRecord) {
        if record.pending.is_some() {
            self.pending_job_runs.insert(record.id.clone());
        }
        self.job_runs_by_cluster
            .entry(record.virtual_cluster_id.clone())
            .or_default()
            .push(record.id.clone());
        self.idempotency.insert(
            (
                record.region.clone(),
                start_job_run_scope(&record.virtual_cluster_id),
                record.client_token.clone(),
            ),
            record.id.clone(),
        );
        self.job_runs.insert(record.id.clone(), record);
    }

    /// Make room for one more managed endpoint, reclaiming the oldest RETIRED ones (an
    /// endpoint in a terminal [`EndpointState`](endpoint_state_is_terminal)) when the cap
    /// is reached.
    ///
    /// `false` when every slot holds an endpoint that has not been torn down: the caller
    /// must then refuse the create rather than grow the store.
    pub fn reserve_managed_endpoint(&mut self) -> bool {
        while self.managed_endpoints.len() >= MAX_MANAGED_ENDPOINTS {
            let retired = self
                .managed_endpoints
                .values()
                .filter(|record| endpoint_state_is_terminal(&record.state))
                .min_by_key(|record| (record.created_at_millis, record.sequence))
                .map(|record| {
                    (
                        record.id.clone(),
                        record.region.clone(),
                        record.virtual_cluster_id.clone(),
                        record.client_token.clone(),
                    )
                });
            let Some((id, region, virtual_cluster_id, client_token)) = retired else {
                return false;
            };
            self.managed_endpoints.remove(&id);
            self.pending_managed_endpoints.remove(&id);
            if let Some(ids) = self.managed_endpoints_by_cluster.get_mut(&virtual_cluster_id) {
                ids.retain(|stored| stored != &id);
            }
            self.idempotency.remove(&(
                region,
                create_managed_endpoint_scope(&virtual_cluster_id),
                client_token,
            ));
        }
        true
    }

    /// Store a freshly minted managed endpoint, with its per-cluster index entry, its
    /// idempotency entry and its pending transition registered. One function, so a record
    /// can never be stored without them.
    pub fn insert_managed_endpoint(&mut self, record: ManagedEndpointRecord) {
        if record.pending.is_some() {
            self.pending_managed_endpoints.insert(record.id.clone());
        }
        self.managed_endpoints_by_cluster
            .entry(record.virtual_cluster_id.clone())
            .or_default()
            .push(record.id.clone());
        self.idempotency.insert(
            (
                record.region.clone(),
                create_managed_endpoint_scope(&record.virtual_cluster_id),
                record.client_token.clone(),
            ),
            record.id.clone(),
        );
        self.managed_endpoints.insert(record.id.clone(), record);
    }

    /// The stored managed endpoint with this id, only when it really belongs to this
    /// cluster IN this region: an endpoint id under the wrong virtual cluster, or one of
    /// another region, is not found, not somebody else's endpoint.
    pub fn managed_endpoint(
        &self,
        region: &str,
        virtual_cluster_id: &str,
        id: &str,
    ) -> Option<&ManagedEndpointRecord> {
        self.managed_endpoints.get(id).filter(|record| {
            record.region == region && record.virtual_cluster_id == virtual_cluster_id
        })
    }

    /// This cluster's stored managed endpoints at `region`'s endpoint, in creation order:
    /// what a `ListManagedEndpoints` against that endpoint reports on.
    ///
    /// The region is part of the read, not only of the caller's cluster lookup, so the
    /// records a listing pages are keyed exactly as the cluster that gates it and as the
    /// cursor that positions into them: a record of another region's endpoint is never one
    /// this endpoint reports, whatever a request's cluster id resolves to.
    pub fn cluster_managed_endpoints<'a>(
        &'a self,
        region: &'a str,
        virtual_cluster_id: &str,
    ) -> impl Iterator<Item = &'a ManagedEndpointRecord> {
        self.managed_endpoints_by_cluster
            .get(virtual_cluster_id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.managed_endpoints.get(id))
            .filter(move |record| record.region == region)
    }

    /// The virtual cluster a `virtualClusterIdentifier` names, for a request signed for
    /// `region`.
    ///
    /// The member is a `String2048` documented as "The ARN of the Virtual Cluster", while
    /// every other operation of this service names a cluster by its `ResourceIdString` id,
    /// so BOTH forms resolve here. An ARN is matched in FULL against the stored record's
    /// own `arn`, the way a tagging call's `resourceArn` is matched
    /// ([`Self::resource_tags_mut`]): one that names another partition, region or account —
    /// a cluster some other endpoint holds — resolves to nothing rather than to this
    /// endpoint's cluster of the same id.
    pub fn identified_virtual_cluster(
        &self,
        region: &str,
        identifier: &str,
    ) -> Option<&VirtualClusterRecord> {
        if let Some(TaggedResource::VirtualCluster { id }) = parse_tagged_resource_arn(identifier) {
            return self
                .virtual_cluster(region, &id)
                .filter(|record| record.arn == identifier);
        }
        // An ARN that names no virtual cluster names none to resolve; anything that is not
        // an ARN at all is the id itself.
        if looks_like_an_arn(identifier) {
            return None;
        }
        self.virtual_cluster(region, identifier)
    }

    /// The managed endpoint an `endpointIdentifier` names under this virtual cluster, on
    /// the terms [`Self::identified_virtual_cluster`] states: its `ResourceIdString` id, or
    /// its full `EndpointArn`.
    ///
    /// An endpoint ARN carries the cluster the endpoint belongs to, so one offered under a
    /// DIFFERENT cluster matches no record of that cluster.
    pub fn identified_managed_endpoint(
        &self,
        region: &str,
        virtual_cluster_id: &str,
        identifier: &str,
    ) -> Option<&ManagedEndpointRecord> {
        if let Some(TaggedResource::ManagedEndpoint { id, .. }) =
            parse_tagged_resource_arn(identifier)
        {
            return self
                .managed_endpoint(region, virtual_cluster_id, &id)
                .filter(|record| record.arn == identifier);
        }
        if looks_like_an_arn(identifier) {
            return None;
        }
        self.managed_endpoint(region, virtual_cluster_id, identifier)
    }

    /// Make room for one more managed-endpoint session, reclaiming the oldest RETIRED ones
    /// (a session that has expired, or one issued for an endpoint the store no longer
    /// holds) when the cap is reached.
    ///
    /// `false` when every slot holds a valid session of a live endpoint: the caller must
    /// then refuse the request rather than grow the store.
    pub fn reserve_endpoint_session(&mut self) -> bool {
        while self.endpoint_sessions.len() >= MAX_ENDPOINT_SESSIONS {
            let now = Instant::now();
            let retired = self
                .endpoint_sessions
                .values()
                .filter(|record| {
                    now >= record.expires_at
                        || self
                            .managed_endpoint(
                                &record.region,
                                &record.virtual_cluster_id,
                                &record.endpoint_id,
                            )
                            .is_none()
                })
                .min_by_key(|record| (record.issued_at_millis, record.sequence))
                .map(|record| record.id.clone());
            let Some(id) = retired else {
                return false;
            };
            self.remove_endpoint_session(&id);
        }
        true
    }

    /// Store a freshly issued managed-endpoint session, with the idempotency entry its
    /// `clientToken` earns when it carried one. One function, so a record can never be
    /// stored without it.
    pub fn insert_endpoint_session(&mut self, record: EndpointSessionRecord) {
        if let Some(client_token) = record.client_token.clone() {
            self.idempotency.insert(
                (
                    record.region.clone(),
                    endpoint_session_scope(&record.virtual_cluster_id, &record.endpoint_id),
                    client_token,
                ),
                record.id.clone(),
            );
        }
        self.endpoint_sessions.insert(record.id.clone(), record);
    }

    /// Drop one stored session together with the idempotency entry it earned, so a token
    /// never names a session that is gone.
    fn remove_endpoint_session(&mut self, id: &str) {
        let Some(record) = self.endpoint_sessions.remove(id) else {
            return;
        };
        if let Some(client_token) = record.client_token {
            self.idempotency.remove(&(
                record.region,
                endpoint_session_scope(&record.virtual_cluster_id, &record.endpoint_id),
                client_token,
            ));
        }
    }

    /// The stored session with this id, only when it really was issued for this endpoint of
    /// this cluster IN this region: a session id offered against another endpoint names
    /// nothing, not that endpoint's session.
    pub fn endpoint_session(
        &self,
        region: &str,
        virtual_cluster_id: &str,
        endpoint_id: &str,
        id: &str,
    ) -> Option<&EndpointSessionRecord> {
        self.endpoint_sessions.get(id).filter(|record| {
            record.region == region
                && record.virtual_cluster_id == virtual_cluster_id
                && record.endpoint_id == endpoint_id
        })
    }

    /// Make room for one more job template.
    ///
    /// `false` once every slot is taken: a job template has no lifecycle state, so none
    /// of them is ever retired and there is nothing to reclaim — the caller must refuse
    /// the create rather than grow the store, and a `DeleteJobTemplate` is what frees a
    /// slot.
    pub fn reserve_job_template(&self) -> bool {
        self.job_templates.len() < MAX_JOB_TEMPLATES
    }

    /// Store a freshly minted job template, with the idempotency entry its `clientToken`
    /// earns. One function, so a record can never be stored without it.
    pub fn insert_job_template(&mut self, record: JobTemplateRecord) {
        self.idempotency.insert(
            (
                record.region.clone(),
                CREATE_JOB_TEMPLATE_SCOPE.to_string(),
                record.client_token.clone(),
            ),
            record.id.clone(),
        );
        self.job_templates.insert(record.id.clone(), record);
    }

    /// Remove a stored job template and the idempotency entry that named it. `None` when
    /// the id names no template.
    ///
    /// The entry goes WITH the record, so this emulator's token index holds exactly one
    /// entry per stored template and is bounded by [`MAX_JOB_TEMPLATES`] — the same terms
    /// [`Self::reserve_virtual_cluster`] reclaims a virtual cluster's entry on. The
    /// consequence is that a create retried with the same `clientToken` AFTER its
    /// template was deleted mints a new one rather than replaying: EMR on EKS documents
    /// no retention window for a spent `clientToken`, so how long a token outlives the
    /// resource it named is not a modelled behaviour, and bounding the index is the
    /// emulator's own choice. While the template exists — the whole window the dedup
    /// contract is about — the token never mints a second one.
    pub fn remove_job_template(&mut self, region: &str, id: &str) -> Option<JobTemplateRecord> {
        self.job_template(region, id)?;
        let record = self.job_templates.remove(id)?;
        self.idempotency.remove(&(
            record.region.clone(),
            CREATE_JOB_TEMPLATE_SCOPE.to_string(),
            record.client_token.clone(),
        ));
        Some(record)
    }

    /// The stored job template with this id, only when it belongs to this region.
    pub fn job_template(&self, region: &str, id: &str) -> Option<&JobTemplateRecord> {
        self.job_templates
            .get(id)
            .filter(|record| record.region == region)
    }

    /// This region's stored job templates, in no particular order: what a
    /// `ListJobTemplates` against this region's endpoint reports on.
    pub fn region_job_templates<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a JobTemplateRecord> {
        self.job_templates
            .values()
            .filter(move |record| record.region == region)
    }

    /// The modelled `JobTemplate` for this id in this region.
    pub fn job_template_view(&self, region: &str, id: &str) -> Option<JobTemplate> {
        Some(self.job_template(region, id)?.to_job_template())
    }

    /// Replace a stored virtual cluster's `schedulerConfiguration` with the one submitted,
    /// remembering `client_token` so a retry of the same update is ignored rather than
    /// applied twice.
    ///
    /// A REPLACEMENT, not a merge: `UpdateVirtualCluster$schedulerConfiguration` is "the
    /// scheduler configuration to apply to the virtual cluster. The new configuration
    /// fully replaces the existing one. If you omit a field, the corresponding limit is
    /// removed." A request that carries no configuration at all therefore leaves the
    /// cluster with no scheduler limits (see [`configured_scheduler_limits`]).
    pub fn apply_scheduler_configuration(
        &mut self,
        region: &str,
        id: &str,
        configuration: Option<SchedulerConfiguration>,
        client_token: &str,
    ) {
        if let Some(record) = self
            .virtual_clusters
            .get_mut(id)
            .filter(|record| record.region == region)
        {
            record.scheduler_configuration = configured_scheduler_limits(configuration);
            record.remember_update_token(client_token);
        }
    }

    /// Move a stored virtual cluster of this region into `now_state` with `next_state`
    /// owed to the clock.
    pub fn move_virtual_cluster(
        &mut self,
        region: &str,
        id: &str,
        now_state: &'static str,
        next_state: &'static str,
    ) {
        if let Some(record) = self
            .virtual_clusters
            .get_mut(id)
            .filter(|record| record.region == region)
        {
            record.state = now_state.to_string();
            record.pending = Some(PendingTransition::due(next_state));
            self.pending_clusters.insert(id.to_string());
        }
    }

    /// Move a stored job run of this region into `now_state` with `next_state` owed to
    /// the clock. The run reports the detail of the state it is moved into.
    pub fn move_job_run(
        &mut self,
        region: &str,
        id: &str,
        now_state: &'static str,
        next_state: &'static str,
    ) {
        if let Some(record) = self
            .job_runs
            .get_mut(id)
            .filter(|record| record.region == region)
        {
            record.state = now_state.to_string();
            record.state_details = job_run_state_details(now_state).map(str::to_string);
            record.pending = Some(PendingTransition::due(next_state));
            self.pending_job_runs.insert(id.to_string());
        }
    }

    /// Move a stored managed endpoint of this cluster and region into `now_state` with
    /// `next_state` owed to the clock.
    pub fn move_managed_endpoint(
        &mut self,
        region: &str,
        virtual_cluster_id: &str,
        id: &str,
        now_state: &'static str,
        next_state: &'static str,
    ) {
        if let Some(record) = self.managed_endpoints.get_mut(id).filter(|record| {
            record.region == region && record.virtual_cluster_id == virtual_cluster_id
        }) {
            record.state = now_state.to_string();
            record.pending = Some(PendingTransition::due(next_state));
            self.pending_managed_endpoints.insert(id.to_string());
        }
    }

    /// Start the teardown of a stored virtual cluster of this region, CARRYING DOWN
    /// everything that lives under it.
    ///
    /// The cluster moves to `TERMINATING` with `TERMINATED` owed to the clock, and in the
    /// same step every managed endpoint and every job run it holds that has not already
    /// reached a terminal state is given the terminal transition of its own kind: an
    /// endpoint to `TERMINATING` with `TERMINATED` owed, a run to `CANCEL_PENDING` with
    /// `CANCELLED` owed — the very two steps `CancelJobRun` writes. A child whose own
    /// teardown or cancellation is already in flight keeps the transition it owes, so a
    /// cascade never restarts one, and a child that has already finished keeps the state
    /// (and the `finishedAt`) it finished in.
    ///
    /// The children go down WITH the parent because a managed endpoint is a gateway INTO a
    /// virtual cluster and a job run runs IN one: neither can still be live in a cluster
    /// that no longer exists to host it, which is the same premise `CreateManagedEndpoint`
    /// and `StartJobRun` refuse new work on. It is also what lets the cluster reach the
    /// retired form [`Self::reserve_virtual_cluster`] and [`Self::reserve_managed_endpoint`]
    /// reclaim, rather than holding a slot for ever behind a child that never ends.
    pub fn start_virtual_cluster_teardown(&mut self, region: &str, id: &str) {
        // The children are read out of the store before anything is written, so the walk
        // borrows the records it decides on and the moves below borrow them mutably.
        let endpoints: Vec<String> = self
            .cluster_managed_endpoints(region, id)
            .filter(|record| {
                !endpoint_state_is_terminal(&record.state) && record.state != ENDPOINT_TERMINATING
            })
            .map(|record| record.id.clone())
            .collect();
        let runs: Vec<String> = self
            .cluster_job_runs(region, id)
            .filter(|record| {
                !job_state_is_terminal(&record.state) && record.state != JOB_CANCEL_PENDING
            })
            .map(|record| record.id.clone())
            .collect();

        self.move_virtual_cluster(region, id, CLUSTER_TERMINATING, CLUSTER_TERMINATED);
        for endpoint_id in endpoints {
            self.move_managed_endpoint(
                region,
                id,
                &endpoint_id,
                ENDPOINT_TERMINATING,
                ENDPOINT_TERMINATED,
            );
        }
        for run_id in runs {
            self.move_job_run(region, &run_id, JOB_CANCEL_PENDING, JOB_CANCELLED);
        }
    }

    /// The stored job run with this id, only when it really belongs to this cluster IN
    /// this region: a job-run id under the wrong virtual cluster, or one of another
    /// region, is not found, not somebody else's run.
    pub fn job_run(
        &self,
        region: &str,
        virtual_cluster_id: &str,
        id: &str,
    ) -> Option<&JobRunRecord> {
        self.job_runs.get(id).filter(|record| {
            record.region == region && record.virtual_cluster_id == virtual_cluster_id
        })
    }

    /// The stored virtual cluster with this id, only when it belongs to this region: EMR
    /// on EKS is regional, so an id minted against one region's endpoint names no
    /// resource at another's.
    pub fn virtual_cluster(&self, region: &str, id: &str) -> Option<&VirtualClusterRecord> {
        self.virtual_clusters
            .get(id)
            .filter(|record| record.region == region)
    }

    /// The modelled `VirtualCluster` for this id in this region, with its
    /// `schedulerStatus` counted out of the cluster's stored job runs (see
    /// [`Self::cluster_scheduler_status`]).
    pub fn virtual_cluster_view(&self, region: &str, id: &str) -> Option<VirtualCluster> {
        Some(self.virtual_cluster_view_of(self.virtual_cluster(region, id)?))
    }

    /// The modelled `VirtualCluster` for a record this store already holds, so a lister
    /// that has walked the records renders each one through the SAME rendering a describe
    /// answers with instead of looking every id up again.
    pub fn virtual_cluster_view_of(&self, record: &VirtualClusterRecord) -> VirtualCluster {
        record.to_virtual_cluster(self.cluster_scheduler_status(&record.id))
    }

    /// This region's stored virtual clusters, in no particular order: what a
    /// `ListVirtualClusters` against this region's endpoint reports on.
    pub fn region_virtual_clusters<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a VirtualClusterRecord> {
        self.virtual_clusters
            .values()
            .filter(move |record| record.region == region)
    }

    /// The virtual cluster this container provider's `namespace` is already mapped onto,
    /// if one holds it.
    ///
    /// A virtual cluster "is a Kubernetes namespace that Amazon EMR is registered with",
    /// and "a single virtual cluster maps to a single Kubernetes namespace"
    /// (`CreateVirtualCluster`), so one namespace of one container cluster carries at most
    /// one virtual cluster at a time. A TERMINATED cluster is no longer registered with
    /// its namespace and so holds nothing: the namespace is free for a new mapping once
    /// its cluster has been torn down.
    ///
    /// Read out of the stored records rather than out of a second index beside them, so
    /// the mapping a create resolves against and the one every read reports can never
    /// disagree. The walk is bounded by [`MAX_VIRTUAL_CLUSTERS`].
    pub fn cluster_mapped_to_namespace(
        &self,
        region: &str,
        container_provider_id: &str,
        namespace: &str,
    ) -> Option<&VirtualClusterRecord> {
        self.virtual_clusters.values().find(|record| {
            record.region == region
                && record.state != CLUSTER_TERMINATED
                && record.container_provider.id == container_provider_id
                && provider_namespace(&record.container_provider) == Some(namespace)
        })
    }

    /// The virtual cluster that still references the security configuration
    /// `configuration_id` names, if one does — the OLDEST such cluster, so the answer does
    /// not depend on the store's iteration order.
    ///
    /// A `securityConfigurationId` is a reference a virtual cluster holds for as long as
    /// the cluster is registered: it is reported on every read of that cluster, so the
    /// configuration it names is in use and cannot leave the store while it is held. A
    /// TERMINATED cluster has been torn down and holds nothing any more, on the same terms
    /// as [`Self::cluster_mapped_to_namespace`]: what its record still reports is what it
    /// referenced while it existed.
    ///
    /// Read out of the stored cluster records rather than out of a reference count beside
    /// them, so what a delete resolves against and what every read reports can never
    /// disagree. The walk is bounded by [`MAX_VIRTUAL_CLUSTERS`].
    pub fn cluster_referencing_security_configuration(
        &self,
        region: &str,
        configuration_id: &str,
    ) -> Option<&VirtualClusterRecord> {
        self.virtual_clusters
            .values()
            .filter(|record| {
                record.region == region
                    && record.state != CLUSTER_TERMINATED
                    && record.security_configuration_id.as_deref() == Some(configuration_id)
            })
            .min_by_key(|record| record.sequence)
    }

    /// Whether this store holds the resource `arn` names, for a request signed for
    /// `region`.
    ///
    /// The ARN is matched in FULL against the stored record's OWN `arn`, and the record
    /// has to belong to the region the request was signed for, so one that names another
    /// partition, region or account — a resource some other endpoint holds — resolves to
    /// nothing rather than to this endpoint's resource of the same id.
    fn holds_tagged_resource(&self, region: &str, resource: &TaggedResource, arn: &str) -> bool {
        let stored = match resource {
            TaggedResource::VirtualCluster { id } => self
                .virtual_cluster(region, id)
                .map(|record| record.arn.as_str()),
            TaggedResource::JobRun {
                virtual_cluster_id,
                id,
            } => self
                .job_run(region, virtual_cluster_id, id)
                .map(|record| record.arn.as_str()),
            TaggedResource::ManagedEndpoint {
                virtual_cluster_id,
                id,
            } => self
                .managed_endpoint(region, virtual_cluster_id, id)
                .map(|record| record.arn.as_str()),
        };
        stored == Some(arn)
    }

    /// The tag set of the resource `arn` names, for editing — the ONE copy that
    /// resource's record holds, so a tag written here is the tag every reader of the
    /// resource reports.
    ///
    /// `None` when this store holds no such resource, on the terms
    /// [`Self::holds_tagged_resource`] states.
    pub fn resource_tags_mut(
        &mut self,
        region: &str,
        resource: &TaggedResource,
        arn: &str,
    ) -> Option<&mut HashMap<String, String>> {
        if !self.holds_tagged_resource(region, resource, arn) {
            return None;
        }
        match resource {
            TaggedResource::VirtualCluster { id } => self
                .virtual_clusters
                .get_mut(id)
                .filter(|record| record.region == region)
                .map(|record| &mut record.tags),
            TaggedResource::JobRun {
                virtual_cluster_id,
                id,
            } => self
                .job_runs
                .get_mut(id)
                .filter(|record| {
                    record.region == region && &record.virtual_cluster_id == virtual_cluster_id
                })
                .map(|record| &mut record.tags),
            TaggedResource::ManagedEndpoint {
                virtual_cluster_id,
                id,
            } => self
                .managed_endpoints
                .get_mut(id)
                .filter(|record| {
                    record.region == region && &record.virtual_cluster_id == virtual_cluster_id
                })
                .map(|record| &mut record.tags),
        }
    }

    /// The tag set of the resource `arn` names, for reporting — the SAME copy
    /// [`Self::resource_tags_mut`] edits, so a read answers exactly what the resource's
    /// own record holds and what every other reader of that resource reports.
    ///
    /// `None` when this store holds no such resource, on the terms
    /// [`Self::holds_tagged_resource`] states.
    pub fn resource_tags(
        &self,
        region: &str,
        resource: &TaggedResource,
        arn: &str,
    ) -> Option<&HashMap<String, String>> {
        if !self.holds_tagged_resource(region, resource, arn) {
            return None;
        }
        match resource {
            TaggedResource::VirtualCluster { id } => self
                .virtual_cluster(region, id)
                .map(|record| &record.tags),
            TaggedResource::JobRun {
                virtual_cluster_id,
                id,
            } => self
                .job_run(region, virtual_cluster_id, id)
                .map(|record| &record.tags),
            TaggedResource::ManagedEndpoint {
                virtual_cluster_id,
                id,
            } => self
                .managed_endpoint(region, virtual_cluster_id, id)
                .map(|record| &record.tags),
        }
    }

    /// This cluster's stored job runs at `region`'s endpoint, in creation order: what a
    /// `ListJobRuns` against that endpoint reports on, keyed by the region as well as the
    /// cluster on the same terms as [`Self::cluster_managed_endpoints`].
    pub fn cluster_job_runs<'a>(
        &'a self,
        region: &'a str,
        virtual_cluster_id: &str,
    ) -> impl Iterator<Item = &'a JobRunRecord> {
        self.job_runs_by_cluster
            .get(virtual_cluster_id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.job_runs.get(id))
            .filter(move |record| record.region == region)
    }

    /// Whether there is room for one more security configuration.
    ///
    /// A security configuration has no retired form to reclaim (it has no status, and only
    /// a delete removes one), so at the cap the create is refused instead of growing the
    /// store without bound.
    pub fn can_add_security_configuration(&self) -> bool {
        self.security_configurations.len() < MAX_SECURITY_CONFIGURATIONS
    }

    /// Store a freshly minted security configuration together with the idempotency entry
    /// its `clientToken` earns, so a record can never be stored without it. A delete that
    /// removes the record removes that entry with it.
    pub fn insert_security_configuration(&mut self, record: SecurityConfigurationRecord) {
        self.idempotency.insert(
            (
                record.region.clone(),
                CREATE_SECURITY_CONFIGURATION_SCOPE.to_string(),
                record.client_token.clone(),
            ),
            record.id.clone(),
        );
        self.security_configurations
            .insert(record.id.clone(), record);
    }

    /// Remove a stored security configuration and the idempotency entry that named it.
    /// `None` when the id names no configuration.
    ///
    /// The entry goes WITH the record, on the same terms as [`Self::remove_job_template`]:
    /// this emulator's token index holds exactly one entry per stored configuration and so
    /// stays bounded by [`MAX_SECURITY_CONFIGURATIONS`]. A create retried with the same
    /// `clientToken` AFTER its configuration was deleted therefore mints a new one rather
    /// than replaying a record that no longer exists; while the configuration exists — the
    /// whole window the dedup contract is about — the token never mints a second one.
    pub fn remove_security_configuration(
        &mut self,
        region: &str,
        id: &str,
    ) -> Option<SecurityConfigurationRecord> {
        self.security_configuration(region, id)?;
        let record = self.security_configurations.remove(id)?;
        self.idempotency.remove(&(
            record.region.clone(),
            CREATE_SECURITY_CONFIGURATION_SCOPE.to_string(),
            record.client_token.clone(),
        ));
        Some(record)
    }

    /// The stored security configuration with this id, only when it belongs to this
    /// region.
    pub fn security_configuration(
        &self,
        region: &str,
        id: &str,
    ) -> Option<&SecurityConfigurationRecord> {
        self.security_configurations
            .get(id)
            .filter(|record| record.region == region)
    }

    /// This region's stored security configurations, in no particular order: what a
    /// `ListSecurityConfigurations` against this region's endpoint reports on.
    pub fn region_security_configurations<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a SecurityConfigurationRecord> {
        self.security_configurations
            .values()
            .filter(move |record| record.region == region)
    }

    /// The modelled `SecurityConfiguration` for this id in this region.
    pub fn security_configuration_view(
        &self,
        region: &str,
        id: &str,
    ) -> Option<SecurityConfiguration> {
        Some(
            self.security_configuration(region, id)?
                .to_security_configuration(),
        )
    }

    /// The resource id `(region, scope, client_token)` already minted, if this token is a
    /// replay of a request to this region.
    pub fn replayed_idempotency(
        &self,
        region: &str,
        scope: &str,
        client_token: &str,
    ) -> Option<&String> {
        self.idempotency.get(&(
            region.to_string(),
            scope.to_string(),
            client_token.to_string(),
        ))
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEmrcontainersAwschicagowebserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEmrcontainersAwschicagowebserviceState>)
}
