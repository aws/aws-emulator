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
//! number of those threads is bounded by the pool that admits a run at all
//! ([`MAX_CONCURRENT_MOUNT_READS`], held for the whole run).
//!
//! An execution environment that is STANDING holds its engine's memory outside that envelope,
//! because the share a run claimed goes back when the invocation is answered: what the
//! environments kept for the next invocation hold is bounded on its own, by their number and
//! by the memory they have grown to between them ([`warm_pool`]).

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

/// How much memory ONE run holds, as the host measures it — the reading both engines report.
mod run_memory;

/// The Python interpreter one invocation runs in, and everything that bounds and contains
/// it.
pub mod python;

/// The RELEASE of that interpreter a run reports, which is the one the function's `Runtime`
/// names.
mod python_release;

/// The SOURCE of the standard library that interpreter ships, which a traceback frame inside
/// one of its modules carries the line of.
mod python_library;

/// The execution ENVIRONMENTS this process keeps standing, which an invocation of an
/// already-initialized one is served by.
pub mod warm_pool;

/// The JavaScript engine one invocation runs in, and everything that bounds and contains
/// it.
pub mod node;

/// The FILESYSTEM that runtime serves the handler: the mounts of the execution environment,
/// read from this process's own memory. Declared before the runtime it opens, because that
/// source is built with this one's macro.
mod guest_mount;

/// The EPHEMERAL STORAGE that runtime serves beside those mounts: the writable `/tmp` the
/// function's `EphemeralStorage.Size` sizes. Declared with the mounts and before the runtime,
/// for the same reason: that source is built with this one's macro too.
mod guest_ephemeral;

/// The NETWORK FILE SYSTEMS that runtime serves beside both: the Amazon EFS access points the
/// function's `FileSystemConfigs` mount, whose files are held by the service that owns them.
/// Declared with the two above and before the runtime, for the same reason: that source is
/// built with this one's macro too.
mod guest_file_system;

/// The runtime the handler runs under, in the language the handler is written in.
mod bootstrap;

/// The AWS SDK call seam a running handler reaches the rest of this emulator through.
pub mod aws_api;
pub use aws_api::AwsAccess;

/// What an SDK client fills into a call's input itself: the members the called service's model
/// marks as an idempotency token.
mod idempotency_token;

/// What an SDK client CONVERTS in a call's input itself: a timestamp member, into the form the
/// called service's protocol carries one in.
mod sdk_timestamps;

/// The HTTP seam a running handler reaches it through when it signs and sends a request of
/// its own.
pub mod guest_http;

/// The RUNTIME API that seam answers at the address `AWS_LAMBDA_RUNTIME_API` names.
pub mod runtime_api;

/// The INSTANCE METADATA SERVICE it answers at the link-local address a node's own facts are
/// published at: the token a read presents, and the environment's own placement.
pub mod instance_metadata;

/// The TLS layer of the standard library a running Python handler imports, supplied beside
/// the socket interface it is built on.
pub mod guest_tls;

/// The STREAM sockets of that same socket interface, and the connections the host holds for
/// them: one TCP connection per socket, to a data-plane listener this emulator opened.
pub mod guest_socket;

/// The waiting interface of that same standard library — the `select` module a handler, or a
/// client library it vendors, imports beside the socket interface.
pub mod guest_select;

/// The signal interface of that same standard library — the `signal` module a handler, or a
/// client library it vendors, imports beside the waiting interface.
pub mod guest_signal;

/// The child-process interface of that same standard library — the `subprocess` module a
/// handler, a client library it vendors, or the frozen `asyncio` imports.
pub mod guest_process;

/// The DATABASE interface of that same standard library — the `sqlite3` module a handler
/// imports — and the in-process SQLite engine its statements run on.
pub mod guest_sqlite;

/// The TIME ZONE interface of that same standard library — the `zoneinfo` package a handler
/// imports — and the IANA database its zones are read from.
pub mod guest_zoneinfo;

/// The CONNECTED PAIRS a run holds inside this process, which the socket interface answers for
/// `socketpair` and the waiting interface reports readiness on.
pub mod guest_pipes;

/// The CONFIGURATION interface of that same standard library — the `sysconfig` module a
/// handler, or a module of the library itself, imports to ask about the installation.
pub mod guest_sysconfig;

/// What a call of that interface PERFORMS: one program of the execution environment, resolved
/// against its own `PATH` and mounts and run over its filesystem in this process.
pub mod guest_child_process;

/// The THREADS an invocation's own code starts, run on the invocation's own thread inside a
/// slice of its own budget.
mod guest_thread;

/// The Amazon S3 half of that seam, whose own protocol binds a request per operation.
mod aws_s3;

/// The URL an SDK in a running handler PRESIGNS for somebody else to fetch, which is signed
/// for this emulator's own endpoint rather than sent anywhere.
mod presigned_url;

/// The CloudFront half of it, for the same reason: that service's own `restXml` protocol
/// binds a request per operation rather than carrying an input document in a JSON body.
mod aws_cloudfront;

/// The Amazon SNS half of it, for the same reason: that service's model declares `awsQuery`,
/// so a request to it is a form-encoded action and its answer is an XML document rather than
/// the input and output documents a JSON protocol carries.
mod aws_sns;

/// The Amazon SES half of it, for the same reason: the email service's own model declares
/// `awsQuery` too, and it is what a handler that exists to mail — a scheduled report, a
/// confirmation, an alert — hands its message to.
mod aws_ses;

/// The SECURITY TOKEN half of it, for the same reason: that service's model declares
/// `awsQuery` too, and it is what a handler that chains roles assumes one through.
mod aws_sts;

/// The IDENTITY half of it, for the same reason: that service's model declares `awsQuery`
/// too, and it is what a handler reads the role it runs as and the documents in force on that
/// role through, and what one administering an identity writes them through.
mod aws_iam;

/// The Amazon RDS half of it, for the same reason: that service's model declares `awsQuery`
/// too, and it is what a handler holding a database credential puts a new one in force
/// through, and what one driving a database's Amazon S3 export and its role associations
/// calls.
mod aws_rds;

/// The AWS CLOUDFORMATION half of it, for the same reason: that service's model declares
/// `awsQuery` too, and it is what a handler reads the stack it was deployed by — its template,
/// its resources, its outputs and its events — and drives a stack of its own through.
mod aws_cloudformation;

/// The Amazon EC2 half of it, for the same reason: that service's model declares `ec2Query`,
/// so a request to it is a form-encoded action answered with an XML document — and it is what
/// a handler that tightens or restores a security group's own rules calls, which is the whole
/// of what a custom resource over a VPC's default group does.
mod aws_ec2;

/// What every half of that seam bound by an AWS SDK CLIENT shares: how a call's input document
/// is read, what the answer carried, and how a refusal is handed back.
mod sdk_call;

/// Binding one of those calls to HTTP for a service whose protocol is `restJson1`.
pub mod rest_json;

/// WHICH values of an answered call an SDK MATERIALISES rather than hand over as the text or
/// the number the wire carried — its bytes and its instants — which the called service's own
/// model says.
pub mod blob_members;

/// The AUTHENTICATION TOKEN the SDK mints in the CLIENT for a handler connecting to a
/// database with IAM database authentication, which is not an API call at all.
pub mod db_auth_token;

/// The credential a running handler's own calls are made with: a session of the function's
/// execution role.
pub mod execution_role_session;
pub use execution_role_session::{
    function_sdk_access, function_sdk_access_in_vpc, ExecutionRoleSessions,
};

/// The AWS SDK modules a Python handler imports, which that seam is reached through.
mod aws_sdk_guest;

/// The DURABLE-EXECUTION programming model a Python handler is written against, which this
/// runtime supplies the way it supplies the AWS SDK.
mod durable_sdk_guest;

/// Delivering one invocation's log to CloudWatch Logs, which is where a client reads it.
pub mod log_delivery;
pub use log_delivery::{log_target, LogEvent, LogGrants, LogTarget};

/// The FORM those records take: the platform frames of one run, rendered in the function's
/// own `LoggingConfig$LogFormat`.
mod log_records;

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
pub use environment_key::{
    absent_environment_key_refusal, absent_source_key_refusal, durable_data_key_fault,
    environment_key_fault, report_environment_key_fault, unencryptable_environment_refusal,
    EnvironmentKeyFault,
};

/// The X-Ray trace context one invocation runs with, which the execution environment
/// reports in `_X_AMZN_TRACE_ID`.
pub mod trace_context;
pub use trace_context::{
    invocation_trace_context, supplied_event_source_links, supplied_trace_header,
    EventSourceTraceLinks,
};
use trace_context::report_segments;

/// The X-Ray daemon a traced invocation reports its own work to, and the datagram surface a
/// handler reaches it through.
pub mod xray_daemon;

/// The VPC a function with a `VpcConfig` is ATTACHED to, as the run inside it addresses that
/// VPC: the private address its traffic leaves from, and the VPC's own DNS resolver.
pub mod vpc_network;
pub use vpc_network::VpcAttachment;

/// The NAME SERVICE of a running handler: which addresses a host name resolves to inside the
/// execution environment.
pub mod guest_resolver;

/// The engine memory the runs of this process may hold TOGETHER, stated in the MB a
/// function's `MemorySize` is stated in.
///
/// This is the AGGREGATE bound on what the handlers of this process can take of the host: one
/// run holds at most the memory its own configuration asks for, clamped to this module's
/// per-run ceiling ([`run_heap_bytes`], at most [`MAX_RUN_MEMORY_BYTES`]), and the runs in
/// flight hold at most this between them — so what the engines hold does not grow with the
/// number of callers, however many invocations arrive.
///
/// It is stated as a COUNT OF THE LARGEST RUNS ([`CONCURRENT_RUNS_OF_THE_LARGEST_HEAP`])
/// because the concurrency a client asks for is a count: "concurrency is the number of
/// in-flight requests your function is handling at the same time", and
/// `PutFunctionConcurrency` reserves "the number of simultaneous executions" for a function,
/// whatever each of those executions is configured for. A function's `MemorySize` is "the
/// amount of memory available to the function at runtime" — the ceiling ONE run may grow to,
/// which this module enforces per run — and not capacity taken from the invocations beside
/// it, so a fan-out of a function configured for the most memory a run is given must still
/// have every one of a handful of simultaneous invocations really running at once. An
/// envelope of a few of those runs would serialise them instead, and a caller that needs its
/// invocations in flight together — a barrier, a latch, a fan-out that samples its own
/// concurrency — would never see more than that few.
pub const MAX_CONCURRENT_RUN_MEMORY_MB: u32 =
    CONCURRENT_RUNS_OF_THE_LARGEST_HEAP * PER_RUN_MEMORY_CEILING_MB;

/// How many runs of the LARGEST per-run heap the envelope above holds at once, which is the
/// concurrency this process serves whatever the functions are configured for — and four times
/// as many runs of the smallest `MemorySize` the model allows ([`MAX_CONCURRENT_RUNS`]).
///
/// A HOST bound, not an API one: no emulator runs the thousand simultaneous executions an
/// account's limit allows, and an invocation that finds the envelope full WAITS for a share
/// rather than being refused ([`PERMIT_WAIT`]). What it has to be is the fan-out a client
/// RESERVES for one function when it needs those invocations in flight together — a
/// reservation made to bound a downstream, or to drive a barrier, is tens of simultaneous
/// executions and not thousands — so that the memory a function is configured for never
/// decides that number. It is stated at twenty of them and no higher: this is the count of
/// handlers this process may run at once, each still stopped at its own configured memory
/// ([`run_budget`]), so what the figure decides is how many engines are live and not how much
/// any one of them may take.
///
/// What this process may therefore hold of the host, as the figure leaves it, each product
/// stated where it is bounded: twenty run threads and their stacks ([`RUN_STACK_BYTES`], so
/// 320 MB), twenty callers' archives inflated at once ([`MAX_CONCURRENT_MOUNT_READS`] ×
/// [`package::ReadBudget`]), the idle environments the pool keeps ([`warm_pool`], stated on
/// their own and unchanged by this figure), and the engine memory of twenty runs, each of
/// which is STOPPED at the memory its own function asked for. Only the last of those grows
/// with the figure, and it grows as the same arithmetic AWS applies to the same reservation:
/// twenty simultaneous executions of a function configured for 512 MB are twenty environments
/// of 512 MB there too. A run that really allocates that much is a handler the developer
/// wrote and configured for it; a caller that cannot make the emulator run more than twenty
/// handlers cannot reach past it however many invocations it sends.
const CONCURRENT_RUNS_OF_THE_LARGEST_HEAP: u32 = 20;

/// The per-run ceiling [`MAX_RUN_MEMORY_BYTES`] states, in those same MB.
const PER_RUN_MEMORY_CEILING_MB: u32 = (MAX_RUN_MEMORY_BYTES / BYTES_PER_MB) as u32;

