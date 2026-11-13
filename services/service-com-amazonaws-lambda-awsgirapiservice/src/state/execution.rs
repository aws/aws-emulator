// SPDX-License-Identifier: CC0-1.0

//! RUNNING a function: what an invocation of a stored deployment package comes to.
//!
//! An invocation's answer is the handler's: "For synchronous invocation, details about the
//! function response, including errors, are included in the response body and headers"
//! (`Invoke`). So this module runs the handler — the package's modules are compiled and
//! executed in this process by the engine for the function's own language ([`python`],
//! [`node`]), with the event the caller sent and the context the runtime reports — and
//! reports back exactly what the run produced: the value the handler returned, serialized
//! by that engine; the error document and `Unhandled` signal of an exception it raised; and
//! the log the run wrote. Nothing here is synthesised: a payload, a log line, an error type
//! or a duration is the one the run produced, and a package no engine here can run is
//! reported as the modelled failure for it rather than as a result.
//!
//! # Which packages run
//!
//! A `Zip` package whose `Runtime` names a language this emulator embeds an engine for
//! ([`engine_for`]) — the Python runtimes and the node ones — out of the bytes the store
//! holds for the version the invocation resolved. Every other package — a container image,
//! a runtime this emulator embeds no engine for — is [`Unrunnable::NoEngine`], which the
//! calling operation answers as the modelled `InvalidRuntimeException`: no response payload
//! is returned that no function produced.
//!
//! # What bounds a run
//!
//! Two ceilings, for two different things:
//!
//! * the RUN's own budget, which the engine spends and the guest cannot reach
//!   ([`Budget`]): the function's own `Timeout`, clamped to the range the model
//!   allows for it, plus the step, memory, log and response bounds;
//! * how long the caller waits for THE RUN once it has that memory, which is this module's
//!   ([`HOST_WAIT_CEILING`]) and is never the guest's to set: a run still going when that
//!   wait is spent is ABANDONED — it keeps its share of the envelope and counts against
//!   [`MAX_ABANDONED_RUNS`] for as long as its thread lasts, because the engine still
//!   holding that memory is the thread's, and past that cap an invocation is refused with
//!   the modelled throttle rather than adding another thread.
//!
//! Both are about the HANDLER's run, because the function's `Timeout` is. An execution
//! environment is INITIALIZED first — the engine is built, its runtime is installed and the
//! handler's module is imported — and that phase has a span of its own
//! ([`INIT_PHASE_LIMIT`]), which is waited out beside the handler's `Timeout` and charged to
//! neither the deadline the run is stopped at nor the time the handler is told it has left.
//!
//! WHICH invocations pay that phase is the engine's. Under the Python interpreter an
//! environment is initialized once per [`warm_pool::EnvironmentKey`] and then serves
//! invocation after invocation, exactly as Lambda keeps one: the invocation that finds no
//! environment of its own standing pays the initialization, and every invocation a standing
//! environment serves is the handler's run alone ([`warm_pool`]). Under the node engine every
//! invocation still initializes an environment of its own, because that runtime settles an
//! invocation inside the context it was started in.
//!
//! The engines of the runs in flight hold at most [`MAX_CONCURRENT_RUN_MEMORY_MB`] of memory
//! between them, so what the handlers of this process take of the host is bounded however
//! many invocations arrive. That envelope is a RESOURCE bound of this process and not a
//! concurrency limit of the API: an invocation the function's reservation and the account's
//! capacity admit is one Lambda SERVES ([`super::execution_slot`]), so a caller that finds
//! the memory held QUEUES for its own share of it — the `MemorySize` its configuration asks
//! for ([`run_memory_claim_mb`]) — for as long as [`PERMIT_WAIT`], the longest a run ahead of
//! it can hold a share, and only an envelope still full after that is answered with the
//! modelled throttle. Every run happens on a thread of its own with no state guard of this
//! service held, so one caller's handler cannot stall another caller's request, and the
//! smallest share a run can hold is what bounds those threads to [`MAX_CONCURRENT_RUNS`].
//!
//! An execution environment that is STANDING holds its engine's memory outside that envelope,
//! because the share a run claimed goes back when the invocation is answered: what the
//! environments kept for the next invocation hold is bounded by the same figure again, and by
//! their number ([`warm_pool`]).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

/// The modules of a .zip deployment package, read out of the bytes the store holds.
pub mod package;

/// What one run may spend, and what it came to: the same for every engine.
pub mod outcome;
pub use outcome::{Budget, Completion, Finished, Stop};

/// The clock a run's time-based bounds are read against, which the HOST owns.
pub mod run_clock;
pub use run_clock::Clock;

/// The Python interpreter one invocation runs in, and everything that bounds and contains
/// it.
pub mod python;

/// The execution ENVIRONMENTS this process keeps standing, which an invocation of an
/// already-initialized one is served by.
pub mod warm_pool;

/// The JavaScript engine one invocation runs in, and everything that bounds and contains
/// it.
pub mod node;

/// The runtime the handler runs under, in the language the handler is written in.
mod bootstrap;

/// The AWS SDK call seam a running handler reaches the rest of this emulator through.
pub mod aws_api;
pub use aws_api::AwsAccess;

/// What an SDK client fills into a call's input itself: the members the called service's model
/// marks as an idempotency token.
mod idempotency_token;

/// The HTTP seam a running handler reaches it through when it signs and sends a request of
/// its own.
pub mod guest_http;

/// The TLS layer of the standard library a running Python handler imports, supplied beside
/// the socket interface it is built on.
pub mod guest_tls;

/// The Amazon S3 half of that seam, whose own protocol binds a request per operation.
mod aws_s3;

/// The CloudFront half of it, for the same reason: that service's own `restXml` protocol
/// binds a request per operation rather than carrying an input document in a JSON body.
mod aws_cloudfront;

/// The Amazon SNS half of it, for the same reason: that service's model declares `awsQuery`,
/// so a request to it is a form-encoded action and its answer is an XML document rather than
/// the input and output documents a JSON protocol carries.
mod aws_sns;

/// What every half of that seam bound by an AWS SDK CLIENT shares: how a call's input document
/// is read, what the answer carried, and how a refusal is handed back.
mod sdk_call;

/// Binding one of those calls to HTTP for a service whose protocol is `restJson1`.
pub mod rest_json;

/// WHICH values of an answered call carry BYTES, which the called service's own model says.
pub mod blob_members;

/// The AUTHENTICATION TOKEN the SDK mints in the CLIENT for a handler connecting to a
/// database with IAM database authentication, which is not an API call at all.
pub mod db_auth_token;

/// The credential a running handler's own calls are made with: a session of the function's
/// execution role.
pub mod execution_role_session;
pub use execution_role_session::{function_sdk_access, ExecutionRoleSessions};

/// The AWS SDK modules a Python handler imports, which that seam is reached through.
mod aws_sdk_guest;

/// Delivering one invocation's log to CloudWatch Logs, which is where a client reads it.
pub mod log_delivery;
pub use log_delivery::{log_target, LogEvent, LogGrants, LogTarget};

/// The AWS SDK modules a node handler imports, reached through that same seam.
mod node_sdk_guest;

/// What a called service's own Smithy model states for an SDK client built against it: the
/// names one client package exports, the error shapes an SDK's own exception classes are built
/// from, and the input members a client fills in itself. It sits under the node client because
/// that is the SDK that first read it; every SDK this runtime supplies reads the one reading.
use node_sdk_guest::exports as sdk_model;

/// How many LAYER archives one run reads.
///
/// AWS's own quota is five layers per function, so a configuration naming more than that
/// has more layers than a function may have. The cap is applied in HOST code before an
/// archive is read, so the number of archives one invocation holds and inflates does not
/// grow with the list a caller wrote — the same ARN repeated is still one archive per entry.
pub const MAX_RUN_LAYER_ARCHIVES: usize = 5;

/// The archives of the layers one function's configuration holds, in the order it names
/// them, at most [`MAX_RUN_LAYER_ARCHIVES`] of them.
///
/// A layer entry this emulator holds no archive bytes for contributes nothing: the layer
/// root simply does not hold its files, which is what a handler importing from it sees. The
/// bytes are SHARED rather than copied — a published layer version is immutable — so an
/// invocation holds one reference per entry and not one copy.
#[must_use]
pub fn layer_archives(layers: &[super::FunctionLayer]) -> Vec<Arc<Vec<u8>>> {
    layers
        .iter()
        .take(MAX_RUN_LAYER_ARCHIVES)
        .filter_map(|layer| layer.zip_bytes.clone())
        .collect()
}

/// The `posix` module a run's Python is built on: this emulator's own, with no host call
/// in it.
mod host_posix;

/// Whether the key a function's environment variables are encrypted with can decrypt them
/// — the check every invocation of such a function begins with.
pub mod environment_key;
pub use environment_key::{durable_data_key_fault, environment_key_fault, EnvironmentKeyFault};

/// The X-Ray trace context one invocation runs with, which the execution environment
/// reports in `_X_AMZN_TRACE_ID`.
pub mod trace_context;
pub use trace_context::{invocation_trace_context, supplied_trace_header};
use trace_context::report_segments;

/// The X-Ray daemon a traced invocation reports its own work to, and the datagram surface a
/// handler reaches it through.
pub mod xray_daemon;

/// The engine memory the runs of this process may hold TOGETHER, stated in the MB a
/// function's `MemorySize` is stated in.
///
/// This is the AGGREGATE bound on what the handlers of this process can take of the host: one
/// run holds at most the memory its own configuration asks for, clamped to this module's
/// per-run ceiling ([`run_heap_bytes`], at most [`MAX_RUN_MEMORY_BYTES`]), and the runs in
/// flight hold at most this between them — so what the engines hold does not grow with the
/// number of callers, however many invocations arrive. It bounds the resource a run really
/// costs rather than a COUNT of runs that ignores it: four runs of the largest per-run heap
/// is the same envelope as sixteen of the smallest `MemorySize` the model allows, and both
/// are what this process can hold at once.
pub const MAX_CONCURRENT_RUN_MEMORY_MB: u32 = 4 * PER_RUN_MEMORY_CEILING_MB;

/// The per-run ceiling [`MAX_RUN_MEMORY_BYTES`] states, in those same MB.
const PER_RUN_MEMORY_CEILING_MB: u32 = (MAX_RUN_MEMORY_BYTES / BYTES_PER_MB) as u32;