/// How long an invocation waits for its share of that envelope before it is throttled.
///
/// As long as a run AHEAD of it is being answered: such a run holds its share while the
/// execution environment it happens in is stood up ([`ENVIRONMENT_BUILD_SPAN`]), for the
/// initialization that then runs in it ([`INIT_PHASE_LIMIT`]) and for
/// the handler's own run, whose wait is bounded by [`HOST_WAIT_CEILING`], plus the
/// [`ABANDON_GRACE`] its caller then gives it. So a caller that arrives while the memory is
/// held is served as soon as a handler ahead of it ends, and only an envelope still full
/// after that span is answered the modelled throttle. A run that outlived its own caller
/// keeps its share until its thread ends, because the engine still holding that memory is
/// that thread's, and it counts against [`MAX_ABANDONED_RUNS`] for just as long.
pub const PERMIT_WAIT: Duration = ENVIRONMENT_BUILD_SPAN
    .saturating_add(INIT_PHASE_LIMIT)
    .saturating_add(HOST_WAIT_CEILING)
    .saturating_add(ABANDON_GRACE);

/// How many handlers run at once, at most, as the memory envelope ALONE bounds them: every
/// function has at least the smallest `MemorySize` the model allows ([`MIN_MEMORY_SIZE_MB`]),
/// so no more runs than this fit in the envelope however small the functions are.
///
/// It is not the number of runs this process really holds at once, which is the SMALLER of
/// this and the mount pool ([`MAX_CONCURRENT_MOUNT_READS`]): a run takes a mount permit before
/// it takes memory and holds it for its whole life ([`run_invocation`]), so that pool is what
/// bounds the run THREADS and the stacks they hold ([`RUN_STACK_BYTES`]) — twenty of them,
/// whatever the functions are configured for. What this figure bounds is the queue in front of
/// the envelope: a burst of invocations of the smallest functions waits on memory rather than
/// being refused for it.
pub const MAX_CONCURRENT_RUNS: u32 = MAX_CONCURRENT_RUN_MEMORY_MB / (MIN_MEMORY_SIZE_MB as u32);

/// How many invocations may hold their mounts INFLATED at once.
///
/// A deployment package and the layers beside it are the caller's own bytes, expanded in
/// memory before the handler can be loaded ([`package`]), so the number of invocations doing
/// that at one instant is the host's to decide and not the caller's. Each read is itself
/// bounded by [`package::ReadBudget`], so this bounds the product: at most this many
/// invocations × that budget is inflated at once. The permit is held for as long as those
/// EXPANDED bytes are live, which is the whole run, so a queue of invocations waiting for
/// memory cannot hold expanded archives nothing accounts for.
///
/// It is a pool of its own, with a figure stated here rather than taken from the runs the
/// envelope admits of the SMALLEST function ([`MAX_CONCURRENT_RUNS`]), because it bounds
/// DIFFERENT bytes: a caller's archive expanded in memory is not the engine memory a handler
/// runs in. What it may not be is SMALLER than the runs this process serves at once, since the
/// permit is held for the whole run — a smaller pool here would decide the concurrency instead
/// of the envelope — so it is the same count as the runs of the largest heap
/// ([`CONCURRENT_RUNS_OF_THE_LARGEST_HEAP`]), and the product it bounds is that many
/// invocations × [`package::ReadBudget`]'s own byte ceiling. An invocation past it waits for a
/// permit exactly as one waiting for run memory does, and an invocation whose package this
/// emulator cannot read is answered as that without ever waiting for memory it would never
/// use.
const MAX_CONCURRENT_MOUNT_READS: usize = CONCURRENT_RUNS_OF_THE_LARGEST_HEAP as usize;

/// See [`MAX_CONCURRENT_MOUNT_READS`].
#[must_use]
pub fn max_concurrent_mount_reads() -> usize {
    MAX_CONCURRENT_MOUNT_READS
}

/// How much of that envelope one invocation of a function configured for `memory_size` MB
/// holds while it runs: the memory its engine is really given ([`run_heap_bytes`]), in MB, and
/// never more than the whole envelope.
///
/// So a function asking for the smallest memory the model allows leaves room for
/// [`MAX_CONCURRENT_RUNS`] runs beside it, and one asking for the per-run ceiling for
/// [`CONCURRENT_RUNS_OF_THE_LARGEST_HEAP`] of them: the concurrency the substrate serves is
/// what the memory in flight allows, and it never falls below that count.
#[must_use]
pub fn run_memory_claim_mb(memory_size: i32) -> u32 {
    let bytes = u64::try_from(run_heap_bytes(memory_size)).unwrap_or(u64::MAX);
    let megabytes = u32::try_from(bytes / BYTES_PER_MB).unwrap_or(u32::MAX);
    megabytes.clamp(1, MAX_CONCURRENT_RUN_MEMORY_MB)
}

/// The longest a CALLER waits for the RUN of a handler, and the most of THIS HOST's own time
/// one run may be held for, whatever the function's own timeout says.
///
/// This is the HOST's ceiling, not the function's: a `Timeout` of up to 900 seconds would
/// otherwise hold a request open for a quarter of an hour. The span the run is JUDGED against
/// is the function's own clamped `Timeout` ([`run_budget`]) — which is what a run that outlasts
/// it is reported to have run past — while this is the host time it may really take, so a
/// handler configured for a longer span is still stopped here. A WAIT the handler asks for is
/// spent inside that span and really passes on this host, up to [`GUEST_WAIT_HOST_SPAN`].
///
/// The wait a caller makes runs from the moment this invocation holds the memory to run in —
/// beside the initialization of the execution environment the handler is called in, which is
/// waited for on its own span ([`INIT_PHASE_LIMIT`]) because the function's `Timeout` does not
/// bound it.
pub const HOST_WAIT_CEILING: Duration = Duration::from_secs(30);

/// How much of THIS HOST's own time the WAITS one run's handler asks for really spend.
///
/// A wait the handler asks for is part of what the function's `Timeout` allows it, so the span
/// it asked for is spent against that `Timeout` in full — a handler that waits past its
/// `Timeout` runs out of it exactly as it does on AWS. A wait is also how TIME GOES BY for the
/// caller: a client that observes the run while it is in flight, or that measures how long the
/// call took, sees the span the handler asked for, so a wait really passes up to here.
///
/// It is stated SHORTER than [`HOST_WAIT_CEILING`] rather than equal to it, because the rest
/// of that ceiling is what the handler's work AFTER the wait may take: a run whose waits could
/// spend the whole ceiling would be stopped on its deadline before returning, and a handler
/// that waits a quarter of an hour inside a `Timeout` that allows it answers here rather than
/// being held for a span its configuration never asked for. So a wait longer than this spends
/// this much of the host and the rest of it against the run's own span alone.
pub const GUEST_WAIT_HOST_SPAN: Duration = Duration::from_secs(20);

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

/// How long this emulator may spend STANDING UP the execution environment an invocation runs
/// in, before a line of the function's own code is read: building the interpreter or the
/// engine, narrowing its native surface and evaluating this emulator's own runtime.
///
/// EMULATOR-CHOSEN, and a span of neither the function's configuration nor of either phase of
/// a run: the two spans a run is judged against measure the RUN's own doing — the
/// `Timeout` bounds the handler's own phase and [`INIT_PHASE_LIMIT`] the code outside the
/// handler — so the work of creating the sandbox those phases happen in is charged to neither,
/// and a function whose handler and initialization code did nothing slow is never reported to
/// have run past its `Timeout` because this emulator was slow to build its interpreter.
/// It is not unbounded either: this is what an environment this process cannot stand up is
/// given up on after, and what the caller's own wait allows for beside the two phases.
///
/// Generous, because what it bounds is this process's own work on whatever host it is running
/// on: building an interpreter takes a fraction of it on an idle host and far more of it on one
/// running every other suite of a build beside it.
pub const ENVIRONMENT_BUILD_SPAN: Duration = Duration::from_secs(60);

/// How long ONE run may sit SUSPENDED on a callback of the durable execution it is the run of,
/// in all.
///
/// A durable execution's wait for something OUTSIDE itself is the execution's and not the
/// invocation's: the execution suspends until a client signals the callback or the window it was
/// configured with closes, which is why that span is charged to neither the function's `Timeout`
/// nor [`HOST_WAIT_CEILING`] ([`python::Bounds::suspend`]). What the suspension does hold is an
/// execution environment of THIS PROCESS, so how long this emulator keeps one open for a
/// callback nobody has signalled is bounded here.
///
/// EMULATOR-CHOSEN, like [`HOST_WAIT_CEILING`] beside it: no published span says how long a
/// durable execution may wait for a callback — `CallbackOptions$TimeoutSeconds` is the caller's
/// own window and `DurableConfig$ExecutionTimeout` reaches a year — so this is what a run is
/// given before the wait becomes a wait of its own and the run's bounds end it. It is generous
/// against the windows a client configures for work it is waiting on, and far shorter than the
/// span either of those members admits.
pub const CALLBACK_SUSPENSION_CEILING: Duration = Duration::from_secs(600);

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

/// The ACCOUNT an invoked function's ARN names, which is what the Extensions API reports as
/// the account the execution environment is running in.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account-id>:…`, so the account is its fifth
/// field. A value that is not an ARN of that shape names no account, and the account is then
/// this emulator's own — the one every ARN it mints carries.
#[must_use]
fn account_of(invoked_function_arn: &str) -> String {
    invoked_function_arn
        .split(':')
        .nth(4)
        .filter(|account| !account.is_empty())
        .map_or_else(
            || ::engine::emulator_account_id().to_string(),
            str::to_string,
        )
}

/// How many traced events one run may execute. Large enough for any handler a test or a
/// client runs, small enough that a loop with no exit ends inside the caller's wait.
const MAX_RUN_STEPS: u64 = 20_000_000;

/// How much memory one run may add to this process.
const MAX_RUN_MEMORY_BYTES: u64 = 512 * 1024 * 1024;

/// How many bytes of a function's EPHEMERAL STORAGE one execution environment really holds.
///
/// `/tmp` is a disk on the real runtime, and the files a run writes there are held in THIS
/// process's memory, so what one environment keeps for them is bounded here exactly as what
/// one run may allocate is ([`MAX_RUN_MEMORY_BYTES`]): a function configured for more
/// ephemeral storage than this reports its configured size — that is what it has — and the
/// writes this host can really hold stop at this figure. Every size up to it, the documented
/// default (`crate::state::DEFAULT_EPHEMERAL_STORAGE_MB`) included, is held in full.
pub const MAX_EPHEMERAL_HELD_BYTES: u64 = 1024 * 1024 * 1024;

/// The size of the `/tmp` directory a function configured for `ephemeral_storage_mb` MB has,
/// in bytes: what its invocations may write there and what a filesystem reading of it reports
/// ("The size of the function's /tmp directory in MB"), held to the range the member's own
/// documentation states.
#[must_use]
pub fn ephemeral_storage_bytes(ephemeral_storage_mb: i32) -> u64 {
    let clamped = i64::from(ephemeral_storage_mb).clamp(
        i64::from(crate::state::DEFAULT_EPHEMERAL_STORAGE_MB),
        i64::from(crate::state::MAX_EPHEMERAL_STORAGE_MB),
    );
    u64::try_from(clamped)
        .unwrap_or(1)
        .saturating_mul(BYTES_PER_MB)
}

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
///
/// This is the BUFFERED delivery's quota, which `InvokeMode` states as "BUFFERED … The
/// maximum payload size is 6 MB"; a run whose answer is delivered as a STREAM carries the
/// other one ([`MAX_STREAMED_RESPONSE_BYTES`]).
const MAX_RESPONSE_BYTES: usize = 6_291_556;