/// How long an invocation waits for its share of that envelope before it is throttled.
///
/// As long as a run AHEAD of it is being answered: such a run holds its share for the
/// initialization of the execution environment it happens in ([`INIT_PHASE_LIMIT`]) and for
/// the handler's own run, whose wait is bounded by [`HOST_WAIT_CEILING`], plus the
/// [`ABANDON_GRACE`] its caller then gives it. So a caller that arrives while the memory is
/// held is served as soon as a handler ahead of it ends, and only an envelope still full
/// after that span is answered the modelled throttle. A run that outlived its own caller
/// keeps its share until its thread ends, because the engine still holding that memory is
/// that thread's, and it counts against [`MAX_ABANDONED_RUNS`] for just as long.
pub const PERMIT_WAIT: Duration = INIT_PHASE_LIMIT
    .saturating_add(HOST_WAIT_CEILING)
    .saturating_add(ABANDON_GRACE);

/// How many handlers run at once, at most, which FOLLOWS from that envelope: every function
/// has at least the smallest `MemorySize` the model allows ([`MIN_MEMORY_SIZE_MB`]), so no
/// more runs than this fit in the envelope however small the functions are — which is also
/// what bounds the run threads and the stacks they hold ([`RUN_STACK_BYTES`]).
pub const MAX_CONCURRENT_RUNS: u32 = MAX_CONCURRENT_RUN_MEMORY_MB / (MIN_MEMORY_SIZE_MB as u32);

/// How many invocations may hold their mounts INFLATED at once: as many as the run envelope
/// admits runs of the smallest function ([`MAX_CONCURRENT_RUNS`]).
///
/// A deployment package and the layers beside it are the caller's own bytes, expanded in
/// memory before the handler can be loaded ([`package`]), so the number of invocations doing
/// that at one instant is the host's to decide and not the caller's. Each read is itself
/// bounded by [`package::ReadBudget`], so this bounds the product: at most this many
/// invocations × that budget is inflated at once. The permit is held for as long as those
/// EXPANDED bytes are live, which is the whole run, so a queue of invocations waiting for
/// memory cannot hold expanded archives nothing accounts for. It is a pool of its own rather
/// than a claim on the run envelope because the two bound different bytes, and because an
/// invocation whose package this emulator cannot read is answered as that without ever
/// waiting for memory it would never use.
#[must_use]
pub fn max_concurrent_mount_reads() -> usize {
    usize::try_from(MAX_CONCURRENT_RUNS).unwrap_or(usize::MAX)
}

/// How much of that envelope one invocation of a function configured for `memory_size` MB
/// holds while it runs: the memory its engine is really given ([`run_heap_bytes`]), in MB, and
/// never more than the whole envelope.
///
/// So a function asking for the smallest memory the model allows leaves room for
/// [`MAX_CONCURRENT_RUNS`] runs beside it, and one asking for the per-run ceiling leaves room
/// for three: the concurrency the substrate serves is what the memory in flight allows.
#[must_use]
pub fn run_memory_claim_mb(memory_size: i32) -> u32 {
    let bytes = u64::try_from(run_heap_bytes(memory_size)).unwrap_or(u64::MAX);
    let megabytes = u32::try_from(bytes / BYTES_PER_MB).unwrap_or(u32::MAX);
    megabytes.clamp(1, MAX_CONCURRENT_RUN_MEMORY_MB)
}

/// The longest a CALLER waits for the RUN of a handler, whatever the function's own timeout
/// says.
///
/// This is the HOST's ceiling, not the function's: a `Timeout` of up to 900 seconds would
/// otherwise hold a request open for a quarter of an hour. The run's own deadline is the
/// function's clamped `Timeout` ([`run_budget`]); this is only how long the answer is
/// waited for, from the moment this invocation holds the memory to run in — beside the
/// initialization of the execution environment the handler is called in, which is waited for
/// on its own span ([`INIT_PHASE_LIMIT`]) because the function's `Timeout` does not bound it.
pub const HOST_WAIT_CEILING: Duration = Duration::from_secs(30);

/// How long the EXECUTION ENVIRONMENT one invocation runs in may take to be initialized,
/// which the function's own `Timeout` does not bound.
///
/// An invocation happens in two phases, and the `Timeout` — "the amount of time that Lambda
/// allows a function to run before stopping it" — bounds the second of them: the INIT phase
/// creates the execution environment, loads the runtime and runs the code outside the
/// handler, and AWS states that phase "is limited to 10 seconds"; the INVOKE phase that
/// follows it is the handler's own run. This is the span that setup is allowed, over and above
/// the `Timeout` the handler itself is given.
///
/// It bounds ONE initialization, not every invocation: under the Python interpreter the
/// environment is built for the invocation that finds none standing and then serves the
/// invocations after it, each of which is the handler's run alone ([`warm_pool`]); under the
/// node engine every invocation initializes an environment of its own and so is bounded by
/// this span as well as by its `Timeout`.
pub const INIT_PHASE_LIMIT: Duration = Duration::from_secs(10);

/// How long after its budget is spent a run is still waited for before it is abandoned.
pub const ABANDON_GRACE: Duration = Duration::from_secs(2);

/// How many abandoned runs may be outstanding before an invocation is throttled instead of
/// starting another.
pub const MAX_ABANDONED_RUNS: usize = 8;

/// The stack one run's thread is given. Well above the interpreter's clamped recursion
/// limit, so deep guest recursion ends that run with the interpreter's own
/// `RecursionError` rather than touching the thread's guard page.
pub const RUN_STACK_BYTES: usize = 16 * 1024 * 1024;

/// The smallest `Timeout` the model allows (`smithy_query(query="shape",
/// shape="Timeout")`: `@range(min: 1)`), which a stored value is clamped up to before it
/// bounds a run.
const MIN_TIMEOUT_SECONDS: i64 = 1;

/// The largest `Timeout` the model allows for a function (AWS publishes 900 seconds, which
/// is the maximum `CreateFunction` accepts), which a stored value is clamped down to.
const MAX_TIMEOUT_SECONDS: i64 = 900;

/// The smallest `MemorySize` the model allows (`smithy_query(query="shape",
/// shape="MemorySize")`: `@range(min: 128, max: 32768)`), which a stored value is clamped
/// up to before it bounds a run.
const MIN_MEMORY_SIZE_MB: i64 = 128;

/// The largest `MemorySize` that same range allows, which a stored value is clamped down
/// to.
const MAX_MEMORY_SIZE_MB: i64 = 32_768;

/// The bytes of one MB of a function's `MemorySize`, which the value is stated in.
const BYTES_PER_MB: u64 = 1024 * 1024;

/// The REQUEST ID one invocation is identified by, in the log it writes and in the error
/// document it answers with: a fresh UUID per invocation, which is what `aws_request_id`
/// is on the real runtime.
#[must_use]
pub fn mint_request_id() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// How many traced events one run may execute. Large enough for any handler a test or a
/// client runs, small enough that a loop with no exit ends inside the caller's wait.
const MAX_RUN_STEPS: u64 = 20_000_000;

/// How much memory one run may add to this process.
const MAX_RUN_MEMORY_BYTES: u64 = 512 * 1024 * 1024;

/// How deep one run may recurse.
///
/// The interpreter's own default is far LOWER than Python's (256 against CPython's 1000),
/// and the standard library's own import chain needs more than that, so this is the limit
/// a run is given rather than a reduction: it is set before any guest code runs and the
/// call that would raise it is taken away, so it is the ceiling for the whole run.
const MAX_RUN_RECURSION: usize = 2_000;

/// How much of a run's output is kept. The model reports "The last 4 KB of the execution
/// log" on `LogResult`, so what is kept is a multiple of that rather than unbounded.
const MAX_RUN_LOG_BYTES: usize = 256 * 1024;

/// How large a handler's response may be, which is the quota Lambda names in its own
/// refusal for a response past it ([`RESPONSE_SIZE_TOO_LARGE`]) — a response of exactly
/// this many bytes is delivered, and one byte more is not.
const MAX_RESPONSE_BYTES: usize = 6_291_556;

/// The bytes of a run's log the model reports on `LogResult`: "The last 4 KB of the
/// execution log, which is base64-encoded."
pub const LOG_TAIL_BYTES: usize = 4 * 1024;

/// The `FunctionError` an invocation whose handler raised reports: "If present, indicates
/// that an error occurred during function execution." AWS reports the error of a handler
/// that raised as `Unhandled`.
pub const FUNCTION_ERROR_UNHANDLED: &str = "Unhandled";

/// The `Runtime` values this emulator runs with the Python interpreter.
///
/// Matched by prefix because the identifiers are versioned (`python3.12`,
/// `python3.13`, …) and the interpreter runs the language rather than one of its point
/// releases (source for the value set: `smithy_query(query="shape", shape="Runtime")`).
const PYTHON_RUNTIME_PREFIX: &str = "python";

/// The `Runtime` values this emulator runs with the JavaScript engine, by the same rule:
/// every `nodejs…` identifier of that same value set names the one language.
const NODE_RUNTIME_PREFIX: &str = "nodejs";

/// Which engine runs a function's handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// The Python interpreter ([`python`]).
    Python,
    /// The JavaScript engine ([`node`]).
    Node,
}

/// The engine that runs a package for `runtime`, or `None` when this emulator embeds none
/// for it.
#[must_use]
pub fn engine_for(runtime: Option<&str>) -> Option<Engine> {
    let runtime = runtime?;
    if runtime.starts_with(PYTHON_RUNTIME_PREFIX) {
        return Some(Engine::Python);
    }
    if runtime.starts_with(NODE_RUNTIME_PREFIX) {
        return Some(Engine::Node);
    }
    None
}

/// Whether `runtime` names a runtime this emulator can run a package for.
#[must_use]
pub fn runs_handler(runtime: Option<&str>) -> bool {
    engine_for(runtime).is_some()
}

/// Everything one invocation of a stored function needs to run.
#[derive(Debug, Clone)]
pub struct Invocation {
    /// The function's name.
    pub function_name: String,
    /// The version the invocation resolved to — what `ExecutedVersion` reports.
    pub executed_version: String,
    /// The ARN the invocation addressed, as `context.invoked_function_arn`.
    pub invoked_function_arn: String,
    /// The Region the invocation was addressed to.
    pub region: String,
    /// The `Runtime` of the resolved configuration.
    pub runtime: Option<String>,
    /// The `Handler` of the resolved configuration.
    pub handler: Option<String>,
    /// The REVISION the resolved configuration is at (`FunctionConfig::revision_id`), which
    /// changes whenever the function's code or configuration does.
    ///
    /// It is what says WHICH execution environments may serve this invocation
    /// ([`warm_pool::EnvironmentKey`]): an environment holds the handler module and the
    /// environment variables of one revision, so an invocation that arrives after a change
    /// is never served by one built before it.
    pub code_revision: String,
    /// The function's own environment variables.
    pub variables: BTreeMap<String, String>,
    /// The function's `MemorySize`, in MB.
    pub memory_size: i32,
    /// The ARCHITECTURE the function is configured for (`Architectures`), which is what
    /// the runtime reports as its own.
    pub architecture: String,
    /// The function's `Timeout`, in seconds.
    pub timeout: i32,
    /// The deployment package bytes of the resolved version, when this emulator holds
    /// them.
    pub zip_bytes: Option<Vec<u8>>,
    /// The archives of the LAYERS the resolved configuration holds, in the order the
    /// configuration names them — the content a managed runtime mounts under the layer
    /// root, where a later layer's entry wins over an earlier one's. Shared rather than
    /// copied: a published layer version is immutable.
    pub layer_zips: Vec<Arc<Vec<u8>>>,
    /// The event payload the caller sent.
    pub payload: Vec<u8>,
    /// The `ClientContext` the caller sent, already decoded from base64 to its JSON text.
    pub client_context: Option<String>,
    /// The `TenantId` the caller sent.
    pub tenant_id: Option<String>,
    /// This invocation's request id.
    pub request_id: String,
    /// The X-Ray TRACE CONTEXT this invocation runs with, which the run reports in
    /// `_X_AMZN_TRACE_ID` and records its own segments under ([`trace_context`]), or `None`
    /// for an invocation that has none.
    pub trace: Option<trace_context::InvocationTrace>,
    /// What the run reports as `AWS_LAMBDA_INITIALIZATION_TYPE`: which kind of execution
    /// environment is serving this invocation, as the admission that let it in decided
    /// ([`super::execution_slot`]).
    pub initialization_type: &'static str,
    /// How the handler reaches the rest of this emulator through the AWS SDK this runtime
    /// supplies it ([`aws_api`]), or `None` where there is no seam to dispatch a call on.
    pub aws: Option<AwsAccess>,
    /// Where this invocation's log is delivered and what the function's execution role may
    /// do there ([`log_delivery`]), or `None` for a run whose caller resolved no target —
    /// in which case the run's log is still its answer's log tail and simply reaches no log
    /// group.
    pub logs: Option<LogTarget>,
    /// The AWS/Lambda series this invocation is metered on once it has RUN — what its run
    /// took, and the failure it came to ([`super::metrics::InvocationMetrics`]) — or `None`
    /// for a run metered nowhere.
    ///
    /// It is carried by the invocation rather than read at each caller, so that every path
    /// that runs a handler meters the run the same way: a synchronous call, an event
    /// delivered off the response path, a response-streaming call and a function URL
    /// request alike.
    pub metrics: Option<super::metrics::InvocationMetrics>,
}

/// Why an invocation ran nothing at all.
///
/// Each of these is a condition the CALLING operation answers as one of its own modelled
/// errors: nothing here is reported as a result of a run that did not happen.
#[derive(Debug, Clone)]
pub enum Unrunnable {
    /// This emulator embeds no engine for the function's runtime or package type.
    NoEngine {
        /// What the function's code is, as the configuration names it.
        runtime: String,
    },
    /// The function's configuration names no handler to run.
    NoHandler,
    /// This emulator holds no bytes for the resolved version's package.
    PackageNotHeld,
    /// The bytes it holds are not a readable archive.
    PackageUnreadable(String),
    /// Too many handlers are already running, or too many runs have been abandoned.
    Overloaded(String),
    /// The run could not be started or driven at all. Never a guest failure.
    Internal(String),
}

/// What one invocation came to.
#[derive(Debug, Clone)]
pub struct Invoked {
    /// The response payload: what the handler returned, or the error document for the
    /// failure it came to.
    pub payload: Vec<u8>,
    /// The `FunctionError` to report, when the run failed.
    pub function_error: Option<String>,
    /// The whole log of the run.
    pub log: String,
    /// That same log as the events CloudWatch Logs holds it as, one per line
    /// ([`log_delivery`]).
    pub log_events: Vec<LogEvent>,
    /// How long the run took.
    pub duration: Duration,
    /// The WRITES a response-streaming handler made, in order — empty for a handler that
    /// returned a value instead. A streaming caller is delivered one chunk per entry, so
    /// the chunk boundaries it sees are the handler's own.
    pub chunks: Vec<Vec<u8>>,
    /// The error document of a failure a response-streaming handler came to AFTER it had
    /// already written, where [`Invoked::payload`] is the bytes it had written rather than
    /// that document.
    ///
    /// Absent for every other run, where the payload IS the document; see
    /// [`Invoked::error_document`].
    pub stream_failure: Option<Vec<u8>>,
}

impl Invoked {
    /// The error document this run failed with, or `None` for a run that did not fail.
    ///
    /// For almost every run the response payload IS that document, which is what an
    /// unhandled error answers with. A RESPONSE-STREAMING run is the exception: its payload
    /// is the bytes the handler had already written, so the document is kept beside them
    /// ([`Invoked::stream_failure`]) and is what the stream's trailer reports as the details
    /// of the error that cut the response short.
    #[must_use]
    pub fn error_document(&self) -> Option<String> {
        self.function_error.as_ref().map(|_| {
            let document = self.stream_failure.as_ref().unwrap_or(&self.payload);
            String::from_utf8_lossy(document).to_string()
        })
    }

    /// The `LogResult` for this run: "The last 4 KB of the execution log, which is
    /// base64-encoded."
    #[must_use]
    pub fn log_tail(&self) -> String {
        let bytes = self.log.as_bytes();
        let from = bytes.len().saturating_sub(LOG_TAIL_BYTES);
        // Start the tail on a character boundary, so what is reported is still text.
        let mut start = from;
        while start < bytes.len() && !self.log.is_char_boundary(start) {
            start += 1;
        }
        ::base64::Engine::encode(
            &::base64::engine::general_purpose::STANDARD,
            &bytes[start..],
        )
    }
}