/// How large the answer of a run DELIVERED AS A STREAM may be: `InvokeMode` states it for
/// the response-streaming mode — "RESPONSE_STREAM … The maximum response payload size is
/// 200 MB" — which is the quota a caller that asked for that delivery gets, whichever
/// seam asked for it ([`Invocation::streamed`]).
pub const MAX_STREAMED_RESPONSE_BYTES: usize = 200 * 1024 * 1024;

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
    /// The function's `EphemeralStorage.Size`, in MB: how large the `/tmp` directory the
    /// invocation writes to is.
    pub ephemeral_storage_mb: i32,
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
    /// The AMAZON COGNITO IDENTITY this invocation was made BY, resolved off the credential
    /// the invoking call was signed with ([`super::caller_cognito_identity`]), and `None` for
    /// an invocation made by a credential no identity pool vended — which is what the runtime
    /// reports as a `context.identity` carrying no identity at all.
    pub cognito_identity: Option<crate::state::InvocationIdentity>,
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
    /// Whether this invocation's answer is DELIVERED AS A STREAM: the caller asked for the
    /// response-streaming delivery (`InvokeWithResponseStream`, or a function URL whose
    /// `InvokeMode` is `RESPONSE_STREAM`) rather than the buffered one.
    ///
    /// It is what decides how much the run may answer: `InvokeMode` gives "BUFFERED … The
    /// maximum payload size is 6 MB" against "RESPONSE_STREAM … The maximum response
    /// payload size is 200 MB", so the quota is the DELIVERY's
    /// ([`MAX_STREAMED_RESPONSE_BYTES`]) and a streamed answer is not bounded by what one
    /// buffered response carries.
    pub streamed: bool,
    /// The FILE SYSTEMS this invocation's execution environment MOUNTS: each Amazon EFS access
    /// point the function's `FileSystemConfigs` name, resolved to the access point's own root
    /// directory, the identity requests through it are made as, and the files behind it
    /// ([`super::file_system_mounts`]).
    ///
    /// Resolved by the CALLER, where the rest of the invocation is assembled, and carried here
    /// because the run happens on a thread of its own. Empty for a function that configures no
    /// file system, and for one whose configuration names an access point this emulator's
    /// Amazon EFS does not hold — such a path is then no path of the execution environment at
    /// all.
    ///
    /// [`super::MountDenied`] for a function whose execution role may not MOUNT the file
    /// system it configured: no execution environment can be built for such an invocation, so
    /// the run never happens and the mount failure is what the calling operation answers
    /// ([`Unrunnable::FileSystemUnmountable`]).
    pub file_systems: Result<Vec<super::FileSystemMount>, super::MountDenied>,
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
    /// The bytes it holds are not a readable archive.
    PackageUnreadable(String),
    /// The function's execution role may not MOUNT the Amazon EFS file system its
    /// `FileSystemConfigs` named, so the execution environment this invocation would have run
    /// in could not be built at all ([`super::file_system_mounts`]).
    FileSystemUnmountable {
        /// The access point the configuration named, as the refusal names it.
        access_point_arn: String,
    },
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
    /// How much of the span the handler asked to WAIT for no time of this host passed over,
    /// so the EXECUTION this invocation is lasted that much longer than
    /// [`Invoked::duration`] ([`outcome::Finished::unwaited`]).
    ///
    /// It is what an invocation still holds the function's concurrency for once its run has
    /// answered ([`super::execution_slot::hold_for_the_rest_of_the_execution`]): the slot an
    /// admitted invocation takes is held until that invocation is done, and an execution is
    /// done when the span it spent against the function's `Timeout` is over. Zero for a run
    /// this host waited out in full.
    pub unwaited: Duration,
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
    /// WHICH bound the PLATFORM ended this run at, for a run it ended, and `None` for one
    /// the handler itself completed — one that returned, streamed or raised, whose outcome
    /// is its own answer rather than a classification of the platform's.
    ///
    /// It is the same reading the answer's own `errorType` is rendered from
    /// ([`stopped_class`]), kept beside the answer because a caller that has to tell a run
    /// the platform STOPPED from a handler that failed cannot read it back out of the
    /// document's text: the run of a durable execution is settled on the one or the other
    /// ([`super::durable_run::run_outcome`]).
    pub stopped: Option<Stop>,
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
/// `durable` is where the run reports the STEPS of the durable execution it IS the run of
/// ([`super::durable_steps`]), for an invocation that started one, and `None` for every other
/// invocation — whose handler is the same handler, run the same way, and whose durable SDK
/// has no execution to checkpoint on.
///
/// The run happens on a thread of its own, holding its own share of this module's run memory
/// and no state guard of this service, and the caller's wait for it is bounded by
/// [`HOST_WAIT_CEILING`] however long the function's own timeout is.
pub async fn run_invocation(
    mut invocation: Invocation,
    durable: Option<super::durable_steps::DurableStepJournal>,
) -> Result<Invoked, Unrunnable> {
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
    // The file systems this invocation's environment MOUNTS, which is part of BUILDING that
    // environment: a function whose execution role may not mount the file system it configured
    // has no environment to run in at all, so nothing is run and the caller is answered the
    // mount failure rather than a result no handler produced
    // ([`super::file_system_mounts`]). Taken OUT of the invocation rather than copied out of
    // it: the handles travel on to the run below, and nothing else reads the field.
    let file_systems = match ::std::mem::replace(&mut invocation.file_systems, Ok(Vec::new())) {
        Ok(mounts) => mounts,
        Err(denied) => {
            return Err(Unrunnable::FileSystemUnmountable {
                access_point_arn: denied.access_point_arn,
            })
        }
    };
    // The AWS SDK a managed runtime ships reads the invocation's own tracing context out of
    // the environment and stamps it on every request it makes, so the seam the handler calls
    // through carries this invocation's header ([`aws_api::AwsAccess::tracing`]). That is
    // what makes the call a handler makes a HOP OF THIS CHAIN to the service that serves it,
    // which is what the recursive-loop guardrail counts (`super::recursion`).
    if let Some(header) = invocation.trace.as_ref().map(|trace| trace.header.clone()) {
        invocation.aws = invocation.aws.take().map(|aws| aws.tracing(header));
    }
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
    // The METADATA SESSION of that environment: the token it was handed and the ones its own
    // metadata service issues, which every invocation served in it reads that service through
    // ([`instance_metadata::session_of`]).
    let metadata_session = instance_metadata::session_of(&environment_key);
    // The RUNTIME API this run addresses is served from the EXECUTION ENVIRONMENT's own state
    // ([`runtime_api`]): the extensions registered in it, against the function it is running.
    // The environment is this invocation's, so the state is too, and the facts the Extensions
    // API reports about the function are read off the invocation itself.
    let sandbox = runtime_api::Sandbox::new(runtime_api::FunctionFacts {
        function_name: invocation.function_name.clone(),
        function_version: invocation.executed_version.clone(),
        handler: handler.clone(),
        account_id: account_of(&invocation.invoked_function_arn),
        // WHERE this environment is running, which is what its own metadata service publishes
        // to a handler inside it ([`instance_metadata`]): the Region the invocation was
        // addressed to, and the architecture the function is configured to run on.
        region: invocation.region.clone(),
        architecture: invocation.architecture.clone(),
    })
    // The METADATA SESSION it reads that service through is the ENVIRONMENT's, not this one
    // invocation's: a session token stays good for the lifetime it was issued for however many
    // invocations the environment that minted it goes on to serve, which is what a handler that
    // mints one outside its own body relies on ([`instance_metadata::session_of`]).
    .in_environment(Arc::clone(&metadata_session));
    invocation.aws = invocation.aws.take().map(|aws| aws.in_sandbox(sandbox));

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
        // A package the store holds NO bytes for holds no module either, which is the same
        // thing to the runtime as an archive carrying no entry the configured handler names:
        // the initialization phase gets no module to import, so the run reports the import
        // failure that runtime words for it. It is not the modelled failure for an archive
        // that could not be unzipped — "Lambda could not unzip the deployment package" is
        // about bytes that are no readable .zip, which is the arm below.
        Err(package::PackageFault::NotHeld) => Vec::new(),
        Err(package::PackageFault::Unreadable(why)) => {
            return Err(Unrunnable::PackageUnreadable(why))
        }
    };
    // The package's own DATA entries, under the prefix a resource is keyed by: the files a
    // package ships beside its modules and reads back rather than imports, which the
    // standard library's own resource reading answers from ([`python::RESOURCE_ID_PREFIX`]).
    // Read out of what is LEFT of the same budget, after the modules, so the modules of a
    // package whose data is large are read in full. A read that answers a FAULT contributes
    // nothing and is not the invocation's: the modules above are what decide whether this
    // package can run at all, so a package whose data the budget had no room for still runs
    // its handler, exactly as one that ships no data does.
    if engine == Engine::Python {
        if let Ok(entries) =
            package::python_resources_within(invocation.zip_bytes.as_deref(), &mut read_budget)
        {
            modules.extend(entries.into_iter().map(|entry| package::PackageModule {
                path: format!("{}{}", python::RESOURCE_ID_PREFIX, entry.path),
                source: entry.source,
            }));
        }
        // And the entries whose bytes are NO text at all — an archive, an image, a packed
        // table: files of the package exactly as the ones above are, held as the bytes they
        // are ([`python::BINARY_ID_PREFIX`]) so the mount carries the file the archive really
        // carried rather than leaving it out of the environment.
        if let Ok(entries) =
            package::python_binaries_within(invocation.zip_bytes.as_deref(), &mut read_budget)
        {
            modules.extend(entries.into_iter().map(|entry| package::PackageModule {
                path: format!("{}{}", python::BINARY_ID_PREFIX, entry.path),
                source: entry.source,
            }));
        }
    }
    // The layers' own entries, under the prefix the runtime keys the layer root by, read
    // out of the SAME budget the deployment package was: what one invocation holds is
    // bounded across its mounts rather than per archive. A layer this emulator holds no
    // readable archive for mounts nothing, exactly as a deployment package it holds no
    // bytes for imports nothing. Each engine mounts the entries IT can read, as the
    // deployment package's were: a layer's content is the caller's files, whichever
    // language the function is written in.
    for archive in &invocation.layer_zips {
        match engine {
            Engine::Node => {
                if let Ok(entries) =
                    package::javascript_modules_within(Some(archive.as_slice()), &mut read_budget)
                {
                    modules.extend(entries.into_iter().map(|entry| package::PackageModule {
                        path: format!("{}{}", node::LAYER_ID_PREFIX, entry.path),
                        source: entry.source,
                    }));
                }
            }
            // A Python layer mounts both halves of its content for the same reason the
            // deployment package does: the modules its import roots put on the path, and
            // the files its packages ship beside them, which the standard library's own
            // resource reading answers from.
            Engine::Python => {
                if let Ok(entries) =
                    package::python_modules_within(Some(archive.as_slice()), &mut read_budget)
                {
                    modules.extend(entries.into_iter().map(|entry| package::PackageModule {
                        path: format!("{}{}", python::LAYER_ID_PREFIX, entry.path),
                        source: entry.source,
                    }));
                }
                if let Ok(entries) =
                    package::python_resources_within(Some(archive.as_slice()), &mut read_budget)
                {
                    modules.extend(entries.into_iter().map(|entry| package::PackageModule {
                        path: format!(
                            "{}{}{}",
                            python::LAYER_ID_PREFIX,
                            python::RESOURCE_ID_PREFIX,
                            entry.path
                        ),
                        source: entry.source,
                    }));
                }
                // And the layer's own entries whose bytes are no text at all, which a layer
                // shipping a packed asset or an executable is made of.
                if let Ok(entries) =
                    package::python_binaries_within(Some(archive.as_slice()), &mut read_budget)
                {
                    modules.extend(entries.into_iter().map(|entry| package::PackageModule {
                        path: format!(
                            "{}{}{}",
                            python::LAYER_ID_PREFIX,
                            python::BINARY_ID_PREFIX,
                            entry.path
                        ),
                        source: entry.source,
                    }));
                }
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

    let mut budget = run_budget(
        invocation.timeout,
        invocation.memory_size,
        invocation.ephemeral_storage_mb,
        // The DELIVERY this invocation was made for, which is what its answer is bounded by
        // ([`Invocation::streamed`]).
        invocation.streamed,
    );
    // The run of a DURABLE EXECUTION is bounded by the EXECUTION's own timeout instead of by
    // the function's: `DurableConfig$ExecutionTimeout` is "the maximum time (in seconds) that
    // a durable execution can run before timing out. This timeout applies to the entire
    // durable execution, not individual function invocations" — and this run IS that whole
    // execution, because every wait, step and callback its handler takes is spent inside it
    // ([`durable_sdk_guest`]). So what the run may spend is what is LEFT of that timeout, and
    // the span one invocation of the function is allowed never ends an execution that still
    // fits inside its own: an execution that outlives its `ExecutionTimeout` is ended by that
    // timeout, as the `TIMED_OUT` it owes ([`super::DurableExecutionRecord::materialize`]).
    // A durable function whose configuration names no timeout bounds its runs by the
    // function's `Timeout` as every other invocation is bounded.
    if let Some(left) = durable
        .as_ref()
        .and_then(|journal| journal.execution_timeout_left(SystemTime::now()))
    {
        budget.wall_clock = left;
    }
    let heap_bytes = run_heap_bytes(invocation.memory_size);
    let started = SystemTime::now();
    // Whether this invocation is the run of a DURABLE EXECUTION, which is what decides the
    // span its wait below allows for a run sitting SUSPENDED on a callback of that execution
    // ([`CALLBACK_SUSPENSION_CEILING`]). Read before the journal travels onto the run's own
    // thread.
    let runs_a_durable_execution = durable.is_some();
    let request_id = invocation.request_id.clone();
    let reported = invocation.clone();
    let (sender, receiver) = ::tokio::sync::oneshot::channel();
    let lifecycle = Arc::new(AtomicU8::new(RUN_RUNNING));
    let thread_lifecycle = Arc::clone(&lifecycle);
    let environment = runtime_environment(&invocation, started, &metadata_session);
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
                        architecture: invocation.architecture.clone(),
                        event,
                        request_id: invocation.request_id.clone(),
                        invoked_function_arn: invocation.invoked_function_arn.clone(),
                        client_context: invocation.client_context.clone(),
                        tenant_id: invocation.tenant_id.clone(),
                        cognito_identity: invocation.cognito_identity.clone(),
                        budget,
                        aws: invocation.aws.clone(),
                        durable,
                        file_systems,
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
                    cognito_identity: invocation.cognito_identity.as_ref(),
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
        )
        // And beside both phases, the span this emulator may spend STANDING THE ENVIRONMENT
        // UP ([`ENVIRONMENT_BUILD_SPAN`]): that work is charged to neither phase, so a wait
        // that did not allow for it would give up on a run whose own bounds had not been
        // reached — the very failure the function's configuration never asked for.
        .saturating_add(ENVIRONMENT_BUILD_SPAN)
        // And, for the run of a DURABLE EXECUTION, the span that execution may sit SUSPENDED
        // on a callback of itself ([`CALLBACK_SUSPENSION_CEILING`]): that span is the
        // execution's rather than the run's, so a wait that did not allow for it would give up
        // on a run whose handler is still waiting for a callback a client may yet signal —
        // and the completion of the execution would be reported as a run nothing finished.
        .saturating_add(if runs_a_durable_execution {
            CALLBACK_SUSPENSION_CEILING
        } else {
            Duration::ZERO
        });
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

    // WHICH log stream this run's records belong to: the one the EXECUTION ENVIRONMENT that
    // served it writes to, which is the environment's own for as long as it stands
    // ([`warm_pool`]) — so the invocations a standing environment serves share one stream and
    // a new environment opens one of its own. A run whose environment was built for it alone
    // writes to the stream its own invocation composed.
    let log_stream = log_stream_of(&reported, &finished, started);
    let invoked = answered(
        finished,
        &request_id,
        &reported,
        budget,
        engine,
        started,
        &log_stream,
    );
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
        log_delivery::deliver(aws, target, &reported.region, &log_stream, &invoked.log_events)
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
///
/// `log_stream` is the stream the EXECUTION ENVIRONMENT that served this run writes to
/// ([`log_stream_of`]), handed in rather than read again here so the record that names it and
/// the delivery that writes to it cannot drift.
fn answered(
    finished: Finished,
    request_id: &str,
    invocation: &Invocation,
    budget: Budget,
    engine: Engine,
    started: SystemTime,
    log_stream: &str,
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
            initialization,
        } => (
            {
                let document = error_document(
                    error_message,
                    Some(error_type),
                    // A failure the runtime raised while it was still INITIALIZING the
                    // execution environment belongs to no invocation: the environment is
                    // initialized before a request is bound to it, so the document names no
                    // request at all. The phase is the one the engine that drove it reported
                    // ([`python`]) rather than a reading of the error's class, because an
                    // initialization fails under whatever class the deployment package's own
                    // top-level code raised; where an engine reports no phase, the classes
                    // its runtime's initialization words itself name it
                    // ([`INITIALIZATION_ERROR_TYPES`]).
                    if *initialization || raised_during_initialization(error_type) {
                        ""
                    } else {
                        request_id
                    },
                    stack_trace,
                    engine,
                );
                // An error document larger than an invocation may answer with is refused for
                // its SIZE, like any other response of that size: what the caller is
                // answered is the response-size refusal rather than the oversize document.
                if document.len() > budget.response_bytes {
                    oversize_response_document(budget.response_bytes)
                } else {
                    document
                }
            },
            Some(FUNCTION_ERROR_UNHANDLED.to_string()),
        ),
        // A run the PLATFORM ended at one of its bounds: the document states what happened
        // ([`stopped_document`]) and names WHICH bound ended it ([`stopped_class`]) — the
        // same class the run's own REPORT record states beside its status
        // ([`log_records::report`]), so a client reading the answer and one reading the log
        // read ONE classification of the run.
        Completion::Stopped(stop) => (
            stopped_document(*stop, request_id, budget),
            Some(FUNCTION_ERROR_UNHANDLED.to_string()),
        ),
        Completion::Internal(why) => (
            error_document(
                why,
                Some(RUNTIME_INTERNAL_ERROR),
                request_id,
                &[],
                engine,
            ),
            Some(FUNCTION_ERROR_UNHANDLED.to_string()),
        ),
    };
    let log = run_log(
        request_id,
        invocation,
        // The stream the environment that served this run writes to, which the record of its
        // initialization names as the instance it is.
        log_stream,
        started,
        // The FORM the records of this run take, which the function's own `LoggingConfig`
        // decides ([`log_records`]): the same reading the runtime that rendered the
        // handler's own records made, because both read it off this invocation's target.
        json_log_records(invocation),
        // The answer the invocation carries, which is what the runtime produced for it.
        payload.len(),
        &finished,
    );
    Invoked {
        payload,
        function_error,
        log_events: log_delivery::events(&log.records, log.opened, started, finished.elapsed),
        log: log.text(),
        duration: finished.elapsed,
        // What the run asked to wait for that no host time passed over: the span of this
        // execution that is still to pass now the run has answered.
        unwaited: finished.unwaited,
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
        // WHICH bound ended the run, for a run the platform ended at one of them.
        stopped: match &finished.completion {
            Completion::Stopped(stop) => Some(*stop),
            _ => None,
        },
    }
}