/// Run `invocation`'s handler and report what it came to.
///
/// The run happens on a thread of its own, holding its own share of this module's run memory
/// and no state guard of this service, and the caller's wait for it is bounded by
/// [`HOST_WAIT_CEILING`] however long the function's own timeout is.
pub async fn run_invocation(mut invocation: Invocation) -> Result<Invoked, Unrunnable> {
    let Some(engine) = engine_for(invocation.runtime.as_deref()) else {
        return Err(Unrunnable::NoEngine {
            runtime: invocation
                .runtime
                .clone()
                .unwrap_or_else(|| "an image package".to_string()),
        });
    };
    let Some(handler) = invocation.handler.clone() else {
        return Err(Unrunnable::NoHandler);
    };
    // The AWS SDK a managed runtime ships reads the invocation's own tracing context out of
    // the environment and stamps it on every request it makes, so the seam the handler calls
    // through carries this invocation's header ([`aws_api::AwsAccess::tracing`]). That is
    // what makes the call a handler makes a HOP OF THIS CHAIN to the service that serves it,
    // which is what the recursive-loop guardrail counts (`super::recursion`).
    if let Some(header) = invocation.trace.as_ref().map(|trace| trace.header.clone()) {
        invocation.aws = invocation.aws.take().map(|aws| aws.tracing(header));
    }

    // A host already holding as many runs past their budget as it keeps threads for starts
    // no further one, and is answered BEFORE a byte of the caller's archive is expanded: a
    // throttled invocation inflates nothing.
    if abandoned_runs() >= MAX_ABANDONED_RUNS {
        // The capacity that is full is this host's, which stands in for the account's
        // concurrent executions, so what the caller is told is the modelled throttle for
        // that pool and the detail of which host ceiling it was stays in the log.
        //
        // The reading is taken before this run is admitted rather than claimed against the
        // ceiling: what bounds the runs in flight is the memory envelope below, which every
        // one of them holds a share of for as long as its thread lasts, abandoned or not.
        // So the most runs that can be outstanding past their wait at one instant is what
        // that envelope admits, each of them accounted for in it.
        ::tracing::warn!(
            abandoned = abandoned_runs(),
            ceiling = MAX_ABANDONED_RUNS,
            "as many runs of this emulator are outstanding past their budget as it keeps \
             threads for, so this invocation was throttled rather than started"
        );
        // The caller is told its request was throttled, so the request is metered as one
        // the service throttled (`super::metrics`), exactly as an invocation the function's
        // own concurrency pool had no room for is.
        throttled_request(&invocation);
        return Err(Unrunnable::Overloaded(
            super::execution_slot::THROTTLED_INVOCATION_MESSAGE.to_string(),
        ));
    }

    // The archives this invocation reads are INFLATED here, under a pool of its own: the
    // package and layer bytes are the caller's, so how many of them this process holds
    // expanded at once is bounded by [`max_concurrent_mount_reads`] rather than by how fast
    // invocations arrive. The permit is a pool of its own rather than a claim on the run
    // envelope — so a package this emulator cannot read at all is answered as that without
    // ever waiting for memory it would never use — and each archive is itself read under
    // [`package::ReadBudget`]'s entry and byte ceilings. It is held for as long as the
    // EXPANDED bytes are live, which is the whole run: the permit moves onto the run's own
    // thread below, so the number of inflated archive sets this process holds is what the
    // pool admits however many invocations queue behind them.
    let reading = ::tokio::time::timeout(
        PERMIT_WAIT,
        Arc::clone(mount_readers()).acquire_owned(),
    )
    .await
    .map_err(|_| {
        Unrunnable::Overloaded(
            "Too many invocations are reading their deployment packages in this emulator. \
             Retry the request."
                .to_string(),
        )
    })?
    .map_err(|error| Unrunnable::Internal(format!("the mount readers are gone: {error}")))?;

    // Each engine imports the sources of its own language: the entries a package holds for
    // the other one are not modules this run could load. One budget is spent across every
    // mount this invocation reads ([`package::ReadBudget`]), whichever engine reads them,
    // which is what bounds this read. It happens BEFORE any capacity is claimed, so a
    // package this emulator cannot run at all is answered as that at once rather than after
    // a wait for memory it will never use.
    let mut read_budget = package::ReadBudget::default();
    let read = match engine {
        Engine::Python => {
            package::python_modules_within(invocation.zip_bytes.as_deref(), &mut read_budget)
        }
        Engine::Node => package::javascript_modules_within(
            invocation.zip_bytes.as_deref(),
            &mut read_budget,
        ),
    };
    let mut modules = match read {
        Ok(modules) => modules,
        Err(package::PackageFault::NotHeld) => return Err(Unrunnable::PackageNotHeld),
        Err(package::PackageFault::Unreadable(why)) => {
            return Err(Unrunnable::PackageUnreadable(why))
        }
    };
    // The layers' own entries, under the prefix the runtime keys the layer root by, read
    // out of the SAME budget the deployment package was: what one invocation holds is
    // bounded across its mounts rather than per archive. A layer this emulator holds no
    // readable archive for mounts nothing, exactly as a deployment package it holds no
    // bytes for imports nothing.
    if engine == Engine::Node {
        for archive in &invocation.layer_zips {
            if let Ok(entries) =
                package::javascript_modules_within(Some(archive.as_slice()), &mut read_budget)
            {
                modules.extend(entries.into_iter().map(|entry| package::PackageModule {
                    path: format!("{}{}", node::LAYER_ID_PREFIX, entry.path),
                    source: entry.source,
                }));
            }
        }
    }
    // The modules the runtime itself supplies on the search path, which is where the AWS
    // SDK a managed node runtime ships is found.
    if engine == Engine::Node {
        modules.extend(node_sdk_guest::runtime_modules());
    }

    // This run's share of the engine memory the process allows its runs together
    // ([`MAX_CONCURRENT_RUN_MEMORY_MB`]), waited for as long as [`PERMIT_WAIT`] — the longest
    // a run ahead of this one can hold a share — so a caller that arrives while the memory is
    // held is served once a handler ahead of it ends, and only an envelope still full after
    // that is answered the modelled throttle. Held for as long as the RUN is, which is not
    // the same as how long its CALLER waits: the share moves onto the run's own thread below
    // and goes back to the envelope when that thread ends, so a run the caller gave up on
    // still holds the memory its engine is still really using and what the handlers of this
    // process hold together stays inside the envelope.
    let claim = run_memory_claim_mb(invocation.memory_size);
    let permit = ::tokio::time::timeout(
        PERMIT_WAIT,
        Arc::clone(run_memory()).acquire_many_owned(claim),
    )
    .await
    .map_err(|_| {
        ::tracing::warn!(
            claimed_mb = claim,
            envelope_mb = MAX_CONCURRENT_RUN_MEMORY_MB,
            waited_seconds = PERMIT_WAIT.as_secs(),
            "the engine memory this emulator runs handlers in did not come free inside the \
             caller's own wait, so this invocation was throttled rather than started"
        );
        throttled_request(&invocation);
        Unrunnable::Overloaded(super::execution_slot::THROTTLED_INVOCATION_MESSAGE.to_string())
    })?
    .map_err(|error| Unrunnable::Internal(format!("the run memory is gone: {error}")))?;

    let budget = run_budget(invocation.timeout, invocation.memory_size);
    let heap_bytes = run_heap_bytes(invocation.memory_size);
    let started = SystemTime::now();
    let request_id = invocation.request_id.clone();
    let reported = invocation.clone();
    let (sender, receiver) = ::tokio::sync::oneshot::channel();
    let lifecycle = Arc::new(AtomicU8::new(RUN_RUNNING));
    let thread_lifecycle = Arc::clone(&lifecycle);
    let environment = runtime_environment(&invocation, started);
    // WHICH execution environments this invocation may be served in: the ones built for this
    // function version at this revision, in this Region, for this tenant and for this kind of
    // initialization ([`warm_pool`]). An invocation that finds one of them standing is served
    // in it and pays no initialization; one that finds none builds its own.
    let environment_key = warm_pool::EnvironmentKey {
        function_name: invocation.function_name.clone(),
        executed_version: invocation.executed_version.clone(),
        region: invocation.region.clone(),
        code_revision: invocation.code_revision.clone(),
        runtime: invocation.runtime.clone().unwrap_or_default(),
        handler: handler.clone(),
        memory_size: invocation.memory_size,
        architecture: invocation.architecture.clone(),
        initialization_type: invocation.initialization_type,
        // An invocation is served only in an environment built for the tenant it names, so
        // nothing a handler left in the interpreter for one tenant is reachable from
        // another's invocation.
        tenant_id: invocation.tenant_id.clone(),
    };
    let spawned = ::std::thread::Builder::new()
        .name("lambda-invocation".to_string())
        .stack_size(RUN_STACK_BYTES)
        .spawn(move || {
            // The two claims this run holds for as long as its own memory is live, moved
            // ONTO this thread so they go back when the thread ends rather than when the
            // caller stops waiting: its share of the run envelope, and its place in the
            // pool that bounds how many invocations hold their mounts expanded at once —
            // the modules below are those expanded bytes, and they live exactly as long as
            // this thread does.
            let _envelope = permit;
            let _mounts = reading;
            // The lifecycle is marked by a GUARD, so the mark and the release of an
            // abandoned run's count happen on every exit of this thread — including an
            // unwind out of the interpreter, which would otherwise leave the count
            // claimed for a thread that is no longer there.
            let _mark = ThreadLifecycle {
                lifecycle: thread_lifecycle,
            };
            let event = event_text(&invocation.payload);
            let finished = match engine {
                // The run happens in an execution environment of this function version
                // ([`warm_pool::serve`]): the one already standing when there is one, which
                // is what makes a warm invocation the handler's own run and nothing besides.
                Engine::Python => warm_pool::serve(
                    &environment_key,
                    warm_pool::Served {
                        modules: Arc::new(modules),
                        handler: handler.clone(),
                        environment,
                        event,
                        request_id: invocation.request_id.clone(),
                        invoked_function_arn: invocation.invoked_function_arn.clone(),
                        client_context: invocation.client_context.clone(),
                        tenant_id: invocation.tenant_id.clone(),
                        budget,
                        aws: invocation.aws.clone(),
                    },
                ),
                Engine::Node => node::run(&node::Run {
                    modules: &modules,
                    handler: &handler,
                    runtime: invocation.runtime.as_deref().unwrap_or_default(),
                    architecture: &invocation.architecture,
                    environment: &environment,
                    event: event.as_deref(),
                    request_id: &invocation.request_id,
                    invoked_function_arn: &invocation.invoked_function_arn,
                    client_context: invocation.client_context.as_deref(),
                    tenant_id: invocation.tenant_id.as_deref(),
                    budget,
                    heap_bytes,
                    aws: invocation.aws.as_ref(),
                }),
            };
            let _ = sender.send(finished);
        });
    if let Err(error) = spawned {
        return Err(Unrunnable::Internal(format!(
            "the invocation thread could not be started: {error}"
        )));
    }

    // What the caller waits for is the INIT phase AND the handler's own run: the function's
    // `Timeout` bounds the handler, and the execution environment the handler runs in is
    // built, loaded and handed the package before the handler is called at all
    // ([`INIT_PHASE_LIMIT`]). A wait of the `Timeout` alone would give up on a run whose
    // handler had not been reached yet — a failure the function's configuration never asked
    // for — so the initialization is waited out beside it and the ceiling on the whole wait
    // moves with it.
    let wait = budget
        .wall_clock
        .saturating_add(INIT_PHASE_LIMIT)
        .saturating_add(ABANDON_GRACE)
        .min(
            HOST_WAIT_CEILING
                .saturating_add(INIT_PHASE_LIMIT)
                .saturating_add(ABANDON_GRACE),
        );
    // The accounting of a run that outlives the caller is a GUARD rather than a branch, so
    // it happens however this invocation ends: the wait running out, the caller's request
    // being cancelled while it is suspended (which drops this future at the await below),
    // or the answer arriving. Without it a cancelled invocation would hand its slot back
    // and leave an uncounted thread running, which is no cap at all.
    let accounting = RunAccounting {
        lifecycle: Arc::clone(&lifecycle),
    };
    let waited = ::tokio::time::timeout(wait, receiver).await;
    drop(accounting);
    let finished = match waited {
        Ok(Ok(finished)) => finished,
        Ok(Err(_)) => {
            return Err(Unrunnable::Internal(
                "the invocation thread ended without reporting".to_string(),
            ))
        }
        Err(_) => {
            // The run is still going after its budget and the grace on top of it. It has
            // been given up on: the guard above counted the run as abandoned until its
            // thread ends, and its share of the run envelope stays claimed for exactly as
            // long, because the engine still holding that memory is the thread's.
            return Err(Unrunnable::Internal(format!(
                "the invocation was given up on after {} seconds",
                wait.as_secs()
            )));
        }
    };

    let invoked = answered(finished, &request_id, &reported, budget, engine, started);
    // What the invocation itself is metered by, once it has run: the time this run took and
    // the failure it came to, on the series Lambda publishes about its own invocations
    // (`super::metrics`). Published off the answer — a metric is not part of what the caller
    // is told — and for a run that HAPPENED alone: a request that ran nothing measured no
    // duration and came to no function error.
    if let Some(metrics) = reported.metrics.as_ref() {
        metrics.publish_outcome(invoked.duration, invoked.function_error.is_some());
    }
    // What X-Ray records for this invocation, where the invocation is one it records: the
    // two segments a Lambda invocation produces ([`trace_context`]), submitted through the
    // same seam the handler's own datagrams are. Reporting a trace is NOT part of answering
    // the invocation: a submission that could not be made is traced where the delivery of a
    // run's log is, and the caller is answered either way.
    if let (Some(aws), Some(recorded)) = (
        reported.aws.as_ref(),
        reported
            .trace
            .as_ref()
            .and_then(|trace| trace.recorded.as_ref()),
    ) {
        let documents = trace_context::segment_documents(
            recorded,
            &reported.function_name,
            started,
            started + invoked.duration,
            invoked.function_error.is_some(),
        );
        report_segments(aws, &reported.region, documents).await;
    }
    // The log an invocation wrote is not part of its answer: Lambda delivers it to the
    // function's own log group, which is where a client reads it ([`log_delivery`]). The
    // delivery is made as the function's execution role and before this invocation is
    // answered, so a client reading the group straight after the call finds the run's lines.
    if let (Some(aws), Some(target)) = (reported.aws.as_ref(), reported.logs.as_ref()) {
        log_delivery::deliver(
            aws,
            target,
            &reported.region,
            &log_stream_name(&reported.executed_version, &reported.request_id, started),
            &invoked.log_events,
        )
        .await;
    }
    Ok(invoked)
}

/// Meter `invocation` as one request the service THROTTLED: the one `Throttles` datapoint a
/// request answered `TooManyRequestsException` is counted by ([`super::metrics`]).
///
/// A throttled request never reached the function, so it is metered here and by none of the
/// series that measure a run.
fn throttled_request(invocation: &Invocation) {
    if let Some(metrics) = invocation.metrics.as_ref() {
        metrics.publish_throttle();
    }
}

/// Marks a run's thread as FINISHED on every exit of that thread, including an unwind out
/// of the interpreter, and gives back the count of a run the caller had already given up
/// on.
///
/// Whichever of this guard and [`RunAccounting`] marks the lifecycle first decides the
/// accounting, so an abandoned run is counted once and released once.
struct ThreadLifecycle {
    /// The run's lifecycle, shared with the task that waits for it.
    lifecycle: Arc<AtomicU8>,
}

impl Drop for ThreadLifecycle {
    fn drop(&mut self) {
        if self.lifecycle.swap(RUN_FINISHED, Ordering::SeqCst) == RUN_ABANDONED {
            release_abandoned_run();
        }
    }
}

/// Counts a run that is still going when the caller stops waiting for it, on EVERY way of
/// stopping: the wait running out, and the invocation's future being dropped where it waits
/// (a cancelled request).
///
/// A run whose thread has already ended is not abandoned, and the thread's own mark is what
/// says so — exactly one of the two marks the lifecycle, so an abandoned run is counted once
/// and released once ([`release_abandoned_run`]).
struct RunAccounting {
    /// The run's lifecycle, shared with its thread.
    lifecycle: Arc<AtomicU8>,
}

impl Drop for RunAccounting {
    fn drop(&mut self) {
        // Counted FIRST and then released if the thread got there first, so the count is
        // never observed low while a claim is in flight: the thread's own release can only
        // run after this claim, whichever of the two marks the lifecycle first.
        claim_abandoned_run();
        if self.lifecycle.swap(RUN_ABANDONED, Ordering::SeqCst) == RUN_FINISHED {
            // The thread ended on its own, so there is nothing outstanding to count. Its
            // mark stands and the claim above is given back.
            self.lifecycle.store(RUN_FINISHED, Ordering::SeqCst);
            release_abandoned_run();
        }
    }
}

/// How a finished run is reported.
fn answered(
    finished: Finished,
    request_id: &str,
    invocation: &Invocation,
    budget: Budget,
    engine: Engine,
    started: SystemTime,
) -> Invoked {
    let (payload, function_error) = match &finished.completion {
        Completion::Returned(payload) => (payload.clone().into_bytes(), None),
        // A response-streaming handler's answer is the bytes it WROTE. The stream IS the
        // answer, and one that has been begun cannot be withdrawn, so those bytes stay the
        // answer whatever the handler went on to do — and a handler that failed ONCE IT HAD
        // WRITTEN fails the invocation beside them, so the stream's trailer names the error
        // that cut the response short and the client can tell a complete response from a
        // truncated one. A handler that failed before its first write delivers no bytes and
        // no error code: nothing was begun for an error to have truncated, and its failure
        // is in the run's LOG alone, as the `Invoke Error` record the runtime writes for it.
        Completion::Streamed { chunks, failure } => (
            chunks.concat().into_bytes(),
            failure
                .as_ref()
                .map(|_| FUNCTION_ERROR_UNHANDLED.to_string()),
        ),
        Completion::Raised {
            error_type,
            error_message,
            stack_trace,
        } => (
            error_document(
                error_message,
                Some(error_type),
                // A failure the runtime raised while it was still RESOLVING the handler
                // belongs to no invocation: the execution environment is initialized
                // before a request is bound to it, so the document names no request at
                // all.
                if raised_during_initialization(error_type) {
                    ""
                } else {
                    request_id
                },
                stack_trace,
                engine,
            ),
            Some(FUNCTION_ERROR_UNHANDLED.to_string()),
        ),
        // A run the sandbox ended at the function's own `Timeout` is reported as that
        // class of failure: the error names the sandbox timeout, and the message states
        // the invocation and the timeout it ran past.
        Completion::Stopped(Stop::Deadline) => (
            error_document(
                &format!(
                    "{request_id} Task timed out after {:.2} seconds",
                    budget.wall_clock.as_secs_f64()
                ),
                Some(SANDBOX_TIMEDOUT),
                request_id,
                &[],
                engine,
            ),
            Some(FUNCTION_ERROR_UNHANDLED.to_string()),
        ),
        // A run the memory ceiling ended is one whose runtime was killed, which is what
        // AWS reports it as: "Runtime exited with error: signal: killed".
        Completion::Stopped(Stop::Memory) => (
            error_document(
                &format!("RequestId: {request_id} Error: {RUNTIME_KILLED}"),
                Some(RUNTIME_EXIT_ERROR),
                request_id,
                &[],
                engine,
            ),
            Some(FUNCTION_ERROR_UNHANDLED.to_string()),
        ),
        // A run the RESPONSE ceiling ended produced an answer Lambda will not carry, and
        // Lambda attributes that failure to the response rather than reporting it as a
        // runtime that exited: the error names the response-size class and states the
        // quota the response passed.
        Completion::Stopped(Stop::Response) => (
            error_document(
                &format!(
                    "Response payload size exceeded maximum allowed payload size ({} bytes).",
                    budget.response_bytes
                ),
                Some(RESPONSE_SIZE_TOO_LARGE),
                request_id,
                &[],
                engine,
            ),
            Some(FUNCTION_ERROR_UNHANDLED.to_string()),
        ),
        Completion::Stopped(stop) => (
            error_document(
                stop.reason(),
                Some(RUNTIME_EXIT_ERROR),
                request_id,
                &[],
                engine,
            ),
            Some(FUNCTION_ERROR_UNHANDLED.to_string()),
        ),
        Completion::Internal(why) => (
            error_document(
                why,
                Some("Runtime.InternalError"),
                request_id,
                &[],
                engine,
            ),
            Some(FUNCTION_ERROR_UNHANDLED.to_string()),
        ),
    };
    let log = run_log(
        request_id,
        &invocation.executed_version,
        invocation.memory_size,
        &finished,
    );
    Invoked {
        payload,
        function_error,
        log_events: log_delivery::events(&log.lines, log.opened, started, finished.elapsed),
        log: log.text(),
        duration: finished.elapsed,
        chunks: match &finished.completion {
            Completion::Streamed { chunks, .. } => {
                chunks.iter().map(|chunk| chunk.clone().into_bytes()).collect()
            }
            _ => Vec::new(),
        },
        // The error document of a failure that came once the stream had already carried
        // bytes: the payload is those bytes, so the document is kept here for the trailer
        // that reports it.
        stream_failure: match &finished.completion {
            Completion::Streamed {
                failure: Some(failure),
                ..
            } => Some(error_document(
                &failure.error_message,
                Some(&failure.error_type),
                request_id,
                &failure.stack_trace,
                engine,
            )),
            _ => None,
        },
    }
}

/// The `errorType` of a run whose runtime ended instead of answering.
const RUNTIME_EXIT_ERROR: &str = "Runtime.ExitError";

/// The `errorType` of a run the SANDBOX ended because it ran past the function's own
/// `Timeout`. The failure is the sandbox's rather than the handler's, so it is the class
/// AWS reports for it rather than one of the runtime's own.
const SANDBOX_TIMEDOUT: &str = "Sandbox.Timedout";

/// The `errorType` of a run whose answer was larger than an invocation may return. The
/// failure is the RESPONSE's, so it is attributed to the function's response rather than
/// reported as a runtime that exited.
const RESPONSE_SIZE_TOO_LARGE: &str = "Function.ResponseSizeTooLarge";

/// What AWS reports as the message of such a run when the sandbox ended it.
const RUNTIME_KILLED: &str = "Runtime exited with error: signal: killed";

/// The `errorType` classes a runtime reports for a failure of the INITIALIZATION phase:
/// the phase in which it reads the configured handler name, imports the module that name
/// resolves to, and finds the entry point on it. Each is raised by the runtime itself
/// before it has an invocation in hand — `MalformedHandlerName` for a name it cannot
/// split, `UserCodeSyntaxError` for a module that does not compile, `ImportModuleError`
/// for one that does not import (including one whose own top-level code raised), and
/// `HandlerNotFound` for a module that does not define the entry point — which is what
/// makes a failure of any of them one no invocation is bound to.
///
/// They are the classes BOTH runtimes report ([`python`], [`node`]); a class a handler's
/// own exception carries can never be one of them, because an exception type name holds no
/// `.`.
const INITIALIZATION_ERROR_TYPES: &[&str] = &[
    "Runtime.MalformedHandlerName",
    "Runtime.UserCodeSyntaxError",
    "Runtime.ImportModuleError",
    "Runtime.HandlerNotFound",
];

/// Whether `error_type` is a failure of the initialization phase
/// ([`INITIALIZATION_ERROR_TYPES`]) rather than of the run.
fn raised_during_initialization(error_type: &str) -> bool {
    INITIALIZATION_ERROR_TYPES.contains(&error_type)
}

/// The member the error document carries the handler's stack under, which is the one the
/// function's own runtime reports it under: `stackTrace` for a Python handler, `trace` for
/// a node one.
fn stack_trace_member(engine: Engine) -> &'static str {
    match engine {
        Engine::Python => "stackTrace",
        Engine::Node => "trace",
    }
}