/// The error payload a run the PLATFORM ended at one of the function's bounds answers with:
/// the sentence that states what happened, under the class that names the bound
/// ([`stopped_class`]).
fn stopped_document(stop: Stop, request_id: &str, budget: Budget) -> Vec<u8> {
    match stop {
        // A run the RESPONSE ceiling ended produced an answer Lambda will not carry, and that
        // refusal is the RESPONSE's rather than the run's: Lambda states it the same way
        // whichever side produced the oversize answer, so it is worded in the one place both
        // are answered from ([`oversize_response_document`]).
        Stop::Response => oversize_response_document(budget.response_bytes),
        other => sandbox_error_document(
            &stopped_message(other, request_id, budget),
            stopped_class(other),
        ),
    }
}

/// What the PLATFORM states about a run IT ended at one of the function's bounds: the
/// sentence the error envelope carries, in the frame every failure the SANDBOX reports an
/// invocation with is stated in — `RequestId: <id> Error: <what happened>`.
fn stopped_message(stop: Stop, request_id: &str, budget: Budget) -> String {
    match stop {
        // The run outlasted the function's own `Timeout`: the message states the invocation
        // and the span it ran past.
        Stop::Deadline => format!(
            "RequestId: {request_id} Error: Task timed out after {:.2} seconds",
            budget.wall_clock.as_secs_f64()
        ),
        // A run the memory ceiling ended is one whose runtime was killed, which is what AWS
        // reports it as: "Runtime exited with error: signal: killed".
        Stop::Memory => format!("RequestId: {request_id} Error: {RUNTIME_KILLED}"),
        // Any other bound of the run is reported as the bound it is. The response quota is
        // worded by [`stopped_document`], where the document both sides of it are answered
        // from is built.
        other => other.reason().to_string(),
    }
}

/// The class the platform reports a run it ended at `stop` under.
///
/// A bound the platform can NAME is reported under its own class — the memory ceiling as the
/// out-of-memory kill it is, the function's `Timeout` as the sandbox timeout, an answer past
/// the response quota as that quota's failure — and a run whose runtime ended for a reason
/// the platform cannot state is reported under the class of a runtime that exited
/// ([`RUNTIME_EXIT_ERROR`]).
fn stopped_class(stop: Stop) -> &'static str {
    match stop {
        Stop::Deadline => SANDBOX_TIMEDOUT,
        Stop::Memory => RUNTIME_OUT_OF_MEMORY,
        Stop::Response => RESPONSE_SIZE_TOO_LARGE,
        Stop::Steps | Stop::HostCeiling => RUNTIME_EXIT_ERROR,
    }
}

/// The class the platform reports a run IT ended under, or `None` for a run the HANDLER
/// itself completed — one that returned, streamed or raised, whose outcome is in its own
/// answer rather than in a classification of the platform's.
///
/// ONE reading, so the `errorType` of the error envelope an invocation answers with and the
/// class its REPORT record states beside its status ([`log_records::report`]) cannot name two
/// different failures of one run.
fn platform_error_class(completion: &Completion) -> Option<&'static str> {
    match completion {
        Completion::Stopped(stop) => Some(stopped_class(*stop)),
        Completion::Internal(_) => Some(RUNTIME_INTERNAL_ERROR),
        Completion::Returned(_) | Completion::Streamed { .. } | Completion::Raised { .. } => None,
    }
}

/// The `errorType` of a run whose runtime ended instead of answering.
const RUNTIME_EXIT_ERROR: &str = "Runtime.ExitError";

/// The `errorType` of a run this emulator's own interpreter could not be built or driven for,
/// which is no failure of the handler's.
const RUNTIME_INTERNAL_ERROR: &str = "Runtime.InternalError";

/// The `errorType` of a run the platform ended because it held more memory than the
/// function's own `MemorySize` allows. Running out of memory is a failure the platform can
/// NAME, so it is reported under its own class rather than under the class of a runtime that
/// ended for an unstated reason ([`RUNTIME_EXIT_ERROR`]).
const RUNTIME_OUT_OF_MEMORY: &str = "Runtime.OutOfMemory";

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

/// The `errorType` classes the NODE runtime reports for a failure of the INITIALIZATION
/// phase: the phase in which it reads the configured handler name, loads the module that
/// name resolves to, and finds the entry point on it. Each is raised by the runtime itself
/// before it has an invocation in hand — `MalformedHandlerName` for a name it cannot split,
/// `UserCodeSyntaxError` for a module that does not compile, `ImportModuleError` for one
/// that does not load, and `HandlerNotFound` for a module that does not define the entry
/// point — which is what makes a failure of any of them one no invocation is bound to.
///
/// It is a FALLBACK reading, for an engine that reports no phase of its own
/// ([`node`]): where the engine states the phase it was in
/// (`Completion::Raised { initialization }`, which [`python`] does), that statement decides,
/// and this list is not consulted for it. Nothing is added here for the Python runtime's
/// own initialization classes — a package whose top-level code raised fails under its OWN
/// exception class, which no list of names can enumerate, and that is exactly why the phase
/// is reported rather than inferred.
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
/// own runtime reports for a handler that raised, written as that runtime's own JSON
/// serializer writes them.
///
/// It is the TEXT of a document a runtime produced, not a document this process produced:
/// what reads it back — a caller of `Invoke`, and a Step Functions `Task` state whose
/// `Resource` named the function, which reports these very bytes as the failure's cause —
/// reads the member order and the separators the runtime wrote ([`separators`]), so both
/// are stated here rather than left to this process's own JSON library. The order is the
/// runtime's: the message, the class, the invocation the run answered where that runtime
/// names it, then the handler's stack.
fn error_document(
    message: &str,
    error_type: Option<&str>,
    request_id: &str,
    stack_trace: &[String],
    engine: Engine,
) -> Vec<u8> {
    let frames: Vec<::serde_json::Value> = stack_trace
        .iter()
        .map(|frame| ::serde_json::Value::String(frame.clone()))
        .collect();
    let mut members: Vec<(&str, ::serde_json::Value)> = vec![(
        "errorMessage",
        ::serde_json::Value::String(message.to_string()),
    )];
    if let Some(error_type) = error_type {
        members.push((
            "errorType",
            ::serde_json::Value::String(error_type.to_string()),
        ));
    }
    if reports_request_id(engine) {
        members.push((
            "requestId",
            ::serde_json::Value::String(request_id.to_string()),
        ));
    }
    members.push((
        stack_trace_member(engine),
        ::serde_json::Value::Array(frames),
    ));
    object_text(&members, separators(engine)).into_bytes()
}

/// How a runtime's own JSON serializer writes the two separators of a document: what it
/// puts between two members, and what it puts between a member's name and its value.
///
/// The error document an invocation answers with is TEXT the function's runtime wrote, so
/// it is written here the way that runtime's serializer writes it rather than the way this
/// process's happens to: a Python runtime marshals a document through the standard
/// library's own JSON encoder, whose separators carry a space (`", "` and `": "`), while a
/// node runtime marshals it through `JSON.stringify`, which writes neither.
fn separators(engine: Engine) -> (&'static str, &'static str) {
    match engine {
        Engine::Python => (", ", ": "),
        Engine::Node => (",", ":"),
    }
}