/// Whether the error payload carries the invocation's own request id.
///
/// A Python runtime's error payload names it (`requestId` beside `stackTrace`); a node
/// runtime's does not — the id is carried in the RESPONSE rather than in the body, so the
/// payload a node handler's failure answers with is exactly the three members
/// `{errorType, errorMessage, trace}`.
fn reports_request_id(engine: Engine) -> bool {
    match engine {
        Engine::Python => true,
        Engine::Node => false,
    }
}

/// The error payload an invocation that failed answers with: the members the function's
/// own runtime reports for a handler that raised.
fn error_document(
    message: &str,
    error_type: Option<&str>,
    request_id: &str,
    stack_trace: &[String],
    engine: Engine,
) -> Vec<u8> {
    let mut document = ::serde_json::Map::new();
    document.insert(
        "errorMessage".to_string(),
        ::serde_json::Value::String(message.to_string()),
    );
    if let Some(error_type) = error_type {
        document.insert(
            "errorType".to_string(),
            ::serde_json::Value::String(error_type.to_string()),
        );
    }
    if reports_request_id(engine) {
        document.insert(
            "requestId".to_string(),
            ::serde_json::Value::String(request_id.to_string()),
        );
    }
    document.insert(
        stack_trace_member(engine).to_string(),
        ::serde_json::Value::Array(
            stack_trace
                .iter()
                .map(|frame| ::serde_json::Value::String(frame.clone()))
                .collect(),
        ),
    );
    ::serde_json::Value::Object(document).to_string().into_bytes()
}

/// The LOG of one run, as Lambda writes it: the invocation's start and end lines around
/// what the handler wrote, and the report line that states what the run spent.
///
/// Every number in it is measured: the duration is the run's own, and the memory is what
/// the run was allowed to hold (see [`python`], [`node`]).
///
/// It is kept as its LINES because that is what CloudWatch Logs holds it as — one event per
/// line ([`log_delivery`]) — and the whole text an invocation answers a log tail from is
/// those same lines.
#[derive(Debug)]
struct RunLog {
    /// Every line of the log, in order and without its line break.
    lines: Vec<String>,
    /// How many of those lines OPEN the log: the framing Lambda writes before the handler's
    /// own output.
    opened: usize,
}

impl RunLog {
    /// The whole log as text, each line closed by its break.
    fn text(&self) -> String {
        let mut text = String::new();
        for line in &self.lines {
            text.push_str(line);
            text.push('\n');
        }
        text
    }
}

fn run_log(request_id: &str, version: &str, memory_size: i32, finished: &Finished) -> RunLog {
    let duration_ms = finished.elapsed.as_secs_f64() * 1_000.0;
    let billed_ms = duration_ms.ceil().max(1.0);
    let mut lines = vec![format!("START RequestId: {request_id} Version: {version}")];
    let opened = lines.len();
    if !finished.log.is_empty() {
        let mut written: Vec<&str> = finished.log.split('\n').collect();
        // A log that ends on a line break closes its last line rather than opening an empty
        // one, so the piece after that break is not a line of the log.
        if written.last().is_some_and(|last| last.is_empty()) {
            written.pop();
        }
        lines.extend(written.into_iter().map(str::to_string));
    }
    lines.push(format!("END RequestId: {request_id}"));
    lines.push(format!(
        "REPORT RequestId: {request_id}\tDuration: {duration_ms:.2} ms\tBilled Duration: \
         {billed_ms:.0} ms\tMemory Size: {memory_size} MB"
    ));
    RunLog { lines, opened }
}

/// The event text the handler is called with, or `None` for an invocation that carried no
/// payload at all.
fn event_text(payload: &[u8]) -> Option<String> {
    if payload.is_empty() {
        return None;
    }
    String::from_utf8(payload.to_vec()).ok()
}

/// `at` as the MILLISECONDS since the epoch, which is how an instant of a run is stated
/// wherever one leaves this module: the deadline a run's own runtime reports to its handler
/// ([`python`], [`node`]) and the timestamp its log events carry ([`log_delivery`]).
pub(crate) fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_millis()).ok())
        .unwrap_or_default()
}

/// What one run may spend: the function's own `Timeout`, clamped to the range the model
/// allows for it and to this module's ceiling, and the memory its own `MemorySize` asks for
/// ([`run_heap_bytes`]), plus the bounds that are this emulator's.
///
/// The memory bound is the SAME quantity the run's share of the process envelope is charged
/// in ([`run_memory_claim_mb`]), so what a run may allocate and what it holds of the
/// envelope cannot diverge: a function configured for the smallest `MemorySize` the model
/// allows is bounded at that, not at the per-run ceiling.
#[must_use]
pub fn run_budget(timeout_seconds: i32, memory_size: i32) -> Budget {
    let clamped = i64::from(timeout_seconds).clamp(MIN_TIMEOUT_SECONDS, MAX_TIMEOUT_SECONDS);
    let wall_clock = Duration::from_secs(u64::try_from(clamped).unwrap_or(1)).min(HOST_WAIT_CEILING);
    Budget {
        wall_clock,
        steps: MAX_RUN_STEPS,
        memory_bytes: u64::try_from(run_heap_bytes(memory_size)).unwrap_or(MAX_RUN_MEMORY_BYTES),
        log_bytes: MAX_RUN_LOG_BYTES,
        response_bytes: MAX_RESPONSE_BYTES,
        recursion_limit: MAX_RUN_RECURSION,
    }
}

/// The memory one run may HOLD: the function's own `MemorySize`, clamped to the range the
/// model allows for it and to this module's own per-run ceiling.
///
/// This is what an engine with an allocation ceiling of its own is given ([`node`]), so
/// that a function's configured memory is the memory its handler really has.
#[must_use]
pub fn run_heap_bytes(memory_size: i32) -> usize {
    let clamped = i64::from(memory_size).clamp(MIN_MEMORY_SIZE_MB, MAX_MEMORY_SIZE_MB);
    let bytes = u64::try_from(clamped)
        .unwrap_or(1)
        .saturating_mul(BYTES_PER_MB)
        .min(MAX_RUN_MEMORY_BYTES);
    usize::try_from(bytes).unwrap_or(usize::MAX)
}

/// The ENVIRONMENT one run sees: the function's own variables, plus the ones the runtime
/// sets for a handler to read its own identity out of.
///
/// This is the whole environment: no host environment variable is in it, and no credential
/// of this emulator is either.
fn runtime_environment(invocation: &Invocation, started: SystemTime) -> BTreeMap<String, String> {
    let mut environment = invocation.variables.clone();
    let reserved: &[(&str, String)] = &[
        ("AWS_LAMBDA_FUNCTION_NAME", invocation.function_name.clone()),
        (
            "AWS_LAMBDA_FUNCTION_VERSION",
            invocation.executed_version.clone(),
        ),
        (
            "AWS_LAMBDA_FUNCTION_MEMORY_SIZE",
            invocation.memory_size.to_string(),
        ),
        (
            "AWS_LAMBDA_LOG_GROUP_NAME",
            invocation.logs.as_ref().map_or_else(
                || super::default_log_group(&invocation.function_name),
                |target| target.log_group.clone(),
            ),
        ),
        (
            "AWS_LAMBDA_LOG_STREAM_NAME",
            log_stream_name(&invocation.executed_version, &invocation.request_id, started),
        ),
        ("AWS_REGION", invocation.region.clone()),
        ("AWS_DEFAULT_REGION", invocation.region.clone()),
        (
            "AWS_LAMBDA_INITIALIZATION_TYPE",
            invocation.initialization_type.to_string(),
        ),
        // "AWS_LAMBDA_RUNTIME_API – … The host and port of the runtime API", which a
        // managed runtime's own environment always names. This emulator serves the
        // invocation to the handler in process rather than over that interface, so the
        // variable names the loopback address the interface is addressed at and nothing
        // reads it.
        ("AWS_LAMBDA_RUNTIME_API", "127.0.0.1:9001".to_string()),
        ("LAMBDA_TASK_ROOT", "/var/task".to_string()),
        ("LAMBDA_RUNTIME_DIR", "/var/runtime".to_string()),
        (
            "_HANDLER",
            invocation.handler.clone().unwrap_or_default(),
        ),
        (
            "AWS_EXECUTION_ENV",
            format!(
                "AWS_Lambda_{}",
                invocation.runtime.clone().unwrap_or_default()
            ),
        ),
    ];
    for (name, value) in reserved {
        environment.insert((*name).to_string(), value.clone());
    }
    // "`_X_AMZN_TRACE_ID` – The X-Ray tracing header", which "changes with each
    // invocation": the trace context of THIS invocation ([`trace_context`]). It is one of
    // the variables the runtime sets and a function may not, so it overrides whatever the
    // function's own environment holds under that name; an invocation with no trace context
    // at all reports none, rather than a chain nobody named.
    match &invocation.trace {
        Some(trace) => {
            environment.insert(
                trace_context::TRACE_ID_VARIABLE.to_string(),
                trace.header.clone(),
            );
        }
        None => {
            environment.remove(trace_context::TRACE_ID_VARIABLE);
        }
    }
    // The variables a managed runtime's own sandbox sets beside those, each at the value
    // AWS documents for it. These are NOT reserved: the function's own variable of the same
    // name is its value, so a function that sets its time zone or its module path runs with
    // the one it set.
    let defaults: &[(&str, String)] = &[
        ("TZ", ":UTC".to_string()),
        ("LANG", "en_US.UTF-8".to_string()),
        (
            "LD_LIBRARY_PATH",
            "/var/lang/lib:/lib64:/usr/lib64:/var/runtime:/var/runtime/lib:/var/task:\
             /var/task/lib:/opt/lib"
                .to_string(),
        ),
        // The two X-Ray variables a managed runtime's environment names: where an
        // instrumented handler sends its segment documents, and what a tracer does when it
        // finds no trace context. The address is one a run's datagrams really reach — the
        // daemon is served in process ([`xray_daemon`]).
        (
            xray_daemon::DAEMON_ADDRESS_VARIABLE,
            xray_daemon::DAEMON_ADDRESS.to_string(),
        ),
        (
            xray_daemon::CONTEXT_MISSING_VARIABLE,
            xray_daemon::CONTEXT_MISSING_LOG_ERROR.to_string(),
        ),
    ];
    for (name, value) in defaults {
        environment
            .entry((*name).to_string())
            .or_insert_with(|| value.clone());
    }
    if engine_for(invocation.runtime.as_deref()) == Some(Engine::Node) {
        // "NODE_PATH – The Node.js library path": the layer directory for this runtime's
        // own major version, the layer's version-independent directory, and the runtime's
        // own modules, which is the search path a `require` of a bare name walks.
        let major = node::node_major(invocation.runtime.as_deref().unwrap_or_default());
        environment.entry("NODE_PATH".to_string()).or_insert_with(|| {
            format!(
                "/opt/nodejs/node{major}/node_modules:/opt/nodejs/node_modules:\
                 /var/runtime/node_modules:/var/runtime:/var/task"
            )
        });
    }
    environment
}