/// `members`, in the order given, as ONE JSON object's text, with `between` between two
/// members and `after_name` after each member's name.
///
/// The member ORDER and the separators are facts of whoever wrote the document, so the
/// text is assembled here rather than left to a `Map`'s own ordering — which is the
/// ordering of this process's JSON library rather than of the writer being emulated.
/// Each value is rendered by that library, because the ESCAPING of a value is the same
/// in every JSON writer.
fn object_text(members: &[(&str, ::serde_json::Value)], separators: (&str, &str)) -> String {
    let (between, after_name) = separators;
    let rendered: Vec<String> = members
        .iter()
        .map(|(name, value)| {
            let name = ::serde_json::Value::String((*name).to_string());
            let value = match value {
                ::serde_json::Value::Array(elements) => {
                    let elements: Vec<String> =
                        elements.iter().map(::std::string::ToString::to_string).collect();
                    format!("[{}]", elements.join(between))
                }
                value => value.to_string(),
            };
            format!("{name}{after_name}{value}")
        })
        .collect();
    format!("{{{}}}", rendered.join(between))
}

/// The error payload an invocation whose answer was larger than one may be is refused with:
/// the response-size class and the quota the answer passed, in bytes.
///
/// It is what the caller is answered whichever side produced that answer — the payload a
/// handler returned, or the document the runtime wrote for an error it raised — because the
/// quota is the RESPONSE's and Lambda states it the same way for both.
fn oversize_response_document(response_bytes: usize) -> Vec<u8> {
    sandbox_error_document(
        &format!(
            "Response payload size exceeded maximum allowed payload size ({response_bytes} bytes)."
        ),
        RESPONSE_SIZE_TOO_LARGE,
    )
}

/// The error payload an invocation the SANDBOX ended answers with: the message and the
/// class of the failure, and nothing besides.
///
/// A run the sandbox ended — at the function's `Timeout`, at its memory ceiling, at the
/// size of the answer it produced, at a runtime that exited — raised no exception in the
/// function: the runtime serialized nothing for it, so the document is the PLATFORM's
/// rather than a runtime's ([`error_document`] is a runtime's). It carries neither the
/// invocation's id nor a stack, whatever runtime the function is configured for: the
/// platform states what happened in the message and what class of failure it was, and
/// there is no handler frame to report because no handler raised.
fn sandbox_error_document(message: &str, error_type: &str) -> Vec<u8> {
    let members: Vec<(&str, ::serde_json::Value)> = vec![
        (
            "errorType",
            ::serde_json::Value::String(error_type.to_string()),
        ),
        (
            "errorMessage",
            ::serde_json::Value::String(message.to_string()),
        ),
    ];
    // The platform writes its own document, naming the CLASS of the failure before the
    // sentence about it, and writes it compactly — it is not marshalled by the runtime
    // the function is configured for, so no runtime's separators are used for it.
    object_text(&members, (",", ":")).into_bytes()
}

/// The LOG of one run, as Lambda writes it: what the INITIALIZATION of the execution
/// environment wrote, then the invocation's start and end lines around what the handler
/// wrote, and the report line that states what the run spent.
///
/// Every number in it is measured: the duration is the run's own, the memory size is what
/// the run was allowed to hold, and the memory used is the most the engine running it
/// measured it holding (see [`python`], [`node`]).
///
/// It is kept as its RECORDS because that is what CloudWatch Logs holds it as — one event per
/// record ([`log_delivery`]) — and the whole text an invocation answers a log tail from is
/// those same records.
#[derive(Debug)]
struct RunLog {
    /// Every record of the log, in order and AS IT WAS WRITTEN: the frames Lambda writes
    /// itself each closed by their own break, and each record the run wrote exactly as the
    /// run wrote it ([`outcome::Capture`]).
    records: Vec<String>,
    /// How many of those records are the ENVIRONMENT's rather than this invocation's: the
    /// record that states the initialization of the execution environment, which Lambda writes
    /// to the function's log group ahead of the first invocation that environment serves and
    /// which is no part of what that invocation reports as its own log ([`Self::text`]).
    environment: usize,
    /// How many of those records OPEN the log: the framing Lambda writes before the handler's
    /// own output, and the initialization's own output ahead of it.
    opened: usize,
}

impl RunLog {
    /// The log THIS INVOCATION wrote, as text: its records, each on a line of its own — which
    /// is what `LogResult` reports ("the last 4 KB of the execution log"). A record the run
    /// closed with no break of its own still ends where the next one begins, because a client
    /// reads the log as the records it holds.
    ///
    /// The record that states the initialization of the execution environment is the
    /// ENVIRONMENT's, so it is no part of this: the output the init phase itself wrote IS,
    /// because the invocation that built the environment is the one that paid for it.
    fn text(&self) -> String {
        let mut text = String::new();
        for record in &self.records[self.environment.min(self.records.len())..] {
            text.push_str(record);
            if !record.ends_with(log_delivery::RECORD_TERMINATOR) {
                text.push(log_delivery::RECORD_TERMINATOR);
            }
        }
        text
    }
}

/// One frame Lambda writes itself, as the record it is: the frame and the terminator that
/// closes it, which is what the runtime writes to its output stream
/// ([`log_delivery::RECORD_TERMINATOR`]).
fn framed(frame: String) -> String {
    format!("{frame}{}", log_delivery::RECORD_TERMINATOR)
}

fn run_log(
    request_id: &str,
    invocation: &Invocation,
    log_stream: &str,
    started: SystemTime,
    json: bool,
    produced_bytes: usize,
    finished: &Finished,
) -> RunLog {
    let version = &invocation.executed_version;
    let memory_size = invocation.memory_size;
    let duration_ms = finished.elapsed.as_secs_f64() * 1_000.0;
    // What the run is BILLED for, in whole milliseconds: the duration rounded up, and never
    // less than the unit it is stated in. Taken from the measured elapsed time rather than
    // from the figure above, so the two formats state one whole number.
    let billed_ms =
        i64::try_from(finished.elapsed.as_micros().div_ceil(1_000).max(1)).unwrap_or(i64::MAX);
    let used_mb = max_memory_used_mb(finished.max_memory_bytes, memory_size);
    let ended = started.checked_add(finished.elapsed).unwrap_or(started);
    let mut records = Vec::new();
    // A run that BUILT the execution environment it ran in opens the environment's log with
    // the initialization: the record that states which runtime version the environment stands
    // on, and then whatever the module-level code of the package itself wrote. Both precede
    // the invocation's own opening frame, which is how a client attributes module-scope output
    // to the cold start; an invocation a standing environment served pays no initialization
    // and writes neither.
    if finished.initialized {
        // WHICH BUILD of the managed runtime the environment stands on, in the form AWS names
        // a runtime version by (`crate::state::runtime_build`): the runtime identifier names
        // the runtime, while a record of an environment names the build of it that
        // initialised, which is what a client correlating a pinned runtime version with the
        // build that ran reads. An identifier naming no version line at all is reported as
        // itself, because there is no build of a line to name.
        let runtime = invocation.runtime.as_deref().unwrap_or_default();
        let runtime_build =
            super::runtime_build(runtime).unwrap_or_else(|| runtime.to_string());
        records.push(framed(log_records::init_start(
            json,
            started,
            &log_records::Initialization {
                initialization_type: invocation.initialization_type,
                function_name: &invocation.function_name,
                function_version: version,
                // The runtime version the environment was built on, and the ARN that version
                // is addressed by — the same reading a read of the function reports as its
                // `RuntimeVersionConfig` (`crate::state::runtime_version_arn`), derived from
                // the same digest as the build above.
                runtime_version: &runtime_build,
                runtime_version_arn: &super::runtime_version_arn(&invocation.region, runtime),
                instance_id: log_stream,
                // The memory an environment of this function may hold, in the bytes the
                // record states it in: the function's own `MemorySize`, in MB.
                instance_max_memory: i64::from(memory_size)
                    .saturating_mul(i64::try_from(BYTES_PER_MB).unwrap_or(1)),
            },
        )));
        records.extend(finished.init_records.iter().cloned());
        // ... and the records that CLOSE that phase, which the structured format states as
        // records of their own: the runtime reporting its half of the phase done, and the
        // report of what the phase spent ([`log_records::init_done`]). Under the plain-text
        // format the same span is on this run's own `REPORT` line instead, so there is
        // nothing to write here.
        if let (true, Some(took)) = (json, finished.init_duration) {
            let done = started.checked_add(took).unwrap_or(started);
            records.extend(
                log_records::init_done(
                    done,
                    &log_records::InitDone {
                        initialization_type: invocation.initialization_type,
                        took,
                        failed: matches!(&finished.completion, Completion::Raised { error_type, .. }
                            if raised_during_initialization(error_type)),
                    },
                )
                .into_iter()
                .map(framed),
            );
        }
    }
    // The initialization record is the one record of the ENVIRONMENT's log above: everything
    // after it is the invocation's own.
    let environment = usize::from(finished.initialized);
    records.push(framed(log_records::start(
        json,
        started,
        request_id,
        // The function this invocation ran, as the frame names it: the function's own ARN,
        // beside the version that executed — the one reading of a function's ARN this crate
        // has (`crate::state::function_arn`).
        &super::function_arn(&invocation.region, &invocation.function_name),
        version,
    )));
    let opened = records.len();
    // One record of the log per RECORD the run wrote — a raw stream write framed at its line
    // breaks, a record the runtime framed whole ([`outcome::Finished::log_records`]) — so what
    // the handler reported in one call is one event of the function's log group.
    records.extend(finished.log_records.iter().cloned());
    records.push(framed(log_records::runtime_done(
        json,
        ended,
        request_id,
        &finished.completion,
        log_records::RuntimeDone {
            duration_ms,
            produced_bytes,
        },
    )));
    records.push(framed(log_records::report(
        json,
        ended,
        request_id,
        &finished.completion,
        log_records::Report {
            duration_ms,
            billed_ms,
            memory_size,
            used_mb,
            // What the initialization of this run's own execution environment took, for a run
            // that paid one: an invocation an already-initialized environment served states
            // no such span, which is how a client tells a cold start from a warm one.
            init_ms: finished
                .init_duration
                .map(|spent| spent.as_secs_f64() * 1_000.0),
        },
    )));
    RunLog {
        records,
        environment,
        opened,
    }
}

/// Whether the records of this invocation's log are the structured ones: the function's own
/// `LoggingConfig$LogFormat`, as the target the invocation was accepted with read it
/// ([`log_delivery::log_target`]).
///
/// An invocation whose caller resolved no target at all logs to no group, and its records keep
/// the runtime's plain-text framing — the format a function that names none carries.
fn json_log_records(invocation: &Invocation) -> bool {
    invocation
        .logs
        .as_ref()
        .is_some_and(|target| target.log_format == super::LOG_FORMAT_JSON)
}