/// The log stream one run writes to: the date the run started, the version it ran, and the
/// invocation's own id — the form a CloudWatch Logs stream of a Lambda function has.
///
/// The date is the `YYYY/MM/DD` of the instant the run started, taken from the one
/// timestamp rendering this crate already has ([`super::iso8601`], whose calendar
/// arithmetic is `aws_smithy_types`'s) rather than computed here.
fn log_stream_name(version: &str, request_id: &str, started: SystemTime) -> String {
    let rendered = super::iso8601(started);
    let date = rendered.split('T').next().unwrap_or_default().replace('-', "/");
    format!(
        "{date}/[{version}]{}",
        request_id.replace('-', "")
    )
}

/// A run that is still going.
const RUN_RUNNING: u8 = 0;
/// A run whose thread has ended.
const RUN_FINISHED: u8 = 1;
/// A run the caller gave up on.
const RUN_ABANDONED: u8 = 2;

/// The engine memory the runs of this process share, in the MB of
/// [`MAX_CONCURRENT_RUN_MEMORY_MB`]: one permit is one MB, so an invocation holds exactly the
/// memory its own configuration asks for ([`run_memory_claim_mb`]) and the runs in flight
/// hold at most the envelope between them.
///
/// The shares are handed out in the order they were asked for, because the semaphore is fair
/// — so a waiting caller is served rather than passed over by the callers arriving behind it,
/// including one waiting for a larger share than they are.
fn run_memory() -> &'static Arc<::tokio::sync::Semaphore> {
    static MEMORY: OnceLock<Arc<::tokio::sync::Semaphore>> = OnceLock::new();
    MEMORY.get_or_init(|| {
        Arc::new(::tokio::sync::Semaphore::new(
            MAX_CONCURRENT_RUN_MEMORY_MB as usize,
        ))
    })
}

/// The mount-reading permits, shared by every invocation this process serves: how many of
/// them may hold their archives INFLATED at one instant ([`max_concurrent_mount_reads`]).
fn mount_readers() -> &'static Arc<::tokio::sync::Semaphore> {
    static READERS: OnceLock<Arc<::tokio::sync::Semaphore>> = OnceLock::new();
    READERS.get_or_init(|| {
        Arc::new(::tokio::sync::Semaphore::new(max_concurrent_mount_reads()))
    })
}

/// How many abandoned runs are outstanding.
fn abandoned() -> &'static AtomicUsize {
    static ABANDONED: AtomicUsize = AtomicUsize::new(0);
    &ABANDONED
}

/// How many runs are outstanding past their budget.
#[must_use]
pub fn abandoned_runs() -> usize {
    abandoned().load(Ordering::SeqCst)
}

/// Count one run as abandoned, for as long as its thread lasts.
fn claim_abandoned_run() {
    abandoned().fetch_add(1, Ordering::SeqCst);
}