/// What the report states the run USED, in whole megabytes: the memory the engine measured
/// this run holding ([`Finished::max_memory_bytes`]), rounded up to the unit the field is
/// stated in and never beyond the memory the function is configured for, which is all the
/// run was allowed to hold.
///
/// A run that executed code HELD memory, so the smallest figure the field can state for one
/// is the unit itself: a reading below a megabyte — which is what a warm execution
/// environment serving a handler that allocates nothing of its own reads — is reported as
/// that one megabyte rather than as none. A run nothing measured because nothing ran is the
/// one case stated as no memory at all, because a figure for a run that did not happen would
/// be one no engine took.
fn max_memory_used_mb(max_memory_bytes: Option<u64>, memory_size: i32) -> i64 {
    let Some(measured) = max_memory_bytes else {
        return 0;
    };
    let configured = i64::from(memory_size).clamp(MIN_MEMORY_SIZE_MB, MAX_MEMORY_SIZE_MB);
    let used = i64::try_from(measured.div_ceil(BYTES_PER_MB)).unwrap_or(configured);
    used.clamp(1, configured)
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
/// allows for it, and the memory its own `MemorySize` asks for ([`run_heap_bytes`]), plus the
/// bounds that are this emulator's.
///
/// The span is the CONFIGURED one, not this host's: "the amount of time that Lambda allows a
/// function to run before stopping it" is what the run is judged against and what a run that
/// ran past it is reported to have run past. How much of this host's own time the run may
/// really take is bounded beside it ([`HOST_WAIT_CEILING`]), and a wait the handler asks for
/// inside its span really passes up to [`GUEST_WAIT_HOST_SPAN`].
///
/// The memory bound is the SAME quantity the run's share of the process envelope is charged
/// in ([`run_memory_claim_mb`]), so what a run may allocate and what it holds of the
/// envelope cannot diverge: a function configured for the smallest `MemorySize` the model
/// allows is bounded at that, not at the per-run ceiling.
///
/// The function's EPHEMERAL STORAGE travels beside the memory and is not part of it: `/tmp`
/// is a disk the function's `EphemeralStorage.Size` sizes, so what a run writes there is
/// bounded by that size and charged against no memory bound
/// ([`ephemeral_storage_bytes`]).
///
/// `streamed` is the DELIVERY the caller asked for ([`Invocation::streamed`]), which is what
/// decides how much the run may answer: the response-streaming quota
/// ([`MAX_STREAMED_RESPONSE_BYTES`]) or the buffered one ([`MAX_RESPONSE_BYTES`]). It is a
/// parameter of the one function that builds a budget, so no caller can construct one whose
/// response bound belongs to the other delivery.
#[must_use]
pub fn run_budget(
    timeout_seconds: i32,
    memory_size: i32,
    ephemeral_storage_mb: i32,
    streamed: bool,
) -> Budget {
    let clamped = i64::from(timeout_seconds).clamp(MIN_TIMEOUT_SECONDS, MAX_TIMEOUT_SECONDS);
    let wall_clock = Duration::from_secs(u64::try_from(clamped).unwrap_or(1));
    Budget {
        wall_clock,
        steps: MAX_RUN_STEPS,
        memory_bytes: u64::try_from(run_heap_bytes(memory_size)).unwrap_or(MAX_RUN_MEMORY_BYTES),
        ephemeral_bytes: ephemeral_storage_bytes(ephemeral_storage_mb),
        log_bytes: MAX_RUN_LOG_BYTES,
        // What the run may ANSWER is the quota of the delivery the caller asked for: a
        // streamed answer is not bounded by what one buffered response carries.
        response_bytes: if streamed {
            MAX_STREAMED_RESPONSE_BYTES
        } else {
            MAX_RESPONSE_BYTES
        },
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

/// The variable a managed runtime reads the function's `ApplicationLogLevel` as, and
/// configures the language's own logging interface from.
const APPLICATION_LOG_LEVEL_VARIABLE: &str = "AWS_LAMBDA_LOG_LEVEL";

/// The variable a managed runtime reads the function's `LoggingConfig$LogFormat` as, and
/// renders its own records in.
///
/// It is set only for a function whose `LoggingConfig` selected the structured format, as
/// the level above is: both are variables the execution environment of a function on the
/// default format carries none of.
const LOG_FORMAT_VARIABLE: &str = "AWS_LAMBDA_LOG_FORMAT";

/// The variable a managed runtime exports the identifier of the runtime it IS as
/// ("AWS_EXECUTION_ENV – The runtime identifier, prefixed by `AWS_Lambda_`", the Lambda
/// Developer Guide's reserved runtime environment variables).
///
/// It is a reserved variable, so it is the service's statement of which runtime a run is of
/// rather than anything the function set: what a run reports of its own runtime is read out of
/// it — the release of the interpreter ([`python_release`]) and the library directory the
/// runtime's own modules are reported under ([`bootstrap`]).
pub(super) const EXECUTION_ENV_VARIABLE: &str = "AWS_EXECUTION_ENV";

/// What that variable's value begins with, as the sentence above states it.
pub(super) const EXECUTION_ENV_PREFIX: &str = "AWS_Lambda_";

/// The environment a run of this engine that is nobody's Lambda invocation is handed so the
/// release it reports is the one its own caller declared.
///
/// An EMBEDDER — another service whose own API declares which runtime a script of its runs in
/// — states that identifier the way a managed runtime's environment states it, because that
/// variable is where this engine reads WHICH runtime a run is of: the release of the
/// interpreter the run reports ([`python_release`]) and the library directory the runtime's
/// own modules are reported under ([`bootstrap`]) both come from it. Handing a run no
/// environment at all names no runtime, which is what leaves a run reporting the interpreter's
/// own build instead of the release its caller asked for.
///
/// Only the runtime is named: nothing of a Lambda function's own configuration belongs to such
/// a run, so an embedder's script sees this and whatever its own caller passed it.
#[must_use]
pub fn environment_naming_runtime(runtime: &str) -> BTreeMap<String, String> {
    BTreeMap::from([(
        EXECUTION_ENV_VARIABLE.to_string(),
        format!("{EXECUTION_ENV_PREFIX}{runtime}"),
    )])
}

/// The variable a managed runtime reads the log STREAM of its execution environment as. It is
/// the environment's own for as long as the environment stands, so the environments the pool
/// keeps re-state it for every invocation they serve ([`warm_pool`]).
pub(super) const LOG_STREAM_VARIABLE: &str = "AWS_LAMBDA_LOG_STREAM_NAME";

/// The three RESERVED environment variables a managed runtime exports the invocation's own
/// credentials in, in the order a credential's parts are named: "AWS_ACCESS_KEY_ID … the
/// access key for your Lambda function's execution role", "AWS_SECRET_ACCESS_KEY … the secret
/// access key" and "AWS_SESSION_TOKEN … the session token" (the Lambda Developer Guide's
/// reserved runtime environment variables, which is also where
/// [`crate::state::RESERVED_ENVIRONMENT_KEYS`] reads them from).
///
/// They are what an AWS SDK's DEFAULT credential chain inside a handler signs with, so a
/// handler that builds a client with no credentials of its own calls as the function's
/// execution role without reading anything itself.
const CREDENTIAL_VARIABLES: &[&str] = &[
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
];

/// What those three variables carry for one run: the TEMPORARY credentials of the session of
/// the function's execution role this invocation's calls are signed as
/// ([`execution_role_session`]).
///
/// Empty — so the run carries none of the three — for a run with no such session: this
/// emulator holds no record of the function's role, or there is no token service to mint a
/// session with, which is exactly where nothing can be shown about the role at all. Empty too
/// for a credential missing any of its three parts, because AWS vends no part of a temporary
/// credential: a handler reading one variable and not the others would sign with something no
/// service can resolve.
fn credential_variables(
    session: Option<&aws_api::CallerIdentity>,
) -> Vec<(&'static str, String)> {
    let Some(session) = session else {
        return Vec::new();
    };
    let parts = [
        session.access_key_id.clone(),
        session.secret_access_key.clone(),
        session.session_token.clone(),
    ];
    if parts.iter().any(String::is_empty) {
        return Vec::new();
    }
    CREDENTIAL_VARIABLES
        .iter()
        .copied()
        .zip(parts)
        .collect()
}

/// The ENVIRONMENT one run sees: the function's own variables, plus the ones the runtime
/// sets for a handler to read its own identity and its own credentials out of.
///
/// This is the whole environment: no host environment variable is in it, and the only
/// credential in it is the session of the function's own execution role
/// ([`credential_variables`]) — never the credential the framework hands this emulator's
/// handlers.
fn runtime_environment(
    invocation: &Invocation,
    started: SystemTime,
    metadata: &instance_metadata::MetadataSession,
) -> BTreeMap<String, String> {
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
            LOG_STREAM_VARIABLE,
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
        // variable names the address the interface is addressed at, which is where the
        // paths of it this emulator can answer are served
        // (`super::execution::runtime_api`).
        (
            "AWS_LAMBDA_RUNTIME_API",
            runtime_api::RUNTIME_API_ADDRESS.to_string(),
        ),
        // The host and port of the METADATA endpoint the execution environment publishes to
        // the handler running in it, beside the runtime API above: the address a handler that
        // discovers which Region and zone it is running in reads its own placement from
        // ([`instance_metadata`]). A handler is told the address rather than having to know
        // one, which is what the variable is for, and it is the address this emulator really
        // answers that interface on.
        (
            "AWS_LAMBDA_METADATA_API",
            instance_metadata::METADATA_API_ADDRESS.to_string(),
        ),
        ("LAMBDA_TASK_ROOT", "/var/task".to_string()),
        ("LAMBDA_RUNTIME_DIR", "/var/runtime".to_string()),
        (
            "_HANDLER",
            invocation.handler.clone().unwrap_or_default(),
        ),
        (
            EXECUTION_ENV_VARIABLE,
            format!(
                "{EXECUTION_ENV_PREFIX}{}",
                invocation.runtime.clone().unwrap_or_default()
            ),
        ),
    ];
    for (name, value) in reserved {
        environment.insert((*name).to_string(), value.clone());
    }
    // The TOKEN this execution environment was handed for that endpoint, which a read of it
    // has to present ([`instance_metadata`]): the environment is given the credential rather
    // than having to ask for one, exactly as it is given the address above. An environment
    // this process could mint no token for names none, so a handler reads no value that is not
    // a token.
    match metadata.environment_token() {
        Some(token) => {
            environment.insert(
                "AWS_LAMBDA_METADATA_TOKEN".to_string(),
                token.to_string(),
            );
        }
        None => {
            environment.remove("AWS_LAMBDA_METADATA_TOKEN");
        }
    }
    // The two variables the structured format brings with it, which a function on the
    // default plain-text format carries NEITHER of: the FORMAT the runtime renders this
    // invocation's records in, and the LEVEL its application records are filtered at.
    //
    // `AWS_LAMBDA_LOG_FORMAT` names the format only where the function's `LoggingConfig`
    // selected the structured one, because the runtime that writes a record reads the form
    // from it ([`bootstrap`], [`node::runtime`]) and a logging library that branches on the
    // mere presence of the variable must take the plain-text branch on a function AWS sets
    // it for no value of.
    if json_log_records(invocation) {
        environment.insert(
            LOG_FORMAT_VARIABLE.to_string(),
            super::LOG_FORMAT_JSON.to_string(),
        );
    } else {
        environment.remove(LOG_FORMAT_VARIABLE);
    }
    // The LEVEL the function's application records are filtered at, which the managed
    // runtimes read as `AWS_LAMBDA_LOG_LEVEL` and configure their own logging interface from:
    // `ApplicationLogLevel` is the filter on "the application logs for your function", so the
    // runtime that WRITES a record is told the level and emits only the records at that level
    // of detail and lower ([`bootstrap`], [`node::runtime`]). A function filtered at no level
    // at all names none, so the runtime reads no level and writes every record.
    match invocation
        .logs
        .as_ref()
        .and_then(|target| target.application_log_level.clone())
    {
        Some(level) => {
            environment.insert(APPLICATION_LOG_LEVEL_VARIABLE.to_string(), level);
        }
        None => {
            environment.remove(APPLICATION_LOG_LEVEL_VARIABLE);
        }
    }
    // The run's own CREDENTIALS, under the three names a managed runtime exports them as.
    // They are the runtime's to set, so whatever the environment held under those names is
    // given up first: a run no session was minted for carries none of them at all, rather
    // than a value this emulator did not mint for the function's role.
    for name in CREDENTIAL_VARIABLES {
        environment.remove(*name);
    }
    let session = invocation.aws.as_ref().and_then(AwsAccess::identity);
    for (name, value) in credential_variables(session) {
        environment.insert(name.to_string(), value);
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
        // "PATH – /usr/local/bin:/usr/bin/:/bin:/opt/bin": the system directories a managed
        // runtime's sandbox looks for a program in, with the layer root's own `bin` at the
        // end — which is what makes an executable a layer ships reachable by its bare name.
        (
            "PATH",
            "/usr/local/bin:/usr/bin/:/bin:/opt/bin".to_string(),
        ),
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

/// The log stream one EXECUTION ENVIRONMENT writes to: the date it was opened, the version
/// it runs, and the id of the environment itself — the form a CloudWatch Logs stream of a
/// Lambda function has.
///
/// One stream per environment, which is what makes the invocations a standing environment
/// serves share a stream and a new environment open one of its own. An environment built for
/// one invocation alone is identified by that invocation.
///
/// The date is the `YYYY/MM/DD` of the instant the run started, taken from the one
/// timestamp rendering this crate already has ([`super::iso8601`], whose calendar
/// arithmetic is `aws_smithy_types`'s) rather than computed here.
pub(super) fn log_stream_name(version: &str, environment_id: &str, started: SystemTime) -> String {
    let rendered = super::iso8601(started);
    let date = rendered.split('T').next().unwrap_or_default().replace('-', "/");
    format!(
        "{date}/[{version}]{}",
        environment_id.replace('-', "")
    )
}

/// The stream the execution environment that served this run writes to: the environment's own
/// where the pool that holds it decided one ([`warm_pool`]), and otherwise the one this
/// invocation composed — an environment built for one invocation alone is identified by that
/// invocation.
fn log_stream_of(invocation: &Invocation, finished: &Finished, started: SystemTime) -> String {
    finished.log_stream.clone().unwrap_or_else(|| {
        log_stream_name(
            &invocation.executed_version,
            &invocation.request_id,
            started,
        )
    })
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

    /// The stream the environment serving those invocations writes to, in the form
    /// [`log_stream_name`] composes.
    const PROBE_LOG_STREAM: &str = "2026/10/26/[7]0123456789abcdef0123456789abcdef";

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
            ephemeral_storage_mb: crate::state::DEFAULT_EPHEMERAL_STORAGE_MB,
            timeout: 3,
            architecture: "x86_64".to_string(),
            zip_bytes: None,
            layer_zips: Vec::new(),
            payload: Vec::new(),
            client_context: None,
            tenant_id: None,
            cognito_identity: None,
            request_id: "11111111-2222-3333-4444-555555555555".to_string(),
            trace: None,
            initialization_type: crate::state::execution_slot::INITIALIZATION_TYPE_ON_DEMAND,
            aws: None,
            logs: None,
            file_systems: Ok(Vec::new()),
            metrics: None,
            streamed: false,
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

    /// What the closing report states the run USED: the reading rounded UP to the megabyte
    /// the field is stated in, floored at that megabyte for a run that executed, clamped to
    /// the memory the function is configured for, and stated as nothing at all only for a run
    /// nothing measured.
    ///
    /// One row per class the rule distinguishes — no reading, a reading under the unit, one on
    /// it, ones over it, and ones at and past the configured size — over more than one
    /// configured size, because the ceiling the figure is clamped to is the function's own.
    #[test]
    fn the_report_states_the_memory_a_run_was_measured_using() {
        // (what it is about, the reading, the configured MemorySize, the megabytes stated)
        let cases: &[(&str, Option<u64>, i32, i64)] = &[
            ("a run nothing measured, because nothing ran", None, 128, 0),
            ("a run that faulted no page of its own", Some(0), 128, 1),
            ("a reading of a single byte", Some(1), 128, 1),
            ("a reading just under the unit", Some(BYTES_PER_MB - 1), 128, 1),
            ("a reading of exactly the unit", Some(BYTES_PER_MB), 128, 1),
            ("a reading just over the unit", Some(BYTES_PER_MB + 1), 128, 2),
            ("a reading of several megabytes", Some(32 * BYTES_PER_MB), 512, 32),
            (
                "a reading a byte over several megabytes",
                Some(32 * BYTES_PER_MB + 1),
                512,
                33,
            ),
            (
                "a reading of the whole memory the function is configured for",
                Some(128 * BYTES_PER_MB),
                128,
                128,
            ),
            (
                "a reading past that memory, which is all the run was allowed to hold",
                Some(129 * BYTES_PER_MB),
                128,
                128,
            ),
            (
                "the same reading under a larger configuration",
                Some(129 * BYTES_PER_MB),
                256,
                129,
            ),
            ("a reading past every bound", Some(u64::MAX), 256, 256),
        ];
        for (case, measured, memory_size, expected) in cases {
            let actual = max_memory_used_mb(*measured, *memory_size);
            assert_eq!(
                actual, *expected,
                "case {case:?} with {memory_size} MB configured: expected {expected} MB stated, \
                 got {actual}"
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
            let budget = run_budget(
                *timeout,
                MIN_MEMORY_SIZE_MB as i32,
                crate::state::DEFAULT_EPHEMERAL_STORAGE_MB,
                false,
            );
            let finished = Finished {
                completion: Completion::Stopped(Stop::Deadline),
                log: String::new(),
                log_records: Vec::new(),
                init_records: Vec::new(),
                initialized: true,
                init_duration: None,
                log_stream: None,
                elapsed: budget.wall_clock,
                unwaited: Duration::ZERO,
                max_memory_bytes: None,
            };
            let invoked = answered(
                finished,
                request_id,
                &probe_invocation(),
                budget,
                *engine,
                SystemTime::UNIX_EPOCH,
                PROBE_LOG_STREAM,
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
            let expected =
                format!("RequestId: {request_id} Error: Task timed out after {rendered} seconds");
            assert_eq!(
                parsed
                    .get("errorMessage")
                    .and_then(::serde_json::Value::as_str),
                Some(expected.as_str()),
                "case {case}: expected the message {expected:?}, got {parsed}"
            );
        }
    }

    /// A run the SANDBOX ended carries exactly two members: the class of the failure and
    /// the message about it, in that order — the platform writes its own document and
    /// names the class first. No runtime serialized the failure, so the document names
    /// neither the request id nor the stack a runtime's own error payload reports beside
    /// them — one row per bound a run can be stopped by, per engine, so the member set is
    /// neither one bound's nor one runtime's.
    #[test]
    fn a_sandbox_ended_runs_document_carries_only_the_message_and_the_class() {
        let request_id = "11111111-2222-3333-4444-555555555555";
        // The bound the run was stopped by, and the class the platform reports it under.
        let stops: &[(Stop, &str)] = &[
            (Stop::Deadline, SANDBOX_TIMEDOUT),
            (Stop::Memory, RUNTIME_OUT_OF_MEMORY),
            (Stop::Response, RESPONSE_SIZE_TOO_LARGE),
            (Stop::Steps, RUNTIME_EXIT_ERROR),
            (Stop::HostCeiling, RUNTIME_EXIT_ERROR),
        ];
        for (stop, class) in stops {
            for engine in [Engine::Python, Engine::Node] {
                let budget = run_budget(
                    3,
                    MIN_MEMORY_SIZE_MB as i32,
                    crate::state::DEFAULT_EPHEMERAL_STORAGE_MB,
                    false,
                );
                let finished = Finished {
                    completion: Completion::Stopped(*stop),
                    log: String::new(),
                    log_records: Vec::new(),
                    init_records: Vec::new(),
                    initialized: true,
                    init_duration: None,
                    log_stream: None,
                    elapsed: budget.wall_clock,
                    unwaited: Duration::ZERO,
                    max_memory_bytes: None,
                };
                let invoked = answered(
                    finished,
                    request_id,
                    &probe_invocation(),
                    budget,
                    engine,
                    SystemTime::UNIX_EPOCH,
                    PROBE_LOG_STREAM,
                );
                let parsed: ::serde_json::Value =
                    ::serde_json::from_slice(&invoked.payload).expect("the document is JSON");
                let case = format!("{engine:?} stopped by {stop:?}");
                let members: Vec<&str> = parsed
                    .as_object()
                    .map(|document| document.keys().map(String::as_str).collect())
                    .unwrap_or_default();
                assert_eq!(
                    members,
                    vec!["errorType", "errorMessage"],
                    "case {case}: expected the platform's own two members, in the order it \
                     writes them, got {parsed}"
                );
                assert_eq!(
                    parsed.get("errorType").and_then(::serde_json::Value::as_str),
                    Some(*class),
                    "case {case}: expected the class {class}, got {parsed}"
                );
                assert!(
                    parsed
                        .get("errorMessage")
                        .and_then(::serde_json::Value::as_str)
                        .is_some_and(|message| !message.is_empty()),
                    "case {case}: expected the message to state what happened, got {parsed}"
                );
            }
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
                        // The row is about the error CLASS a runtime words itself, which is
                        // what names the phase where the engine reports none.
                        initialization: false,
                    },
                    log: String::new(),
                    log_records: Vec::new(),
                    init_records: Vec::new(),
                    initialized: true,
                    init_duration: None,
                    log_stream: None,
                    elapsed: Duration::ZERO,
                    unwaited: Duration::ZERO,
                    max_memory_bytes: None,
                };
                let invoked = answered(
                    finished,
                    request_id,
                    &probe_invocation(),
                    run_budget(
                        3,
                        MIN_MEMORY_SIZE_MB as i32,
                        crate::state::DEFAULT_EPHEMERAL_STORAGE_MB,
                        false,
                    ),
                    engine,
                    SystemTime::UNIX_EPOCH,
                    PROBE_LOG_STREAM,
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

    /// A failure the ENGINE reported as the INITIALIZATION's names no invocation whatever
    /// class it carries: an initialization fails under whatever class the deployment
    /// package's own top-level code raised, so the phase decides this and the class does
    /// not. One row per class a package can fail its own import under, each read both ways
    /// round — reported as the initialization's and as a run's.
    #[test]
    fn a_failure_the_engine_reported_as_the_initializations_names_no_invocation() {
        let request_id = "11111111-2222-3333-4444-555555555555";
        for error_type in ["LookupError", "SyntaxError", "ValueError", "RuntimeError"] {
            for initialization in [true, false] {
                let finished = Finished {
                    completion: Completion::Raised {
                        error_type: error_type.to_string(),
                        error_message: "the package's own code raised".to_string(),
                        stack_trace: Vec::new(),
                        initialization,
                    },
                    log: String::new(),
                    log_records: Vec::new(),
                    init_records: Vec::new(),
                    initialized: true,
                    init_duration: None,
                    log_stream: None,
                    elapsed: Duration::ZERO,
                    unwaited: Duration::ZERO,
                    max_memory_bytes: None,
                };
                let invoked = answered(
                    finished,
                    request_id,
                    &probe_invocation(),
                    run_budget(
                        3,
                        MIN_MEMORY_SIZE_MB as i32,
                        crate::state::DEFAULT_EPHEMERAL_STORAGE_MB,
                        false,
                    ),
                    Engine::Python,
                    SystemTime::UNIX_EPOCH,
                    PROBE_LOG_STREAM,
                );
                let parsed: ::serde_json::Value = ::serde_json::from_slice(&invoked.payload)
                    .expect("the error document is JSON");
                let actual = parsed
                    .get("requestId")
                    .and_then(::serde_json::Value::as_str);
                let expected = if initialization { "" } else { request_id };
                assert_eq!(
                    actual,
                    Some(expected),
                    "case {error_type} reported by the initialization: {initialization}: \
                     expected the request id {expected:?}, got {parsed}"
                );
            }
        }
    }

    /// An error document larger than an invocation may answer with is refused for its SIZE,
    /// like any other response of that size: the caller reads the response-size class and
    /// the quota instead of the oversize document, and a document of exactly the quota is
    /// answered as the runtime wrote it.
    ///
    /// The rows are the sizes around that quota, measured on the document rather than on the
    /// message: the overhead the document's own members add is taken from the document
    /// builder itself, so the boundary is the real one.
    #[test]
    fn an_error_document_past_the_response_quota_is_answered_as_a_response_past_it() {
        let request_id = "11111111-2222-3333-4444-555555555555";
        let budget = run_budget(
            3,
            MIN_MEMORY_SIZE_MB as i32,
            crate::state::DEFAULT_EPHEMERAL_STORAGE_MB,
            false,
        );
        let quota = budget.response_bytes;
        // What the document's own members cost, beside the message: the builder's own answer
        // for an empty message.
        let overhead = error_document("", Some("Oversize"), request_id, &[], Engine::Python).len();
        for (case, message_bytes, oversize) in [
            ("a document of exactly the quota", quota - overhead, false),
            ("a document one byte past the quota", quota - overhead + 1, true),
        ] {
            // Characters no JSON writer escapes, so one character is one byte of the
            // document.
            let message = "x".repeat(message_bytes);
            let finished = Finished {
                completion: Completion::Raised {
                    error_type: "Oversize".to_string(),
                    error_message: message.clone(),
                    stack_trace: Vec::new(),
                    initialization: false,
                },
                log: String::new(),
                log_records: Vec::new(),
                init_records: Vec::new(),
                initialized: true,
                init_duration: None,
                log_stream: None,
                elapsed: Duration::ZERO,
                unwaited: Duration::ZERO,
                max_memory_bytes: None,
            };
            let invoked = answered(
                finished,
                request_id,
                &probe_invocation(),
                budget,
                Engine::Python,
                SystemTime::UNIX_EPOCH,
                PROBE_LOG_STREAM,
            );
            let parsed: ::serde_json::Value =
                ::serde_json::from_slice(&invoked.payload).expect("the document is JSON");
            let class = parsed
                .get("errorType")
                .and_then(::serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let reported = parsed
                .get("errorMessage")
                .and_then(::serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if oversize {
                assert_eq!(
                    (class.as_str(), reported.as_str()),
                    (
                        RESPONSE_SIZE_TOO_LARGE,
                        format!(
                            "Response payload size exceeded maximum allowed payload size \
                             ({quota} bytes)."
                        )
                        .as_str()
                    ),
                    "case {case}: expected the response-size refusal, got {class} and {} \
                     bytes of message",
                    reported.len()
                );
            } else {
                assert_eq!(
                    (class.as_str(), reported.len(), invoked.payload.len()),
                    ("Oversize", message_bytes, quota),
                    "case {case}: expected the runtime's own document, got {class} and {} \
                     bytes of document",
                    invoked.payload.len()
                );
            }
        }
    }

    /// Every frame of the stack a Python run's error document carries is ONE frame as the
    /// runtime renders it: its location line, and — for a module a FILE of the execution
    /// environment holds — the source line at that location, with no fine-grained
    /// error-location anchors under it. One row per shape of stack, including one that reaches
    /// a module of the standard library this runtime serves FROZEN, whose frame names that
    /// frozen module and stands on no source text at all.
    #[test]
    fn a_reported_frame_is_a_location_and_its_source_line() {
        // The module of the standard library the third row's failure is taken inside, as the
        // frame of a frozen module names it.
        let frozen = "<frozen os>";
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
                Some(frozen),
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
                let names_a_file = file.starts_with('/') && file.ends_with(".py");
                let names_a_frozen_module = file.starts_with('<') && file.ends_with('>');
                assert!(
                    names_a_file || names_a_frozen_module,
                    "case {case}: frame {index} must name a source file by path or the frozen \
                     module it stands in, got {file:?}"
                );
                assert!(
                    names_a_file || lines.len() == 1,
                    "case {case}: frame {index} stands in {file:?}, which no file holds, so it \
                     carries no source text, got {frame:?}"
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
                let expected = format!("  File \"{within}\", line ");
                assert!(
                    frames.iter().any(|frame| frame.starts_with(&expected)),
                    "case {case}: expected a frame standing in {within:?}, got \
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
        let environment = runtime_environment(
            &probe_invocation(),
            SystemTime::UNIX_EPOCH,
            &instance_metadata::MetadataSession::new(),
        );
        let finished = ::std::thread::Builder::new()
            .stack_size(RUN_STACK_BYTES)
            .spawn(move || {
                python::run(&python::Run {
                    modules: &modules,
                    handler: "lambda_function.handler",
                    environment: &environment,
                    architecture: crate::state::DEFAULT_ARCHITECTURE,
                    event: None,
                    request_id: "11111111-2222-3333-4444-555555555555",
                    invoked_function_arn: "arn:aws:lambda:us-west-2:000000000000:function:probe",
                    client_context: None,
                    tenant_id: None,
                    cognito_identity: None,
                    budget: run_budget(
                        5,
                        MIN_MEMORY_SIZE_MB as i32,
                        crate::state::DEFAULT_EPHEMERAL_STORAGE_MB,
                        false,
                    ),
                    aws: None,
                    durable: None,
                    file_systems: &[],
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
    /// run — one row per bound of that range and one past each. The span a run is judged
    /// against is the CONFIGURED one, however much of this host's time it may really take
    /// ([`HOST_WAIT_CEILING`]), so a `Timeout` past that ceiling still bounds the run at
    /// what its function asked for.
    #[test]
    fn a_stored_timeout_is_clamped_before_it_bounds_a_run() {
        let cases: &[(i32, u64)] = &[
            (0, 1),
            (-7, 1),
            (1, 1),
            (3, 3),
            (29, 29),
            (HOST_WAIT_CEILING.as_secs() as i32 + 1, HOST_WAIT_CEILING.as_secs() + 1),
            (900, 900),
            (100_000, 900),
        ];
        for (configured, expected) in cases {
            let actual = run_budget(
                *configured,
                MIN_MEMORY_SIZE_MB as i32,
                crate::state::DEFAULT_EPHEMERAL_STORAGE_MB,
                false,
            )
                .wall_clock
                .as_secs();
            assert_eq!(
                actual, *expected,
                "case {configured} s configured: expected a {expected} s run budget, got {actual}"
            );
        }
    }

    /// RULE: what a run may ANSWER is the quota of the DELIVERY the caller asked for —
    /// `InvokeMode` states "BUFFERED … The maximum payload size is 6 MB" against
    /// "RESPONSE_STREAM … The maximum response payload size is 200 MB" — and nothing else
    /// the function is configured for decides it.
    #[test]
    fn the_response_quota_is_the_one_of_the_delivery_asked_for() {
        // Rows differ along the delivery and along the other bounds the function carries,
        // which decide no part of this one.
        let rows: &[(&str, bool, i32, i32, usize)] = &[
            (
                "a buffered delivery of the smallest function",
                false,
                1,
                MIN_MEMORY_SIZE_MB as i32,
                MAX_RESPONSE_BYTES,
            ),
            (
                "a buffered delivery of a long-running, large function",
                false,
                900,
                2048,
                MAX_RESPONSE_BYTES,
            ),
            (
                "a streamed delivery of the smallest function",
                true,
                1,
                MIN_MEMORY_SIZE_MB as i32,
                MAX_STREAMED_RESPONSE_BYTES,
            ),
            (
                "a streamed delivery of a long-running, large function",
                true,
                900,
                2048,
                MAX_STREAMED_RESPONSE_BYTES,
            ),
        ];
        for (case, streamed, timeout, memory_size, expected) in rows {
            let budget = run_budget(
                *timeout,
                *memory_size,
                crate::state::DEFAULT_EPHEMERAL_STORAGE_MB,
                *streamed,
            );
            assert_eq!(
                budget.response_bytes, *expected,
                "case {case:?}: expected a response quota of {expected} bytes, got {}",
                budget.response_bytes
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
                unwaited: Duration::ZERO,
                chunks: Vec::new(),
                stream_failure: None,
                stopped: None,
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
        let environment = runtime_environment(
            &invocation,
            SystemTime::UNIX_EPOCH,
            &instance_metadata::MetadataSession::new(),
        );
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
            // The documented value of the sandbox's own search path, whose last entry is the
            // layer root's `bin` directory.
            ("PATH", "/usr/local/bin:/usr/bin/:/bin:/opt/bin"),
        ];
        for (name, expected) in cases {
            let actual = environment.get(*name).map(String::as_str);
            assert_eq!(
                actual,
                Some(*expected),
                "case {name}: expected {expected:?}, got {actual:?}"
            );
        }
        for absent in [
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "HOME",
        ] {
            assert!(
                !environment.contains_key(absent),
                "case {absent}: no host variable reaches a run, and no credential variable \
                 reaches one no execution-role session was minted for, got {:?}",
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

    /// RULE: a run is given the three reserved credential variables at the values of the
    /// session of its function's execution role, and a run with no such session — or one
    /// whose credential is missing a part, which is no temporary credential — is given none
    /// of them, because AWS vends no part of one.
    ///
    /// The rows differ along WHICH part the credential is missing, which is what the rule
    /// generalises over.
    #[test]
    fn a_run_is_given_the_whole_execution_role_session_credential_or_none_of_it() {
        let whole = aws_api::CallerIdentity {
            access_key_id: "ASIAEXECUTIONROLESESSION".to_string(),
            secret_access_key: "a-secret-no-response-carries".to_string(),
            session_token: "a-session-token-of-the-execution-role".to_string(),
        };
        let without = |part: usize| {
            let mut held = whole.clone();
            match part {
                0 => held.access_key_id.clear(),
                1 => held.secret_access_key.clear(),
                _ => held.session_token.clear(),
            }
            held
        };
        /// One row: the credential the run signs with, and what the run is then given under
        /// the three reserved names.
        struct Row<'r> {
            case: &'static str,
            session: Option<aws_api::CallerIdentity>,
            given: Vec<(&'static str, &'r str)>,
        }
        let rows: Vec<Row<'_>> = vec![
            Row {
                case: "a run no session was minted for",
                session: None,
                given: Vec::new(),
            },
            Row {
                case: "a run signing as a session of its execution role",
                session: Some(whole.clone()),
                given: vec![
                    ("AWS_ACCESS_KEY_ID", whole.access_key_id.as_str()),
                    ("AWS_SECRET_ACCESS_KEY", whole.secret_access_key.as_str()),
                    ("AWS_SESSION_TOKEN", whole.session_token.as_str()),
                ],
            },
            Row {
                case: "a credential with no access key id",
                session: Some(without(0)),
                given: Vec::new(),
            },
            Row {
                case: "a credential with no secret",
                session: Some(without(1)),
                given: Vec::new(),
            },
            Row {
                case: "a credential with no session token",
                session: Some(without(2)),
                given: Vec::new(),
            },
        ];
        for Row {
            case,
            session,
            given,
        } in rows
        {
            let exported = credential_variables(session.as_ref());
            let actual: Vec<(&str, &str)> = exported
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect();
            assert_eq!(
                actual, given,
                "case {case:?}: expected the run to be given {given:?}, got {actual:?}"
            );
        }
    }

    /// One row of the table below: the invocation's trace context, what the function's own
    /// environment sets for the two X-Ray variables, and what the run must then be published.
    struct DaemonRow {
        case: &'static str,
        /// The trace context the invocation carries, or `None` for one carrying none.
        trace: Option<trace_context::InvocationTrace>,
        /// The function's own value for the daemon address, where it sets one.
        own_address: Option<&'static str>,
        /// The function's own value for the context-missing variable, where it sets one.
        own_missing: Option<&'static str>,
    }

    /// EVERY run is told where to send its X-Ray segment documents, and the two variables that
    /// say so are ones the runtime sets WITHOUT reserving — so a function that sets one of them
    /// runs with its own value, and one that does not is given the platform's.
    ///
    /// The rows span what that generalises over: whether the invocation carries a trace context
    /// at all (an invocation X-Ray records, one it does not, and one with no context), and
    /// whether the function's own environment names either variable. The published address is
    /// read back as `IP_ADDRESS:PORT`, the form the runtime environment reports it in, rather
    /// than against a literal.
    #[test]
    fn the_daemon_address_is_published_to_every_run_and_a_functions_own_value_wins() {
        let rows = vec![
            DaemonRow {
                case: "an invocation with no trace context at all",
                trace: None,
                own_address: None,
                own_missing: None,
            },
            DaemonRow {
                case: "an invocation carrying a context X-Ray records nothing for",
                trace: Some(trace_context::InvocationTrace {
                    header: "Root=1-00000000-000000000000000000000001;Sampled=0".to_string(),
                    recorded: None,
                }),
                own_address: None,
                own_missing: None,
            },
            DaemonRow {
                case: "an invocation X-Ray records",
                trace: Some(trace_context::InvocationTrace {
                    header: "Root=1-00000000-000000000000000000000002;Parent=\
                             0000000000000003;Sampled=1"
                        .to_string(),
                    recorded: Some(trace_context::Recorded {
                        trace_id: "1-00000000-000000000000000000000002".to_string(),
                        service_span: "0000000000000004".to_string(),
                        function_span: "0000000000000003".to_string(),
                        caller_span: None,
                        linked_traces: Vec::new(),
                    }),
                }),
                own_address: None,
                own_missing: None,
            },
            DaemonRow {
                case: "a function whose own environment names both variables",
                trace: None,
                own_address: Some("127.0.0.1:2100"),
                own_missing: Some("RUNTIME_ERROR"),
            },
        ];
        for row in &rows {
            let case = row.case;
            let mut invocation = probe_invocation();
            invocation.trace = row.trace.clone();
            if let Some(address) = row.own_address {
                invocation.variables.insert(
                    xray_daemon::DAEMON_ADDRESS_VARIABLE.to_string(),
                    address.to_string(),
                );
            }
            if let Some(missing) = row.own_missing {
                invocation.variables.insert(
                    xray_daemon::CONTEXT_MISSING_VARIABLE.to_string(),
                    missing.to_string(),
                );
            }
            let environment = runtime_environment(
                &invocation,
                SystemTime::UNIX_EPOCH,
                &instance_metadata::MetadataSession::new(),
            );
            let published = environment
                .get(xray_daemon::DAEMON_ADDRESS_VARIABLE)
                .map(String::as_str)
                .unwrap_or_default();
            // Whatever it carries, it is the `IP_ADDRESS:PORT` the runtime environment reports
            // the daemon at, which is the form a handler parses.
            assert!(
                published.parse::<::std::net::SocketAddrV4>().is_ok(),
                "case {case:?}: expected {} to be published as IP_ADDRESS:PORT, got \
                 {published:?}",
                xray_daemon::DAEMON_ADDRESS_VARIABLE
            );
            let context_missing = environment
                .get(xray_daemon::CONTEXT_MISSING_VARIABLE)
                .map(String::as_str)
                .unwrap_or_default();
            // The function's own value for either variable is the one its run carries; a
            // function that sets neither is given the platform's.
            let expected_address = row.own_address.unwrap_or(xray_daemon::DAEMON_ADDRESS);
            let expected_missing = row
                .own_missing
                .unwrap_or(xray_daemon::CONTEXT_MISSING_LOG_ERROR);
            assert_eq!(
                published, expected_address,
                "case {case:?}: expected the daemon address {expected_address:?}, got \
                 {published:?}"
            );
            assert_eq!(
                context_missing, expected_missing,
                "case {case:?}: expected {} to be {expected_missing:?}, got {context_missing:?}",
                xray_daemon::CONTEXT_MISSING_VARIABLE
            );
        }
    }
}