/// Release the count of one abandoned run, when its thread finally ends.
fn release_abandoned_run() {
    let mut held = abandoned_runs();
    while held > 0 {
        match abandoned().compare_exchange(held, held - 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return,
            Err(seen) => held = seen,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One invocation of a Python function, as [`run_invocation`] builds one, for the tests
    /// that report on an invocation rather than run one.
    fn probe_invocation() -> Invocation {
        Invocation {
            function_name: "probe".to_string(),
            executed_version: "7".to_string(),
            invoked_function_arn: "arn:aws:lambda:us-west-2:000000000000:function:probe:7"
                .to_string(),
            region: "us-west-2".to_string(),
            runtime: Some("python3.12".to_string()),
            handler: Some("lambda_function.lambda_handler".to_string()),
            code_revision: "11111111-1111-1111-1111-111111111111".to_string(),
            variables: BTreeMap::new(),
            memory_size: 256,
            timeout: 3,
            architecture: "x86_64".to_string(),
            zip_bytes: None,
            layer_zips: Vec::new(),
            payload: Vec::new(),
            client_context: None,
            tenant_id: None,
            request_id: "11111111-2222-3333-4444-555555555555".to_string(),
            trace: None,
            initialization_type: crate::state::execution_slot::INITIALIZATION_TYPE_ON_DEMAND,
            aws: None,
            logs: None,
            metrics: None,
        }
    }

    /// The runtimes this emulator can run a package for, and which engine runs each — one
    /// row per class of `Runtime` value the model declares (source:
    /// `smithy_query(query="shape", shape="Runtime")`).
    #[test]
    fn a_runtime_resolves_to_the_engine_that_runs_it() {
        let cases: &[(Option<&str>, Option<Engine>)] = &[
            (Some("python3.12"), Some(Engine::Python)),
            (Some("python3.13"), Some(Engine::Python)),
            (Some("python3.9"), Some(Engine::Python)),
            (Some("nodejs22.x"), Some(Engine::Node)),
            (Some("nodejs20.x"), Some(Engine::Node)),
            (Some("nodejs18.x"), Some(Engine::Node)),
            (Some("nodejs"), Some(Engine::Node)),
            (Some("java21"), None),
            (Some("dotnet8"), None),
            (Some("ruby3.3"), None),
            (Some("provided.al2023"), None),
            (Some("Image"), None),
            (None, None),
        ];
        for (runtime, expected) in cases {
            let actual = engine_for(*runtime);
            assert_eq!(
                actual, *expected,
                "case {runtime:?}: expected the engine {expected:?}, got {actual:?}"
            );
            assert_eq!(
                runs_handler(*runtime),
                expected.is_some(),
                "case {runtime:?}: a runtime runs exactly when an engine here runs it"
            );
        }
    }

    /// A stored `MemorySize` is clamped into the range the model allows before it bounds a
    /// run, and never past this module's own per-run ceiling — one row per bound and one
    /// past each.
    #[test]
    fn a_stored_memory_size_is_clamped_before_it_bounds_a_run() {
        let cases: &[(i32, u64)] = &[
            (0, 128),
            (-7, 128),
            (128, 128),
            (256, 256),
            (512, 512),
            (10_240, MAX_RUN_MEMORY_BYTES / BYTES_PER_MB),
            (32_768, MAX_RUN_MEMORY_BYTES / BYTES_PER_MB),
            (100_000, MAX_RUN_MEMORY_BYTES / BYTES_PER_MB),
        ];
        for (configured, expected_mb) in cases {
            let actual = run_heap_bytes(*configured) as u64 / BYTES_PER_MB;
            assert_eq!(
                actual, *expected_mb,
                "case {configured} MB configured: expected a {expected_mb} MB ceiling, got \
                 {actual}"
            );
        }
    }

    /// The error document carries the handler's stack under the member the function's own
    /// runtime reports it under — one row per engine.
    #[test]
    fn the_error_document_names_the_stack_the_way_the_runtime_does() {
        let cases: &[(Engine, &str, &str)] = &[
            (Engine::Python, "stackTrace", "trace"),
            (Engine::Node, "trace", "stackTrace"),
        ];
        for (engine, expected, absent) in cases {
            let document = error_document(
                "boom",
                Some("Error"),
                "11111111-2222-3333-4444-555555555555",
                &["  at handler".to_string()],
                *engine,
            );
            let parsed: ::serde_json::Value =
                ::serde_json::from_slice(&document).expect("the error document is JSON");
            assert!(
                parsed.get(*expected).is_some_and(::serde_json::Value::is_array),
                "case {engine:?}: expected the stack under {expected}, got {parsed}"
            );
            assert!(
                parsed.get(*absent).is_none(),
                "case {engine:?}: {absent} is another runtime's member, got {parsed}"
            );
        }
    }

    /// A run the sandbox ended at the function's own `Timeout` is reported as that class of
    /// failure, with a message stating the timeout the run was given — one row per
    /// `Timeout` and per engine, so neither the class nor the message is one run's.
    #[test]
    fn a_timed_out_runs_document_names_the_sandbox_timeout_class() {
        let request_id = "11111111-2222-3333-4444-555555555555";
        let cases: &[(Engine, i32, &str)] = &[
            (Engine::Python, 1, "1.00"),
            (Engine::Python, 3, "3.00"),
            (Engine::Python, 29, "29.00"),
            (Engine::Node, 1, "1.00"),
            (Engine::Node, 3, "3.00"),
        ];
        for (engine, timeout, rendered) in cases {
            let budget = run_budget(*timeout, MIN_MEMORY_SIZE_MB as i32);
            let finished = Finished {
                completion: Completion::Stopped(Stop::Deadline),
                log: String::new(),
                elapsed: budget.wall_clock,
            };
            let invoked = answered(
                finished,
                request_id,
                &probe_invocation(),
                budget,
                *engine,
                SystemTime::UNIX_EPOCH,
            );
            let parsed: ::serde_json::Value =
                ::serde_json::from_slice(&invoked.payload).expect("the error document is JSON");
            let case = format!("{engine:?} with a {timeout} s timeout");
            assert_eq!(
                invoked.function_error.as_deref(),
                Some(FUNCTION_ERROR_UNHANDLED),
                "case {case}: a timed-out run is reported as a function error, got {:?}",
                invoked.function_error
            );
            assert_eq!(
                parsed.get("errorType").and_then(::serde_json::Value::as_str),
                Some(SANDBOX_TIMEDOUT),
                "case {case}: expected the sandbox timeout class, got {parsed}"
            );
            let expected = format!("{request_id} Task timed out after {rendered} seconds");
            assert_eq!(
                parsed
                    .get("errorMessage")
                    .and_then(::serde_json::Value::as_str),
                Some(expected.as_str()),
                "case {case}: expected the message {expected:?}, got {parsed}"
            );
        }
    }

    /// The request id the error document names is the INVOCATION's only for a failure the
    /// invocation was bound to: one the runtime raised while it was still RESOLVING the
    /// handler names none — one row per error class a runtime reports for each phase, per
    /// engine (the node document names no request at all, which is its own runtime's rule).
    #[test]
    fn an_initialization_failures_document_names_no_invocation() {
        let request_id = "11111111-2222-3333-4444-555555555555";
        // The class the runtime reported, and the request id the document must name for a
        // Python function — `None` where the member is absent, as it is for a node one.
        let cases: &[(&str, Option<&str>, Option<&str>)] = &[
            ("ValueError", Some(request_id), None),
            ("KeyError", Some(request_id), None),
            ("Runtime.MarshalError", Some(request_id), None),
            ("Runtime.InvalidEvent", Some(request_id), None),
            ("Runtime.MalformedHandlerName", Some(""), None),
            ("Runtime.ImportModuleError", Some(""), None),
            ("Runtime.UserCodeSyntaxError", Some(""), None),
            ("Runtime.HandlerNotFound", Some(""), None),
        ];
        for (error_type, python, node) in cases {
            for (engine, expected) in [(Engine::Python, python), (Engine::Node, node)] {
                let finished = Finished {
                    completion: Completion::Raised {
                        error_type: (*error_type).to_string(),
                        error_message: "the run did not answer".to_string(),
                        stack_trace: Vec::new(),
                    },
                    log: String::new(),
                    elapsed: Duration::ZERO,
                };
                let invoked = answered(
                    finished,
                    request_id,
                    &probe_invocation(),
                    run_budget(3, MIN_MEMORY_SIZE_MB as i32),
                    engine,
                    SystemTime::UNIX_EPOCH,
                );
                let parsed: ::serde_json::Value = ::serde_json::from_slice(&invoked.payload)
                    .expect("the error document is JSON");
                let actual = parsed
                    .get("requestId")
                    .and_then(::serde_json::Value::as_str);
                assert_eq!(
                    actual, *expected,
                    "case {engine:?} reporting {error_type}: expected the request id \
                     {expected:?}, got {parsed}"
                );
            }
        }
    }

    /// Every frame of the stack a Python run's error document carries is ONE frame as the
    /// runtime renders it: its location line, which names the source FILE by path, and the
    /// source line at that location — with no fine-grained error-location anchors under it.
    /// One row per shape of stack, including one that reaches a module the RUNTIME itself
    /// supplies, whose frame names that module's file under the runtime's own library
    /// directory rather than the bare module name it was compiled under.
    #[test]
    fn a_reported_frame_is_a_location_and_its_source_line() {
        // The library directory of the runtime the probe function is configured with, where
        // a managed runtime keeps the standard library it ships.
        let library = "/var/lang/lib/python3.12/";
        let cases: &[(&str, &str, usize, Option<&str>)] = &[
            (
                "a subscript of a key the event does not hold",
                "def handler(event, context):\n    return event[\"absent\"]\n",
                1,
                None,
            ),
            (
                "a raise two calls deep in the package's own module",
                "def inner(value):\n    raise ValueError(\"no good: %s\" % value)\n\n\
                 def handler(event, context):\n    return inner(7)\n",
                2,
                None,
            ),
            (
                "a read of an environment variable the function does not set",
                "import os\n\n\
                 def handler(event, context):\n    return os.environ[\"ABSENT\"]\n",
                2,
                Some(library),
            ),
        ];
        for (case, source, least, within) in cases {
            let frames = python_run_frames(source);
            assert!(
                frames.len() >= *least,
                "case {case}: expected at least {least} frames, got {frames:?}"
            );
            for (index, frame) in frames.iter().enumerate() {
                let lines: Vec<&str> =
                    frame.strip_suffix('\n').unwrap_or(frame).split('\n').collect();
                assert!(
                    lines.len() <= 2 && frame.ends_with('\n'),
                    "case {case}: frame {index} is a location and at most its source line, \
                     each ended, got {frame:?}"
                );
                let file = lines[0]
                    .strip_prefix("  File \"")
                    .and_then(|rest| rest.split_once("\", line "))
                    .map(|(file, _)| file)
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case}: frame {index} must open with its location, got \
                             {frame:?}"
                        )
                    });
                assert!(
                    file.starts_with('/') && file.ends_with(".py"),
                    "case {case}: frame {index} must name a source file by path, got {file:?}"
                );
                for line in lines.iter().skip(1) {
                    let marks = line.trim();
                    assert!(
                        marks.is_empty()
                            || !marks.chars().all(|mark| matches!(mark, '~' | '^')),
                        "case {case}: frame {index} carries an error-location anchor, which \
                         no frame of a reported stack does, got {line:?}"
                    );
                }
            }
            if let Some(within) = within {
                let expected = format!("  File \"{within}");
                assert!(
                    frames.iter().any(|frame| frame.starts_with(&expected)),
                    "case {case}: expected a frame naming a source file in {within:?}, got \
                     {frames:?}"
                );
            }
        }
    }

    /// The frames of the stack a Python handler's own failure reports, out of a run of
    /// `source` as the one module of a package — driven through the interpreter on a thread
    /// with the stack a run is given, the way [`run_invocation`] drives one.
    fn python_run_frames(source: &str) -> Vec<String> {
        let modules = vec![package::PackageModule {
            path: "lambda_function.py".to_string(),
            source: source.to_string(),
        }];
        // The environment a run of the probe function really sees, which is where the
        // runtime reads the library directory of its own modules from.
        let environment = runtime_environment(&probe_invocation(), SystemTime::UNIX_EPOCH);
        let finished = ::std::thread::Builder::new()
            .stack_size(RUN_STACK_BYTES)
            .spawn(move || {
                python::run(&python::Run {
                    modules: &modules,
                    handler: "lambda_function.handler",
                    environment: &environment,
                    event: None,
                    request_id: "11111111-2222-3333-4444-555555555555",
                    invoked_function_arn: "arn:aws:lambda:us-west-2:000000000000:function:probe",
                    client_context: None,
                    tenant_id: None,
                    budget: run_budget(5, MIN_MEMORY_SIZE_MB as i32),
                    aws: None,
                })
            })
            .expect("a run thread")
            .join()
            .expect("the run thread ended");
        match finished.completion {
            Completion::Raised { stack_trace, .. } => stack_trace,
            other => panic!("expected the handler's own failure, got {other:?}"),
        }
    }

    /// A stored `Timeout` is clamped into the range the model allows before it bounds a
    /// run, and never past this module's own ceiling — one row per bound and one past each.
    #[test]
    fn a_stored_timeout_is_clamped_before_it_bounds_a_run() {
        let cases: &[(i32, u64)] = &[
            (0, 1),
            (-7, 1),
            (1, 1),
            (3, 3),
            (29, 29),
            (900, HOST_WAIT_CEILING.as_secs()),
            (100_000, HOST_WAIT_CEILING.as_secs()),
        ];
        for (configured, expected) in cases {
            let actual = run_budget(*configured, MIN_MEMORY_SIZE_MB as i32)
                .wall_clock
                .as_secs();
            assert_eq!(
                actual, *expected,
                "case {configured} s configured: expected a {expected} s run budget, got {actual}"
            );
        }
    }

    /// The log tail is the LAST 4 KB of the log, base64-encoded — one row per size class
    /// around that bound.
    #[test]
    fn the_log_tail_is_the_last_four_kilobytes() {
        for size in [0_usize, 1, LOG_TAIL_BYTES - 1, LOG_TAIL_BYTES, LOG_TAIL_BYTES + 1, 64 * 1024]
        {
            let invoked = Invoked {
                payload: Vec::new(),
                function_error: None,
                log: "L".repeat(size),
                log_events: Vec::new(),
                duration: Duration::ZERO,
                chunks: Vec::new(),
                stream_failure: None,
            };
            let decoded = ::base64::Engine::decode(
                &::base64::engine::general_purpose::STANDARD,
                invoked.log_tail(),
            )
            .expect("the tail is base64");
            let expected = size.min(LOG_TAIL_BYTES);
            assert_eq!(
                decoded.len(),
                expected,
                "case a {size}-byte log: expected a {expected}-byte tail, got {}",
                decoded.len()
            );
        }
    }

    /// The environment a run sees carries the function's own variables and the ones the
    /// runtime sets, and nothing of the host.
    #[test]
    fn the_run_environment_is_the_functions_own_plus_the_runtimes() {
        let mut invocation = probe_invocation();
        invocation
            .variables
            .insert("SECRET".to_string(), "value".to_string());
        let environment = runtime_environment(&invocation, SystemTime::UNIX_EPOCH);
        let cases: &[(&str, &str)] = &[
            ("SECRET", "value"),
            ("AWS_LAMBDA_FUNCTION_NAME", "probe"),
            ("AWS_LAMBDA_FUNCTION_VERSION", "7"),
            ("AWS_LAMBDA_FUNCTION_MEMORY_SIZE", "256"),
            ("AWS_LAMBDA_LOG_GROUP_NAME", "/aws/lambda/probe"),
            ("AWS_REGION", "us-west-2"),
            ("LAMBDA_TASK_ROOT", "/var/task"),
            ("_HANDLER", "lambda_function.lambda_handler"),
            ("AWS_EXECUTION_ENV", "AWS_Lambda_python3.12"),
            ("AWS_LAMBDA_INITIALIZATION_TYPE", "on-demand"),
        ];
        for (name, expected) in cases {
            let actual = environment.get(*name).map(String::as_str);
            assert_eq!(
                actual,
                Some(*expected),
                "case {name}: expected {expected:?}, got {actual:?}"
            );
        }
        for absent in ["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "PATH", "HOME"] {
            assert!(
                !environment.contains_key(absent),
                "case {absent}: no host or credential variable reaches a run, got {:?}",
                environment.get(absent)
            );
        }
        let stream = environment
            .get("AWS_LAMBDA_LOG_STREAM_NAME")
            .map(String::as_str)
            .unwrap_or_default();
        assert_eq!(
            stream, "1970/01/01/[7]11111111222233334444555555555555",
            "expected the stream to name the date, the version and the invocation, got {stream:?}"
        );
    }
}
