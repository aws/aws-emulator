// SPDX-License-Identifier: CC0-1.0

//! The JAVASCRIPT engine one invocation runs in, and everything that bounds and contains
//! it.
//!
//! A `nodejs*` function's answer is whatever its handler returned, so the handler is RUN:
//! the deployment package's own modules are compiled and executed by the engine this module
//! builds, in this process, and the value the handler produced is serialized by that same
//! engine. Nothing about a run is synthesised — a payload, a log line, an error type or a
//! duration here is the one the guest produced.
//!
//! # Containment
//!
//! The guest reaches exactly what this module installs, and nothing else:
//!
//! * the engine is QuickJS through `rquickjs` with the `dyn-load` feature left OUT of the
//!   build (see this crate's `Cargo.toml`), so there is no native-library loader in the
//!   binary, and the engine itself exposes no filesystem, process, socket, environment or
//!   host-clock-setting API of its own: an unaugmented context can reach the host through
//!   nothing at all;
//! * the host functions [`install_host`] installs are the whole host surface of a run —
//!   the invocation log, a package entry's source and the package's entry list, a core
//!   module's source, and the three pure computations `crypto` is served by (a digest, a
//!   keyed digest and random bytes) — and the runtime source takes every one of them off
//!   the global object as soon as it holds them
//!   ([`runtime`]). What stays reachable from guest code is the runtime's own frozen
//!   protocol object, through which a handler can at most decide its own answer — which
//!   returning a value already does — because every bound is held here, in host code;
//! * module resolution is this emulator's ([`RuntimeResolver`], [`RuntimeLoader`]), which
//!   answers with a file of THIS package or one of the core modules
//!   ([`core_modules::CORE_MODULES`]) and with nothing else, so neither `require` nor
//!   `import` resolves a path of the host;
//! * the environment the guest reads is the invocation's own variables, held in memory. No
//!   host environment variable, host credential or other function's state is reachable
//!   from guest code.
//!
//! # Bounds
//!
//! Every bound lives in host code the guest cannot switch off:
//!
//! * the DEADLINE and the STEP ceiling are checked by [`Bounds::step`], called from the
//!   engine's own interrupt handler, which is installed before any guest code is compiled
//!   and which no guest call can read, replace or clear. The stop is LATCHED: once a bound
//!   is breached the run's answer is that stop, whatever the guest goes on to return or
//!   catch. Every deadline of a run is read against the CLOCK the host hands the run
//!   ([`run_against`]), which is this host's own for every invocation this emulator serves —
//!   so a run can be judged against a clock whose span the host's own speed cannot spend,
//!   which is what lets a run asked about one of the bounds below be answered by THAT bound
//!   rather than by its deadline on a host slow enough to spend it.
//! * MEMORY is the engine's own allocation ceiling, set from the function's `MemorySize`
//!   before anything is evaluated: an allocation past it fails inside the engine rather
//!   than being noticed afterwards, so a run cannot grow this process past the ceiling at
//!   all.
//! * The LOG and the RESPONSE are each capped, so a small request cannot make an unbounded
//!   answer.
//! * BLOCKING is not the guest's to do: the engine has no sleep, and the timer queue a
//!   handler schedules work on is driven by the HOST ([`drive`]), which never waits past
//!   what the run has left.
//! * A long stretch of engine-internal work emits no interrupt check, so the deadline
//!   cannot always land inside one. That is what the caller's own host watch and
//!   abandoned-run accounting ([`super::run_invocation`]) exist for: the CALLER is always
//!   answered inside its ceiling, and a run still going after its budget was spent is
//!   given up on.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use rquickjs::function::Func;
use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::{CatchResultExt, Context, Ctx, Function, Module, Object, Runtime, Value};

use super::aws_api::{Answer, AwsAccess, Refusal, MAX_CALLS_PER_RUN};
use super::outcome::{captured, Capture};
use super::package::PackageModule;

// What one run may spend and what it came to are the same for every engine, so they are
// the shared ones ([`super::outcome`]) rather than this engine's own.
pub use super::outcome::{Budget, Completion, Finished, Stop, StreamFailure};

// So is the clock a run's time-based bounds are read against ([`super::run_clock`]): WHICH
// clock a run is judged by is a property of the run ([`run_against`]) rather than a call to
// the host buried inside each check.
pub use super::run_clock::Clock;

/// The core modules this emulator supplies to a run.
mod core_modules;

/// The digests and random bytes the `crypto` core module is served by.
mod digest;

/// `Intl` and the local time zone a run reads its clock in.
mod intl;

/// The runtime source a handler runs under.
mod runtime;

use core_modules::core_module_source;

/// The C stack one run's engine may use.
///
/// Well under the stack the run's thread is given ([`super::RUN_STACK_BYTES`]), so deep
/// guest recursion ends that run with the engine's own stack-overflow error rather than
/// touching the thread's guard page.
const ENGINE_STACK_BYTES: usize = 2 * 1024 * 1024;

/// How many times the engine may ask whether this run should stop before it is stopped
/// anyway.
///
/// The engine asks this question periodically as it executes guest code, so this is a
/// ceiling on the WORK one run may do at all, standing BEHIND the deadline: it is set high
/// enough that a handler doing real work reaches its configured `Timeout` first, and is
/// there for a run the deadline cannot end on its own.
const MAX_ENGINE_CHECKS: u64 = 1_000_000_000;

/// How close to the engine's allocation limit the host reads as having reached it
/// ([`memory_exhausted`]): the engine refuses the allocation that would cross the limit, so
/// a run sitting this near it has no room left to run in.
const MEMORY_CEILING_SLACK_BYTES: i64 = 1024 * 1024;

/// The longest the host waits in one go for a timer to come due, so that the deadline is
/// re-read while a run is waiting rather than only when it next wakes.
const TIMER_WAIT_SLICE: Duration = Duration::from_millis(250);

/// What an unsettled invocation is waiting for when only an empty event loop can answer it
/// — the non-async handler's case ([`runtime`]).
const WAITING_FOR_LOOP: &str = "loop";

/// The engine's own error type for an allocation it could not make, which is how a run
/// that reached the memory ceiling surfaces: the ceiling is enforced inside the engine, so
/// the guest sees an exception rather than the host seeing a sample.
const ENGINE_OUT_OF_MEMORY: &str = "out of memory";

/// The error type the engine itself raises under, as against one of the language's own.
const ENGINE_INTERNAL_ERROR: &str = "InternalError";

/// One invocation to run: the package, the handler, and everything the runtime reports to
/// it.
#[derive(Debug)]
pub struct Run<'a> {
    /// The deployment package's modules.
    pub modules: &'a [PackageModule],
    /// The `Handler` configured for the function (`module.export`).
    pub handler: &'a str,
    /// The `Runtime` configured for the function, which names the Node release line the
    /// run reports as its own.
    pub runtime: &'a str,
    /// The `Architectures` entry the function is configured for, which the run reports as
    /// its own `process.arch`.
    pub architecture: &'a str,
    /// The environment the invocation runs with: the function's own variables plus the
    /// ones the runtime sets.
    pub environment: &'a BTreeMap<String, String>,
    /// The event payload, as the caller sent it.
    pub event: Option<&'a str>,
    /// The invocation's request id.
    pub request_id: &'a str,
    /// The ARN the invocation addressed, as `context.invokedFunctionArn`.
    pub invoked_function_arn: &'a str,
    /// The `ClientContext` the caller passed, already decoded to its JSON text.
    pub client_context: Option<&'a str>,
    /// The `TenantId` the caller passed.
    pub tenant_id: Option<&'a str>,
    /// What this run may spend.
    pub budget: Budget,
    /// The engine's allocation ceiling: the function's `MemorySize`, in bytes.
    pub heap_bytes: usize,
    /// How the handler reaches the rest of this emulator through the AWS SDK this runtime
    /// supplies it ([`super::aws_api`]), or `None` where there is no seam to dispatch a
    /// call on.
    pub aws: Option<&'a AwsAccess>,
}

/// Run `request`'s handler to completion.
///
/// Called on a thread of its own with no state guard of this service held, because the
/// engine is synchronous and CPU-bound.
#[must_use]
pub fn run(request: &Run<'_>) -> Finished {
    run_against(request, &Clock::host())
}

/// [`run`], against the CLOCK this run's time-based bounds are read from.
///
/// Which bound stops a run is the host's own reading, so which clock the host reads is a
/// property of the run: every invocation this emulator serves is bounded against this host's
/// own clock ([`Clock::host`], which is what [`run`] passes), and a run driven to establish
/// that ONE named bound is what stops it is bounded against a clock whose span this host's
/// speed cannot spend ([`super::run_clock`]) — so a run asked about its memory ceiling, its
/// step budget or its response cap is not answered by its deadline instead because the host
/// was slow.
#[must_use]
pub fn run_against(request: &Run<'_>, clock: &Clock) -> Finished {
    // The run's own DURATION is this host's own elapsed time whatever clock bounds the run:
    // what it reports is how long the caller waited, which is not a bound.
    let started = Instant::now();
    let capture = Arc::new(Mutex::new(Capture::new(request.budget.log_bytes)));
    let bounds = Arc::new(Bounds::new(&request.budget, clock.clone()));
    let completion = evaluate(request, &capture, &bounds);
    let elapsed = started.elapsed();
    // The stop LATCH wins over whatever the guest answered: a run whose budget was spent
    // is a stopped run even if the guest caught the exception the engine raised in it.
    let completion = match bounds.stopped() {
        Some(stop) => Completion::Stopped(stop),
        None => completion,
    };
    let log = captured(&capture).text.clone();
    Finished {
        completion,
        log,
        elapsed,
    }
}

/// Build the engine, install the host surface and the runtime, and drive the invocation.
fn evaluate(request: &Run<'_>, capture: &Arc<Mutex<Capture>>, bounds: &Arc<Bounds>) -> Completion {
    let input = match runtime_input(request) {
        Ok(input) => input,
        Err(error) => return Completion::Internal(error),
    };
    let engine = match Runtime::new() {
        Ok(engine) => engine,
        Err(error) => return Completion::Internal(format!("the engine did not start: {error}")),
    };
    // Every bound is on the engine BEFORE a line of the package is compiled.
    engine.set_memory_limit(request.heap_bytes);
    engine.set_max_stack_size(ENGINE_STACK_BYTES);
    let interrupt = Arc::clone(bounds);
    engine.set_interrupt_handler(Some(Box::new(move || interrupt.step().is_some())));
    // A promise REJECTED with nothing to handle it ends a node process, so the runtime is
    // told of every such rejection and of every one that is handled after the fact: the
    // engine reports both, and which of the two it is decides whether the invocation fails
    // ([`runtime`]'s `rejectionTracked`).
    engine.set_host_promise_rejection_tracker(Some(Box::new(
        |ctx: Ctx<'_>, promise: Value<'_>, reason: Value<'_>, handled: bool| {
            // Before the runtime has installed its protocol there is no invocation to fail,
            // so a rejection then is nothing to report.
            let _: Result<(), String> =
                runtime_call(&ctx, "rejectionTracked", (promise, reason, handled));
        },
    )));
    // The only module resolution there is: this package's files and the core modules.
    engine.set_loader(RuntimeResolver, RuntimeLoader);
    let context = match Context::full(&engine) {
        Ok(context) => context,
        Err(error) => return Completion::Internal(format!("the engine did not start: {error}")),
    };
    let sources: Arc<BTreeMap<String, String>> = Arc::new(
        request
            .modules
            .iter()
            .map(|module| (module.path.clone(), module.source.clone()))
            .collect(),
    );
    let prepared = context.with(|ctx| -> Result<(), String> {
        install_host(&ctx, capture, &sources, bounds, request.aws)?;
        evaluate_script(&ctx, runtime::RUNTIME_SOURCE)?;
        // `Intl` and the local-zone reading of `Date`, which stand on the zone database the
        // host looks a zone's offset up in ([`intl`]).
        evaluate_script(&ctx, intl::INTL_SOURCE)?;
        // The runtime is installed; from here it loads the handler's module, which is the
        // rest of the INIT phase. The runtime itself says when that phase is over, by the
        // one host call that begins the invoke phase ([`install_host`]'s
        // `__hostBeginInvocation`) — so the bounds measure the handler and not the
        // environment it runs in.
        runtime_call::<_, ()>(&ctx, "start", (input,))
    });
    if let Err(error) = prepared {
        return Completion::Internal(error);
    }
    let completion = drive(&engine, &context, bounds);
    // The run is over, and nothing the engine does to tear itself down is the guest's
    // work: the interrupt handler that stopped it is taken off first, so no stop lands
    // inside the engine's own teardown.
    engine.set_interrupt_handler(None);
    // A bound the engine enforced by raising inside the guest leaves that exception
    // pending in the context; it is taken here, so nothing of the stopped run is left
    // held when the engine is torn down.
    context.with(|ctx| drop(ctx.catch()));
    completion
}

/// Drive the invocation to its answer: the engine's own job queue first, then the timer
/// queue the handler scheduled work on, until the run settles or one of its bounds ends it.
///
/// This is where a run WAITS, and it never waits past what the run has left: the host holds
/// the clock, so a handler that schedules work far in the future cannot hold the caller
/// open with it.
fn drive(engine: &Runtime, context: &Context, bounds: &Arc<Bounds>) -> Completion {
    let mut reported_empty_loop = false;
    let mut reported_exhausted = false;
    loop {
        let mut job_failed = false;
        loop {
            match engine.execute_pending_job() {
                Ok(true) => {}
                Ok(false) => break,
                // A job that ended on an exception nothing in the guest handled. The
                // invocation's own answer is read below; this is only noted, never
                // reported as a result.
                Err(_) => {
                    job_failed = true;
                    break;
                }
            }
            if bounds.stopped().is_some() {
                break;
            }
        }
        if let Some(stop) = bounds.stopped() {
            return Completion::Stopped(stop);
        }
        // A promise this run rejected with nothing to handle it ends the invocation the way
        // it ends a node process: reported before the handler's own answer is read, because
        // the process does not survive to deliver one.
        if let Err(error) =
            context.with(|ctx| runtime_call::<_, ()>(&ctx, "reportUnhandledRejection", ()))
        {
            return Completion::Internal(error);
        }
        // Read from the engine OUTSIDE the context below: the engine's own accounting is
        // not reachable while a context of it is entered.
        if memory_exhausted(engine) {
            // The host's own reading of the memory bound, which does not depend on what
            // the guest did with the engine's refusal of an allocation.
            return Completion::Stopped(bounds.breach(Stop::Memory));
        }
        let polled = context.with(|ctx| -> Result<Option<Completion>, String> {
            let answered: Option<String> = runtime_call(&ctx, "poll", ())?;
            Ok(answered.map(|text| read_completion(&text, bounds)))
        });
        match polled {
            Err(error) => return Completion::Internal(error),
            Ok(Some(completion)) => return completion,
            Ok(None) => {}
        }
        if job_failed {
            return Completion::Internal(
                "a job of the invocation ended on an exception the guest did not handle"
                    .to_string(),
            );
        }
        let pending = context.with(|ctx| -> Result<(Option<Duration>, String), String> {
            let delay: Option<f64> = runtime_call(&ctx, "nextTimerDelayMs", ())?;
            let waiting: String = runtime_call(&ctx, "waiting", ())?;
            Ok((delay.map(milliseconds), waiting))
        });
        let (delay, waiting) = match pending {
            Ok(pending) => pending,
            Err(error) => return Completion::Internal(error),
        };
        if let Some(delay) = delay {
            let remaining = bounds.remaining();
            if remaining.is_zero() {
                return Completion::Stopped(bounds.breach(Stop::Deadline));
            }
            ::std::thread::sleep(delay.min(remaining).min(TIMER_WAIT_SLICE));
            if let Err(error) = context.with(|ctx| runtime_call::<_, ()>(&ctx, "runDueTimers", ()))
            {
                return Completion::Internal(error);
            }
            continue;
        }
        // No job and no timer is left to run. For a non-async handler that is the moment
        // its answer is reported: "the runtime waits for the event loop to be empty" before
        // it answers with what the callback was given.
        if waiting == WAITING_FOR_LOOP && !reported_empty_loop {
            reported_empty_loop = true;
            if let Err(error) = context.with(|ctx| runtime_call::<_, ()>(&ctx, "eventLoopEmpty", ()))
            {
                return Completion::Internal(error);
            }
            continue;
        }
        // The handler's promise is still pending with nothing left that could settle it:
        // no further work of this run can happen. That is the point a node process has
        // nothing left to run and exits, so the invocation is reported as the runtime
        // having exited rather than held open until the function's timeout.
        if !reported_exhausted {
            reported_exhausted = true;
            if let Err(error) =
                context.with(|ctx| runtime_call::<_, ()>(&ctx, "eventLoopExhausted", ()))
            {
                return Completion::Internal(error);
            }
            continue;
        }
        return Completion::Internal(
            "the invocation did not answer and its runtime did not report an exit".to_string(),
        );
    }
}

/// A delay the runtime reported in milliseconds, as a duration. A value that is not a
/// finite non-negative number is no delay at all.
fn milliseconds(delay: f64) -> Duration {
    if !delay.is_finite() || delay <= 0.0 {
        return Duration::ZERO;
    }
    Duration::try_from_secs_f64(delay / 1_000.0).unwrap_or(Duration::ZERO)
}

/// Whether the run has reached the memory ceiling this module installed on the engine,
/// read from the ENGINE's own accounting rather than from anything the guest said.
///
/// The host's own half of the memory bound: it is read between the stretches of guest work
/// the host drives, so a run that holds the ceiling across them is stopped by the host
/// whatever the guest did with the engine's own refusal.
fn memory_exhausted(engine: &Runtime) -> bool {
    let usage = engine.memory_usage();
    let limit = usage.malloc_limit;
    if limit <= 0 {
        return false;
    }
    usage.malloc_size.saturating_add(MEMORY_CEILING_SLACK_BYTES) >= limit
}

/// Install the whole host surface of a run: the invocation log, the sources module
/// resolution answers from, and the computations the `crypto` core module is served by.
///
/// These functions are ALL a run can call of this process. None of them opens a host path,
/// reads a host environment variable or reaches another function's state: the log is this
/// invocation's own capture, the sources are the bytes the store holds for this version's
/// package and this emulator's own core modules, and a digest or a draw of random bytes is
/// arithmetic over what it was handed.
fn install_host(
    ctx: &Ctx<'_>,
    capture: &Arc<Mutex<Capture>>,
    sources: &Arc<BTreeMap<String, String>>,
    bounds: &Arc<Bounds>,
    aws: Option<&AwsAccess>,
) -> Result<(), String> {
    let log = Arc::clone(capture);
    let globals = ctx.globals();
    globals
        .set(
            "__hostLog",
            Func::from(move |text: String| captured(&log).write(&text)),
        )
        .map_err(|error| format!("the invocation log could not be installed: {error}"))?;
    let held = Arc::clone(sources);
    globals
        .set(
            "__hostModuleSource",
            Func::from(move |path: String| {
                held.get(&path).cloned().or_else(|| {
                    // The modules the RUNTIME supplies rather than the package: the AWS SDK
                    // a managed node runtime ships, which is one package per service.
                    super::node_sdk_guest::runtime_module_source(&path)
                })
            }),
        )
        .map_err(|error| format!("the package could not be installed: {error}"))?;
    let listed = Arc::clone(sources);
    globals
        .set(
            "__hostModulePaths",
            Func::from(move || listed.keys().cloned().collect::<Vec<String>>()),
        )
        .map_err(|error| format!("the package's entries could not be installed: {error}"))?;
    globals
        .set(
            "__hostCoreSource",
            Func::from(|name: String| core_module_source(&name).map(str::to_string)),
        )
        .map_err(|error| format!("the core modules could not be installed: {error}"))?;
    // How a module of the package is COMPILED: as a compilation unit named by the module's
    // own path, so a stack frame of it names the file the caller wrote rather than the
    // engine's anonymous input. The source is the runtime's own wrapper around the module's
    // text ([`runtime`]); nothing of the host is read for it.
    globals
        .set("__hostCompileScript", Func::from(compile_script))
        .map_err(|error| format!("the compiler could not be installed: {error}"))?;
    // Where the INIT phase ends and the handler's own run begins. The runtime calls this
    // once it has loaded the handler's module and resolved its entry point — which is
    // everything the function's `Timeout` does not bound — and the deadline it answers is
    // the instant that `Timeout` is up at, which is what the handler is told it has left.
    // It is ONE-SHOT ([`Bounds::start`]), and the runtime takes it off the globals with the
    // rest of this surface before a line of the package is loaded.
    let phase_bounds = Arc::clone(bounds);
    globals
        .set(
            "__hostBeginInvocation",
            Func::from(move || {
                // Milliseconds since the epoch as a NUMBER, which is how JavaScript states
                // an instant: the value is far inside the integers a double holds exactly.
                #[allow(clippy::cast_precision_loss)]
                {
                    phase_bounds.start() as f64
                }
            }),
        )
        .map_err(|error| format!("the invoke phase could not be installed: {error}"))?;
    // The three pure computations the `crypto` core module is served by. They reach no
    // host path, socket or environment: an algorithm name and hex bytes go in, hex bytes
    // come out ([`digest`]).
    globals
        .set(
            "__hostDigest",
            Func::from(|algorithm: String, data: String| digest::digest(&algorithm, &data)),
        )
        .map_err(|error| format!("the digests could not be installed: {error}"))?;
    globals
        .set(
            "__hostHmac",
            Func::from(|algorithm: String, key: String, data: String| {
                digest::hmac(&algorithm, &key, &data)
            }),
        )
        .map_err(|error| format!("the keyed digests could not be installed: {error}"))?;
    globals
        .set(
            "__hostEncode",
            Func::from(|coding: String, bytes: Vec<u8>| digest::encode(&coding, bytes)),
        )
        .map_err(|error| format!("the text codings could not be installed: {error}"))?;
    globals
        .set(
            "__hostDecode",
            Func::from(|coding: String, text: String| digest::decode(&coding, &text)),
        )
        .map_err(|error| format!("the text codings could not be installed: {error}"))?;
    globals
        .set(
            "__hostRandomHex",
            Func::from(|count: f64| {
                // The draw is made in HOST memory, outside the engine's own allocation
                // ceiling, so the count is checked against this module's ceiling BEFORE it
                // becomes a length: a value that is not a whole number inside the ceiling
                // is refused rather than converted (an `as` cast of a large float
                // saturates, which would turn a refusal into the largest allocation there
                // is).
                if !count.is_finite() || count < 0.0 {
                    return None;
                }
                let ceiling = digest::MAX_RANDOM_BYTES;
                #[allow(clippy::cast_precision_loss)]
                if count.fract() != 0.0 || count > ceiling as f64 {
                    return None;
                }
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                digest::random_hex(count as usize)
            }),
        )
        .map_err(|error| format!("the random source could not be installed: {error}"))?;
    // The OFFSET of a named time zone, which is the IANA database's answer for that zone at
    // that instant: the one fact `Intl` and the local reading of `Date` stand on.
    globals
        .set(
            "__hostZoneOffsetMs",
            Func::from(|zone: String, epoch_ms: f64| zone_offset_ms(&zone, epoch_ms)),
        )
        .map_err(|error| format!("the zone database could not be installed: {error}"))?;
    // The ONE call a handler reaches another service of this emulator by. The service
    // table, the signature and the destination are all [`super::aws_api`]'s, so what the
    // guest decides is only which operation of which reachable service it asks for and
    // what it sends; the two bounds that are this run's — how many calls it may make and
    // how long one may take — are held here, where no guest code can reach them.
    let access = aws.cloned();
    let call_bounds = Arc::clone(bounds);
    let made = AtomicU64::new(0);
    globals
        .set(
            "__hostAwsCall",
            Func::from(
                move |service: String, operation: String, region: String, body: String| {
                    // A run whose budget is already spent makes no further call: the
                    // latched stop is its answer, so work it starts afterwards is work
                    // nothing will report.
                    if call_bounds.stopped().is_some() {
                        return refusal_answer("not_made", "the run's budget is spent");
                    }
                    if made.fetch_add(1, Ordering::Relaxed) >= MAX_CALLS_PER_RUN as u64 {
                        return refusal_answer(
                            "not_made",
                            &format!(
                                "one invocation may make at most {MAX_CALLS_PER_RUN} AWS SDK \
                                 calls"
                            ),
                        );
                    }
                    let Some(access) = access.as_ref() else {
                        return refusal_answer(
                            "not_made",
                            "this invocation was given no AWS SDK seam, so no call was made",
                        );
                    };
                    let remaining = call_bounds.remaining();
                    if remaining.is_zero() {
                        return refusal_answer("timed_out", "");
                    }
                    call_answer(&access.call(&service, &operation, &region, &body, remaining))
                },
            ),
        )
        .map_err(|error| format!("the AWS SDK seam could not be installed: {error}"))?;
    Ok(())
}

/// What the guest reads back from one AWS SDK call: the service's own answer, or the reason
/// no call was made.
///
/// A document rather than an object, for the reason the runtime's own protocol is one: the
/// host's engine surface stays a string in and a string out.
fn call_answer(answered: &Result<Answer, Refusal>) -> String {
    let answer = match answered {
        Ok(answer) => answer,
        Err(Refusal::UnknownService) => {
            return refusal_answer(
                "unknown_service",
                &super::aws_api::reachable_services().join(", "),
            )
        }
        Err(Refusal::UnknownOperation) => return refusal_answer("unknown_operation", ""),
        Err(Refusal::TimedOut) => return refusal_answer("timed_out", ""),
        Err(Refusal::RequestTooLarge) => {
            return refusal_answer(
                "not_made",
                &format!(
                    "the request document is larger than the {} bytes one AWS SDK call may \
                     carry",
                    super::aws_api::MAX_REQUEST_BYTES
                ),
            )
        }
        Err(Refusal::ResponseTooLarge) => {
            return refusal_answer(
                "not_made",
                &format!(
                    "the answer is larger than the {} bytes one AWS SDK call may read back",
                    super::aws_api::MAX_RESPONSE_BYTES
                ),
            )
        }
        Err(Refusal::NotMade(detail)) => return refusal_answer("not_made", detail),
    };
    ::serde_json::json!({
        "outcome": "answered",
        "status": answer.status,
        "body": answer.body,
        "errorType": answer.error_type,
        "requestId": answer.request_id,
        "streams": answer.streams,
        "blobs": answer.blobs,
    })
    .to_string()
}

/// The document a call that was NOT made is reported to the guest with.
fn refusal_answer(outcome: &str, detail: &str) -> String {
    ::serde_json::json!({ "outcome": outcome, "detail": detail }).to_string()
}

/// The offset of the IANA time zone `zone` at `epoch_ms`, in milliseconds, or `None` when
/// the database holds no zone of that name.
///
/// The database is `chrono-tz`'s compiled-in copy of the IANA one, so a zone's offset — and
/// its daylight-saving transitions — are that database's and not this emulator's.
fn zone_offset_ms(zone: &str, epoch_ms: f64) -> Option<i64> {
    use ::chrono::Offset;
    use ::chrono::TimeZone;
    let named: ::chrono_tz::Tz = zone.parse().ok()?;
    if !epoch_ms.is_finite() {
        return None;
    }
    let instant = ::chrono::DateTime::from_timestamp_millis(epoch_ms as i64)?;
    let offset = named.offset_from_utc_datetime(&instant.naive_utc());
    Some(i64::from(offset.fix().local_minus_utc()) * 1_000)
}

/// Compile `source` as a unit named `name`, and answer with what it exports as its default.
///
/// This is how a CommonJS module of the package is compiled: the runtime wraps the module's
/// own text in a function that takes the module scope's five names, and the unit is named by
/// the module's own path, so every frame of a stack taken inside it names that path and that
/// line. The engine's own `Function` constructor cannot be told a name, which is why this
/// one step is the host's.
fn compile_script<'js>(
    ctx: Ctx<'js>,
    name: String,
    source: String,
) -> rquickjs::Result<Value<'js>> {
    let declared = Module::declare(ctx.clone(), name, source)?;
    let (evaluated, _) = declared.eval()?;
    evaluated.get::<_, Value<'js>>("default")
}

/// Evaluate one script of THIS emulator's own in `ctx`.
fn evaluate_script(ctx: &Ctx<'_>, source: &str) -> Result<(), String> {
    ctx.eval::<Value<'_>, _>(source)
        .catch(ctx)
        .map(|_| ())
        .map_err(|caught| format!("the Lambda runtime source does not run: {caught}"))
}

/// Call one function of the runtime's own protocol object.
fn runtime_call<'js, A, R>(ctx: &Ctx<'js>, name: &str, args: A) -> Result<R, String>
where
    A: rquickjs::function::IntoArgs<'js>,
    R: rquickjs::FromJs<'js>,
{
    let runtime: Object<'js> = ctx
        .globals()
        .get("__lambdaRuntime")
        .map_err(|error| format!("the Lambda runtime did not install itself: {error}"))?;
    let function: Function<'js> = runtime
        .get(name)
        .map_err(|error| format!("the Lambda runtime has no {name}: {error}"))?;
    function
        .call(args)
        .catch(ctx)
        .map_err(|caught| format!("the Lambda runtime's {name} failed: {caught}"))
}

/// The runtime's answer, as this module reports it.
fn read_completion(answered: &str, bounds: &Arc<Bounds>) -> Completion {
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(answered) else {
        return Completion::Internal("the runtime answered with a document that is not JSON".into());
    };
    let member = |name: &str| {
        document
            .get(name)
            .and_then(::serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    match document.get("outcome").and_then(::serde_json::Value::as_str) {
        Some("returned") => {
            let payload = member("payload");
            if payload.len() > bounds.response_bytes {
                return Completion::Stopped(bounds.breach(Stop::Response));
            }
            Completion::Returned(payload)
        }
        Some("streamed") => {
            let chunks: Vec<String> = document
                .get("chunks")
                .and_then(::serde_json::Value::as_array)
                .map(|written| {
                    written
                        .iter()
                        .filter_map(::serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if chunks.iter().map(String::len).sum::<usize>() > bounds.response_bytes {
                return Completion::Stopped(bounds.breach(Stop::Response));
            }
            // A streaming handler that failed once its stream had already carried bytes
            // reports that failure beside the writes, and the runtime reports it only then
            // ([`runtime`]'s `callStreamingHandler`): the bytes stay the answer, and the
            // failure is what the stream's trailer names.
            let failure = document
                .get("errorType")
                .and_then(::serde_json::Value::as_str)
                .map(|error_type| StreamFailure {
                    error_type: error_type.to_string(),
                    error_message: member("errorMessage"),
                    stack_trace: traced(&document),
                });
            Completion::Streamed { chunks, failure }
        }
        Some("raised") => {
            let error_type = member("errorType");
            let error_message = member("errorMessage");
            // An allocation the engine REFUSED because this run reached the ceiling
            // installed on it is that bound doing its job, not an error of the handler's
            // own making, so it is reported as the stop. The engine's own error is the
            // only signal there is for it: the refusal unwinds the guest's allocation
            // before the host is next outside the engine, so the engine's accounting
            // ([`memory_exhausted`]) no longer shows the ceiling by the time the host
            // could read it. A guest can raise an error wearing these same words, which
            // mislabels the failure of its OWN run and nothing else — every bound here is
            // the host's and none of them is read from what the guest says.
            if error_type == ENGINE_INTERNAL_ERROR && error_message.contains(ENGINE_OUT_OF_MEMORY)
            {
                return Completion::Stopped(bounds.breach(Stop::Memory));
            }
            Completion::Raised {
                error_type,
                error_message,
                stack_trace: traced(&document),
            }
        }
        _ => Completion::Internal("the runtime answered with no outcome".into()),
    }
}

/// The formatted frames of the stack the runtime reported on `document`, one per entry.
fn traced(document: &::serde_json::Value) -> Vec<String> {
    document
        .get("trace")
        .and_then(::serde_json::Value::as_array)
        .map(|frames| {
            frames
                .iter()
                .filter_map(::serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The JSON document the runtime source is handed.
fn runtime_input(request: &Run<'_>) -> Result<String, String> {
    let environment: ::serde_json::Map<String, ::serde_json::Value> = request
        .environment
        .iter()
        .map(|(name, value)| (name.clone(), ::serde_json::Value::String(value.clone())))
        .collect();
    let client_context = match request.client_context {
        Some(text) => {
            ::serde_json::from_str::<::serde_json::Value>(text).unwrap_or(::serde_json::Value::Null)
        }
        None => ::serde_json::Value::Null,
    };
    let document = ::serde_json::json!({
        "handler": request.handler,
        "environment": environment,
        "event": request.event,
        "requestId": request.request_id,
        "invokedFunctionArn": request.invoked_function_arn,
        "clientContext": client_context,
        "tenantId": request.tenant_id,
        "nodeMajor": node_major(request.runtime),
        "nodeVersion": node_version(request.runtime),
        // The platform this process really runs on, and the architecture the FUNCTION is
        // configured for — which is the architecture its execution environment has.
        "platform": platform(),
        "arch": node_architecture(request.architecture),
    });
    ::serde_json::to_string(&document).map_err(|error| error.to_string())
}

/// The `process.platform` of this process: the name Node gives the operating system this
/// build runs on.
fn platform() -> &'static str {
    match ::std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

/// The `process.arch` an `Architectures` entry names: the name Node gives the instruction
/// set the function is configured for.
///
/// The model's value set is `x86_64` and `arm64` (`smithy_query(query="shape",
/// shape="Architecture")`); Node names the first of them `x64` and the second by the same
/// name.
fn node_architecture(architecture: &str) -> &str {
    match architecture {
        "x86_64" => "x64",
        other => other,
    }
}

/// The id prefix the entries of a LAYER are keyed under, which is what the runtime mounts
/// at the layer root.
pub const LAYER_ID_PREFIX: &str = "layer:";

/// The id prefix the modules the RUNTIME itself supplies are keyed under.
pub const RUNTIME_ID_PREFIX: &str = "runtime:";

/// The MAJOR version of the Node release a `Runtime` identifier names — the digits before
/// the first `.` of what follows `nodejs` — or the empty string for the unversioned
/// identifier.
///
/// This is what the layer search path's own `node<major>` directory is keyed by, so a layer
/// built for another major version of the runtime is not on the path at all.
pub(super) fn node_major(runtime: &str) -> String {    runtime
        .strip_prefix(super::NODE_RUNTIME_PREFIX)
        .unwrap_or_default()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect()
}

/// The release the run reports as `process.version` and `process.versions.node`: the
/// version the `Runtime` identifier names, as a three-part version.
///
/// Only what the identifier states is reported — `nodejs22.x` is the 22 line and `nodejs8.10`
/// the 8.10 one — because the identifier is the only statement of a version there is.
fn node_version(runtime: &str) -> String {
    let named = runtime
        .strip_prefix(super::NODE_RUNTIME_PREFIX)
        .unwrap_or_default();
    let mut parts: Vec<String> = Vec::new();
    for part in named.split('.') {
        // The leading digits of the part, so a suffix the identifier carries beside the
        // number (`4.3-edge`) names the same release as the number alone.
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            break;
        }
        parts.push(digits);
    }
    match parts.len() {
        0 => "0.0.0".to_string(),
        1 => format!("{}.0.0", parts[0]),
        _ => format!("{}.{}.0", parts[0], parts[1]),
    }
}

/// How an `import` specifier is resolved: by the runtime's own resolution, which answers
/// with a file of this package or a core module and with nothing else.
#[derive(Debug, Clone, Copy)]
struct RuntimeResolver;

impl Resolver for RuntimeResolver {
    fn resolve<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        let resolved: Option<String> =
            runtime_call(ctx, "resolveModule", (base.to_string(), name.to_string()))
                .map_err(|_| module_not_found(ctx, name))?;
        resolved.ok_or_else(|| module_not_found(ctx, name))
    }
}

/// The failure a specifier that resolves to nothing is reported as: the error the node
/// runtime raises for a module it cannot find, raised as the exception the resolution fails
/// with.
///
/// It is the SAME error a `require` of that specifier throws — `MODULE_NOT_FOUND`, "Cannot
/// find module '<specifier>'" — so a failed `import` is classified the way a failed `require`
/// is, which is the class the node runtime reports for a module it could not import
/// (`Runtime.ImportModuleError`) rather than as whatever the engine's own resolver failure
/// looks like.
fn module_not_found<'js>(ctx: &Ctx<'js>, name: &str) -> rquickjs::Error {
    match runtime_call::<_, Value<'js>>(ctx, "moduleNotFound", (name.to_string(),)) {
        Ok(error) => ctx.throw(error),
        Err(_) => rquickjs::Error::new_resolving(String::new(), name.to_string()),
    }
}

/// How a resolved module is loaded: from the source the runtime hands back for it. No host
/// path is opened for a module, because there is no module outside this package and this
/// emulator's own core modules.
#[derive(Debug, Clone, Copy)]
struct RuntimeLoader;

impl Loader for RuntimeLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        path: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<Module<'js>> {
        let source: Option<String> = runtime_call(ctx, "moduleSource", (path.to_string(),))
            .map_err(|_| rquickjs::Error::new_loading(path.to_string()))?;
        let Some(source) = source else {
            return Err(rquickjs::Error::new_loading(path.to_string()));
        };
        Module::declare(ctx.clone(), path.to_string(), source)
    }
}

/// The run's budget, as the engine's interrupt handler spends it.
#[derive(Debug)]
pub struct Bounds {
    /// When the PHASE the run is in must be over: the initialization of the execution
    /// environment until [`Self::start`], and the handler's own run from there.
    deadline: Mutex<Instant>,
    /// How long the handler may run for.
    wall_clock: Duration,
    /// The clock every deadline of this run is read against, which the HOST hands the run
    /// ([`run_against`]) and no guest code can reach.
    clock: Clock,
    /// When the HANDLER's run must be over, in milliseconds since the epoch, which is what
    /// the handler is told it has left. Set by [`Self::start`].
    deadline_epoch_ms: ::std::sync::atomic::AtomicI64,
    /// Whether the INVOKE phase has begun ([`Self::start`]).
    spending: AtomicBool,
    /// How many engine checks are left.
    checks: AtomicU64,
    /// How large the handler's response may be.
    response_bytes: usize,
    /// Which bound was breached, once one was. Latched: the answer to a run that spent its
    /// budget is that stop, whatever the guest does afterwards.
    stopped: Mutex<Option<Stop>>,
    /// Whether a bound was breached at all, read without taking the lock.
    breached: AtomicBool,
}

impl Bounds {
    /// The bounds of a run with `budget`, in its INIT phase: the deadline is the one the
    /// initialization of the execution environment has, which is not the function's
    /// `Timeout` — the phase has a limit of its own ([`super::INIT_PHASE_LIMIT`]), and AWS
    /// retries an initialization that outlasts it under the configured function timeout, so
    /// what an initialization is given up on after is the two spans together. The handler's
    /// own deadline is taken by [`Self::start`], when the invoke phase begins.
    fn new(budget: &Budget, clock: Clock) -> Self {
        Self {
            deadline: Mutex::new(
                clock.now() + super::INIT_PHASE_LIMIT.saturating_add(budget.wall_clock),
            ),
            wall_clock: budget.wall_clock,
            clock,
            deadline_epoch_ms: ::std::sync::atomic::AtomicI64::new(0),
            spending: AtomicBool::new(false),
            checks: AtomicU64::new(MAX_ENGINE_CHECKS),
            response_bytes: budget.response_bytes,
            stopped: Mutex::new(None),
            breached: AtomicBool::new(false),
        }
    }

    /// Begin the INVOKE phase: the handler's deadline runs from HERE, so what the bounds
    /// measure is the handler's own work rather than the execution environment this
    /// emulator built to run it in — the engine, the runtime and the load of the handler's
    /// own module, none of which the function's `Timeout` bounds. Answers when that
    /// deadline falls, in milliseconds since the epoch, which is what the handler is told
    /// it has left.
    ///
    /// ONE-SHOT: the invoke phase of a run begins once. A second call answers the same
    /// deadline and moves nothing, so reaching this again cannot extend a deadline that is
    /// already running.
    fn start(&self) -> i64 {
        if self.spending.swap(true, Ordering::SeqCst) {
            return self.deadline_epoch_ms.load(Ordering::Relaxed);
        }
        *self.deadline.lock().unwrap_or_else(PoisonError::into_inner) =
            self.clock.now() + self.wall_clock;
        // What the handler is TOLD it has left is a host wall-clock instant rather than a
        // bound: the bound is the deadline above, which this run's own clock is read for.
        let epoch_ms = super::epoch_millis(SystemTime::now() + self.wall_clock);
        self.deadline_epoch_ms.store(epoch_ms, Ordering::Relaxed);
        epoch_ms
    }

    /// When the run must be over.
    fn deadline(&self) -> Instant {
        *self.deadline.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Answer the engine's question of whether this run should stop: `None` to carry on,
    /// `Some(stop)` for the bound that is now breached.
    fn step(&self) -> Option<Stop> {
        if let Some(stop) = self.stopped() {
            return Some(stop);
        }
        if self.checks.fetch_sub(1, Ordering::Relaxed) == 0 {
            return Some(self.breach(Stop::Steps));
        }
        // The HOST time one run may take, which a clock the host drives states beside its own
        // ([`Clock::within`]) and which is read from this host's own clock whatever this run's
        // deadline is read against.
        if self.clock.past_host_ceiling() {
            return Some(self.breach(Stop::HostCeiling));
        }
        if self.clock.now() >= self.deadline() {
            return Some(self.breach(Stop::Deadline));
        }
        None
    }

    /// Latch `stop` as the run's answer and report it.
    fn breach(&self, stop: Stop) -> Stop {
        let mut latched = self.stopped.lock().unwrap_or_else(PoisonError::into_inner);
        let already = *latched.get_or_insert(stop);
        self.breached.store(true, Ordering::Relaxed);
        already
    }

    /// The bound this run breached, if it breached one.
    #[must_use]
    pub fn stopped(&self) -> Option<Stop> {
        if !self.breached.load(Ordering::Relaxed) {
            return None;
        }
        *self.stopped.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How long the run has left.
    fn remaining(&self) -> Duration {
        self.deadline().saturating_duration_since(self.clock.now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A budget small enough to exhaust in a test and large enough for any handler one
    /// runs.
    fn budget() -> Budget {
        Budget {
            wall_clock: Duration::from_secs(5),
            steps: 2_000_000,
            memory_bytes: 256 * 1024 * 1024,
            log_bytes: 16 * 1024,
            response_bytes: 64 * 1024,
            recursion_limit: 2_000,
        }
    }

    /// Run `entries` as a package, on a thread with the stack a run is given — the same
    /// way [`super::super::run_invocation`] runs one.
    fn ran_package(entries: &[(&str, &str)], handler: &str, event: Option<&str>) -> Finished {
        ran_with(entries, handler, event, &BTreeMap::new(), budget())
    }

    /// Run `source` as `index.js` of a one-file package.
    fn ran(source: &str, handler: &str, event: Option<&str>) -> Finished {
        ran_package(&[("index.js", source)], handler, event)
    }

    /// How long this module waits for a run's own thread to end before it reports the run as
    /// one that did not end at all.
    ///
    /// NOT a bound on the run: every bound a run has is the host's own, read inside the run
    /// ([`Bounds`]). This is the backstop that keeps a run which will not end from wedging
    /// the suite — the host gives such a run up and answers its caller anyway
    /// ([`super::super::run_invocation`]), and so does this module. It is far above any span
    /// a row's own bound takes, so no row is ever answered by it.
    const RUN_ANSWER_CEILING: Duration = Duration::from_secs(240);

    /// [`ran_package`] with an environment and a budget of its own, against this host's own
    /// clock.
    fn ran_with(
        entries: &[(&str, &str)],
        handler: &str,
        event: Option<&str>,
        environment: &BTreeMap<String, String>,
        budget: Budget,
    ) -> Finished {
        ran_against(
            entries,
            handler,
            event,
            environment,
            budget,
            &Clock::host(),
            "a run against this host's own clock",
        )
    }

    /// [`ran_with`] against `clock`, which is what leaves ONE bound able to answer the run:
    /// a row that names a bound other than the deadline is judged against a clock that does
    /// not advance, so no span of this host's time can answer it instead.
    ///
    /// The run's answer is waited for rather than joined on, so a run that will not end
    /// fails the row that asked for it ([`RUN_ANSWER_CEILING`]) instead of holding the suite.
    fn ran_against(
        entries: &[(&str, &str)],
        handler: &str,
        event: Option<&str>,
        environment: &BTreeMap<String, String>,
        budget: Budget,
        clock: &Clock,
        case: &str,
    ) -> Finished {
        let modules: Vec<PackageModule> = entries
            .iter()
            .map(|(path, source)| PackageModule {
                path: (*path).to_string(),
                source: (*source).to_string(),
            })
            .collect();
        let handler = handler.to_string();
        let event = event.map(str::to_string);
        let environment = environment.clone();
        let clock = clock.clone();
        let (answered, answer) = ::std::sync::mpsc::channel();
        ::std::thread::Builder::new()
            .stack_size(crate::state::execution::RUN_STACK_BYTES)
            .spawn(move || {
                let finished = run_against(
                    &Run {
                        modules: &modules,
                        handler: &handler,
                        runtime: "nodejs22.x",
                        architecture: "x86_64",
                        environment: &environment,
                        event: event.as_deref(),
                        request_id: "11111111-2222-3333-4444-555555555555",
                        invoked_function_arn:
                            "arn:aws:lambda:us-west-2:000000000000:function:probe",
                        client_context: None,
                        tenant_id: None,
                        budget,
                        heap_bytes: 128 * 1024 * 1024,
                        aws: None,
                    },
                    &clock,
                );
                let _ = answered.send(finished);
            })
            .expect("a run thread");
        answer.recv_timeout(RUN_ANSWER_CEILING).unwrap_or_else(|_| {
            panic!("case {case}: the run did not end, so no bound of it ended it")
        })
    }

    /// The JSON the handler returned, or a panic naming what it came to instead.
    fn returned(finished: &Finished, case: &str) -> ::serde_json::Value {
        match &finished.completion {
            Completion::Returned(payload) => ::serde_json::from_str(payload)
                .unwrap_or_else(|_| panic!("case {case}: expected JSON, got {payload:?}")),
            other => panic!("case {case}: expected a returned value, got {other:?}"),
        }
    }

    /// The handler's own value is what comes back, for one row per JSON type a handler can
    /// return and one per way of returning it — so the answer is the run's, not a shape
    /// this emulator fills in.
    #[test]
    fn the_answer_is_what_the_handler_returned() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "an async arrow handler",
                "exports.handler = async (event) => ({ ok: true });",
                r#"{"ok":true}"#,
            ),
            (
                "a promise the handler returned",
                "exports.handler = function () { return Promise.resolve([1, 2, 3]); };",
                "[1,2,3]",
            ),
            (
                "a value computed from the event",
                "exports.handler = async (event) => ({ doubled: event.n * 2 });",
                r#"{"doubled":42}"#,
            ),
            (
                "a string",
                "exports.handler = async () => 'hello';",
                r#""hello""#,
            ),
            ("null", "exports.handler = async () => null;", "null"),
            (
                "a value from the engine's own library",
                "exports.handler = async () => ({ parsed: JSON.parse('[1,2]') });",
                r#"{"parsed":[1,2]}"#,
            ),
            (
                "a module.exports handler",
                "module.exports.handler = async () => ({ exported: true });",
                r#"{"exported":true}"#,
            ),
        ];
        for (case, source, expected) in cases {
            let finished = ran(source, "index.handler", Some(r#"{"n":21}"#));
            let actual = returned(&finished, case);
            let wanted: ::serde_json::Value =
                ::serde_json::from_str(expected).expect("the expected JSON");
            assert_eq!(
                actual, wanted,
                "case {case}: expected {wanted}, got {actual}"
            );
        }
    }

    /// One row of a module table: the case, the package's entries, and the JSON the
    /// handler is expected to answer with.
    type ModuleCase<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);

    /// A handler written as an ES MODULE runs too, under each rule by which Node decides a
    /// file is one — one row per form, and one importing another module of the package.
    #[test]
    fn an_es_module_handler_runs() {
        let cases: &[ModuleCase<'_>] = &[
            (
                "an .mjs handler",
                &[("index.mjs", "export const handler = async () => ({ esm: 'mjs' });")],
                r#"{"esm":"mjs"}"#,
            ),
            (
                "a .js handler of a package declared a module",
                &[
                    ("package.json", r#"{"type":"module"}"#),
                    ("index.js", "export const handler = async () => ({ esm: 'type' });"),
                ],
                r#"{"esm":"type"}"#,
            ),
            (
                "a .js handler of a nested directory of a package declared a module",
                &[
                    ("package.json", r#"{"type":"module"}"#),
                    (
                        "src/index.js",
                        "export const handler = async () => ({ esm: 'detected' });",
                    ),
                ],
                r#"{"esm":"detected"}"#,
            ),
            (
                "an export the handler string addresses by path",
                &[(
                    "index.mjs",
                    "export const nested = { handler: async () => ({ esm: 'nested' }) };",
                )],
                r#"{"esm":"nested"}"#,
            ),
            (
                "an es module importing the package's own modules",
                &[
                    (
                        "index.mjs",
                        "import { value } from './lib/sibling.mjs';\n\
                         import { join } from 'node:path';\n\
                         import data from './data.json';\n\
                         export const handler = async () => ({\n\
                             esm: value, joined: join('a', 'b'), data: data.value });\n",
                    ),
                    ("lib/sibling.mjs", "export const value = 'imported';"),
                    ("data.json", r#"{"value":"json"}"#),
                ],
                r#"{"esm":"imported","joined":"a/b","data":"json"}"#,
            ),
        ];
        for (case, entries, expected) in cases {
            let handler = match *case {
                "an export the handler string addresses by path" => "index.nested.handler",
                "a .js handler of a nested directory of a package declared a module" => {
                    "src/index.handler"
                }
                _ => "index.handler",
            };
            let finished = ran_package(entries, handler, None);
            let actual = returned(&finished, case);
            let wanted: ::serde_json::Value =
                ::serde_json::from_str(expected).expect("the expected JSON");
            assert_eq!(
                actual, wanted,
                "case {case}: expected {wanted}, got {actual}"
            );
        }
    }

    /// A manifest whose `main` leads back to a directory already being resolved does not
    /// make the resolution recurse without end: the chain is bounded, so the require
    /// resolves to nothing and the invocation is answered — one row per shape of cycle a
    /// package can hold.
    #[test]
    fn a_cyclic_package_manifest_resolves_to_nothing() {
        let cases: &[ModuleCase<'_>] = &[
            (
                "a manifest whose main is its own directory",
                &[
                    ("index.js", "require('dep');"),
                    ("node_modules/dep/package.json", r#"{"main":"."}"#),
                ],
                "Runtime.ImportModuleError",
            ),
            (
                "two manifests naming each other",
                &[
                    ("index.js", "require('first');"),
                    (
                        "node_modules/first/package.json",
                        r#"{"main":"../second"}"#,
                    ),
                    (
                        "node_modules/second/package.json",
                        r#"{"main":"../first"}"#,
                    ),
                ],
                "Runtime.ImportModuleError",
            ),
        ];
        for (case, entries, expected) in cases {
            let finished = ran_package(entries, "index.handler", None);
            match &finished.completion {
                Completion::Raised { error_type, .. } => assert_eq!(
                    error_type, expected,
                    "case {case}: expected {expected}, got {error_type}"
                ),
                other => panic!("case {case}: expected a raised error, got {other:?}"),
            }
        }
    }

    /// A callback-style handler answers with what it passed its callback, and a handler
    /// that never calls it answers `null` once the event loop is empty.
    #[test]
    fn a_callback_handler_answers_with_what_it_passed_the_callback() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "a callback called at once",
                "exports.handler = function (event, context, callback) { \
                 callback(null, { called: true }); };",
                r#"{"called":true}"#,
            ),
            (
                "a callback called from a timer",
                "exports.handler = function (event, context, callback) { \
                 setTimeout(function () { callback(null, { late: true }); }, 10); };",
                r#"{"late":true}"#,
            ),
            (
                "a callback with the wait switched off",
                "exports.handler = function (event, context, callback) { \
                 context.callbackWaitsForEmptyEventLoop = false; \
                 callback(null, { prompt: true }); };",
                r#"{"prompt":true}"#,
            ),
            (
                "a handler that never calls it",
                "exports.handler = function (event, context, callback) { return 1; };",
                "null",
            ),
        ];
        for (case, source, expected) in cases {
            let finished = ran(source, "index.handler", None);
            let actual = returned(&finished, case);
            let wanted: ::serde_json::Value =
                ::serde_json::from_str(expected).expect("the expected JSON");
            assert_eq!(
                actual, wanted,
                "case {case}: expected {wanted}, got {actual}"
            );
        }
    }

    /// An error the handler raised is reported as the error it is, with the engine's own
    /// trace — one row per way a handler fails.
    #[test]
    fn a_handler_that_raised_is_reported_as_raised() {
        let cases: &[(&str, &str, &str, &str)] = &[
            (
                "a thrown Error",
                "exports.handler = async () => { throw new Error('boom'); };",
                "Error",
                "boom",
            ),
            (
                "a thrown TypeError",
                "exports.handler = async () => { null.member; };",
                "TypeError",
                "",
            ),
            (
                "a rejected promise",
                "exports.handler = () => Promise.reject(new RangeError('out of range'));",
                "RangeError",
                "out of range",
            ),
            (
                "an error passed to the callback",
                "exports.handler = function (event, context, callback) { \
                 callback(new Error('callback error')); };",
                "Error",
                "callback error",
            ),
            (
                "a handler the module does not export",
                "exports.other = async () => 1;",
                "Runtime.HandlerNotFound",
                "index.handler is undefined or not exported",
            ),
            (
                "an export that is not a function",
                "exports.handler = 'not callable';",
                "Runtime.HandlerNotFound",
                "index.handler is not a function",
            ),
            (
                "a module that does not load",
                "require('./absent-module');",
                "Runtime.ImportModuleError",
                "Cannot find module './absent-module'",
            ),
            (
                "source that does not compile",
                "exports.handler = async () => { this is not javascript };",
                "Runtime.UserCodeSyntaxError",
                "",
            ),
            (
                "a value that cannot be serialized",
                "exports.handler = async () => { const held = {}; held.self = held; return held; };",
                "Error",
                "Unable to stringify response body",
            ),
        ];
        for (case, source, error_type, message) in cases {
            let finished = ran(source, "index.handler", None);
            match &finished.completion {
                Completion::Raised {
                    error_type: actual_type,
                    error_message,
                    ..
                } => {
                    assert_eq!(
                        actual_type, error_type,
                        "case {case}: expected the error type {error_type}, got {actual_type}"
                    );
                    assert!(
                        error_message.to_lowercase().contains(&message.to_lowercase()),
                        "case {case}: expected {message:?} in the message, got {error_message:?}"
                    );
                }
                other => panic!("case {case}: expected a raised error, got {other:?}"),
            }
        }
    }

    /// A handler named by a malformed `Handler` string is the runtime's own failure, not a
    /// run — one row per way the string names nothing callable.
    #[test]
    fn a_malformed_handler_name_is_reported_as_one() {
        for handler in ["index", "", ".handler", "index."] {
            let finished = ran("exports.handler = async () => 1;", handler, None);
            match &finished.completion {
                Completion::Raised { error_type, .. } => assert_eq!(
                    error_type, "Runtime.MalformedHandlerName",
                    "case {handler:?}: expected the malformed-name failure, got {error_type}"
                ),
                other => panic!("case {handler:?}: expected a raised error, got {other:?}"),
            }
        }
    }

    /// A package's own modules resolve the way Node resolves them — one row per form a
    /// `require` takes — and nothing outside the package resolves at all.
    #[test]
    fn require_resolves_the_packages_own_files_and_the_core_modules() {
        let entries: &[(&str, &str)] = &[
            (
                "index.js",
                "const sibling = require('./sibling');\n\
                 const nested = require('./lib/nested.js');\n\
                 const data = require('./data.json');\n\
                 const dep = require('dependency');\n\
                 const path = require('path');\n\
                 const util = require('node:util');\n\
                 exports.handler = async () => ({\n\
                     sibling: sibling.value,\n\
                     nested: nested.value,\n\
                     data: data.value,\n\
                     dep: dep.value,\n\
                     joined: path.join('a', 'b', '..', 'c'),\n\
                     formatted: util.format('%s=%d', 'n', 7),\n\
                 });\n",
            ),
            ("sibling.js", "exports.value = 'sibling';\n"),
            ("lib/nested.js", "exports.value = 'nested';\n"),
            ("data.json", r#"{"value":"data"}"#),
            (
                "node_modules/dependency/package.json",
                r#"{"name":"dependency","main":"main.js"}"#,
            ),
            (
                "node_modules/dependency/main.js",
                "module.exports = { value: 'dependency' };\n",
            ),
        ];
        let finished = ran_package(entries, "index.handler", None);
        let answered = returned(&finished, "a package that requires its own files");
        let expected = ::serde_json::json!({
            "sibling": "sibling",
            "nested": "nested",
            "data": "data",
            "dep": "dependency",
            "joined": "a/c",
            "formatted": "n=7",
        });
        assert_eq!(
            answered, expected,
            "expected {expected}, got {answered}"
        );
    }

    /// Nothing of the HOST is reachable from guest code: one row per thing a Node program
    /// could otherwise reach the host with, each of which resolves to nothing here. The
    /// traversal rows are the point of the one resolution helper: a specifier is
    /// normalised before it is looked up, so no form of `..` — written relatively, under
    /// the task root, or inside a bare package name — names anything outside the package.
    #[test]
    fn no_host_module_or_path_resolves_for_a_run() {
        let cases: &[(&str, &str)] = &[
            ("a host file", "require('fs').readFileSync('/etc/passwd')"),
            (
                "a host file outside the mounts, read as a module",
                "require('/etc/passwd')",
            ),
            (
                "a write outside the writable directory",
                "require('fs').writeFileSync('/var/task/written.txt', 'x')",
            ),
            ("a child process", "require('child_process')"),
            ("a socket", "require('net')"),
            ("a native binding", "process.binding('fs')"),
            ("a host path above the task root", "require('../../../etc/passwd')"),
            (
                "a traversal under the task root",
                "require('/var/task/../../../etc/passwd')",
            ),
            (
                "a traversal dressed as the task root itself",
                "require('/var/task/../task/../../etc/passwd')",
            ),
            (
                "a traversal through a package name",
                "require('../node_modules/../../../etc/passwd')",
            ),
            ("the task root itself", "require('/var/task/')"),
            ("the host's own loader", "require('module')"),
        ];
        for (case, reach) in cases {
            let source = format!(
                "exports.handler = async () => {{ {reach}; return {{ reached: true }}; }};"
            );
            let finished = ran(&source, "index.handler", None);
            assert!(
                matches!(finished.completion, Completion::Raised { .. }),
                "case {case}: expected the run to be refused the host, got {:?}",
                finished.completion
            );
        }
    }

    /// A `Handler` whose module part leaves the package names no module at all: it is
    /// normalised and contained the same way a `require` is.
    #[test]
    fn a_handler_module_that_leaves_the_package_resolves_to_nothing() {
        for handler in [
            "../../../etc/passwd.handler",
            "/var/task/../../etc/passwd.handler",
            "...handler",
        ] {
            let finished = ran("exports.handler = async () => 1;", handler, None);
            match &finished.completion {
                Completion::Raised { error_type, .. } => assert!(
                    error_type == "Runtime.ImportModuleError"
                        || error_type == "Runtime.MalformedHandlerName",
                    "case {handler:?}: expected the module not to resolve, got {error_type}"
                ),
                other => panic!("case {handler:?}: expected a raised error, got {other:?}"),
            }
        }
    }

    /// The host functions the runtime was built on are not reachable from guest code, and
    /// the protocol object the host calls cannot be replaced by it.
    #[test]
    fn the_host_functions_are_not_reachable_from_guest_code() {
        let finished = ran(
            "exports.handler = async () => {\n\
                 const replace = function () {\n\
                     try {\n\
                         globalThis.__lambdaRuntime.poll = null;\n\
                         return globalThis.__lambdaRuntime.poll !== null;\n\
                     } catch (error) {\n\
                         return true;\n\
                     }\n\
                 };\n\
                 return {\n\
                     log: typeof globalThis.__hostLog,\n\
                     source: typeof globalThis.__hostModuleSource,\n\
                     core: typeof globalThis.__hostCoreSource,\n\
                     frozen: Object.isFrozen(globalThis.__lambdaRuntime),\n\
                     kept: replace(),\n\
                 };\n\
             };\n",
            "index.handler",
            None,
        );
        let answered = returned(&finished, "the host surface a run can reach");
        let expected = ::serde_json::json!({
            "log": "undefined",
            "source": "undefined",
            "core": "undefined",
            "frozen": true,
            "kept": true,
        });
        assert_eq!(answered, expected, "expected {expected}, got {answered}");
    }

    /// The protocol object's own `requireId` is no second route to a lookup key: an id this
    /// runtime did not produce — one that would leave a mount through `..`, an absolute host
    /// path, an empty string, a value that is not a string at all — resolves to nothing,
    /// because every lookup re-normalises its id against the mount and tail it names.
    ///
    /// The rows differ along the FORM the id is written in, which is what the normalisation
    /// generalises over, and each must come back as the resolution failure a node runtime
    /// reports for a module it cannot find.
    #[test]
    fn a_require_id_the_runtime_did_not_produce_resolves_to_nothing() {
        let finished = ran(
            "exports.handler = async () => {\n\
                 const ids = [\n\
                     '../../../etc/passwd',\n\
                     './../../etc/passwd',\n\
                     '/etc/passwd',\n\
                     'layer:../../etc/passwd',\n\
                     'runtime:../../../etc/passwd',\n\
                     '',\n\
                     7,\n\
                 ];\n\
                 return ids.map(function (id) {\n\
                     try {\n\
                         globalThis.__lambdaRuntime.requireId(id);\n\
                         return 'resolved';\n\
                     } catch (error) {\n\
                         return (error && error.code) || String(error);\n\
                     }\n\
                 });\n\
             };\n",
            "index.handler",
            None,
        );
        let answered = returned(&finished, "an id this runtime did not produce");
        let refused = ::serde_json::json!("MODULE_NOT_FOUND");
        let outcomes = answered
            .as_array()
            .unwrap_or_else(|| panic!("expected one outcome per id, got {answered}"))
            .clone();
        assert_eq!(
            outcomes.len(),
            7,
            "expected one outcome per id, got {answered}"
        );
        for (index, outcome) in outcomes.iter().enumerate() {
            assert_eq!(
                outcome, &refused,
                "id {index}: expected {refused} for an id the runtime did not produce, got \
                 {outcome} in {answered}"
            );
        }
    }

    /// The environment a handler reads is the invocation's own, and the context is the one
    /// the runtime reports.
    #[test]
    fn the_handler_reads_the_invocations_environment_and_context() {
        let mut environment = BTreeMap::new();
        environment.insert("SECRET".to_string(), "shhh".to_string());
        environment.insert("AWS_LAMBDA_FUNCTION_NAME".to_string(), "probe".to_string());
        environment.insert("AWS_LAMBDA_FUNCTION_VERSION".to_string(), "7".to_string());
        environment.insert(
            "AWS_LAMBDA_FUNCTION_MEMORY_SIZE".to_string(),
            "256".to_string(),
        );
        let finished = ran_with(
            &[(
                "index.js",
                "exports.handler = async (event, context) => ({\n\
                     secret: process.env.SECRET,\n\
                     host: process.env.PATH === undefined,\n\
                     name: context.functionName,\n\
                     version: context.functionVersion,\n\
                     memory: context.memoryLimitInMB,\n\
                     request: context.awsRequestId,\n\
                     remaining: context.getRemainingTimeInMillis() > 0,\n\
                 });\n",
            )],
            "index.handler",
            None,
            &environment,
            budget(),
        );
        let answered = returned(&finished, "the environment and the context");
        let expected = ::serde_json::json!({
            "secret": "shhh",
            "host": true,
            "name": "probe",
            "version": "7",
            "memory": "256",
            "request": "11111111-2222-3333-4444-555555555555",
            "remaining": true,
        });
        assert_eq!(answered, expected, "expected {expected}, got {answered}");
    }

    /// What the handler wrote to the console is the run's log, capped at the budget.
    #[test]
    fn the_console_writes_into_the_invocations_log() {
        let finished = ran(
            "exports.handler = async () => { \
             console.log('handler says', { n: 1 }); \
             console.error('and warns'); \
             return 1; };",
            "index.handler",
            None,
        );
        for expected in ["handler says { n: 1 }", "and warns"] {
            assert!(
                finished.log.contains(expected),
                "expected {expected:?} in the log, got {:?}",
                finished.log
            );
        }
    }

    /// A handler that never stops is stopped by the deadline, and the stop is LATCHED: the
    /// run's answer is the stop whatever the guest catches.
    #[test]
    fn a_handler_that_never_stops_is_stopped_by_its_deadline() {
        let cases: &[(&str, &str)] = &[
            (
                "a loop with no exit",
                "exports.handler = async () => { for (;;) {} };",
            ),
            (
                "a loop that catches what the engine raises",
                "exports.handler = async () => { try { for (;;) {} } catch (error) { \
                 return { escaped: true }; } };",
            ),
        ];
        for (case, source) in cases {
            let mut tight = budget();
            tight.wall_clock = Duration::from_millis(500);
            let started = Instant::now();
            let finished = ran_with(&[("index.js", source)], "index.handler", None, &BTreeMap::new(), tight);
            assert!(
                matches!(finished.completion, Completion::Stopped(Stop::Deadline)),
                "case {case}: expected the deadline stop, got {:?}",
                finished.completion
            );
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "case {case}: the stop must land inside the caller's wait, took {:?}",
                started.elapsed()
            );
        }
    }

    /// A blocking wait the guest asks for is the HOST's to make, and never outlasts the
    /// run: a timer far in the future does not hold the caller open.
    #[test]
    fn a_timer_past_the_deadline_does_not_outlast_the_run() {
        let mut tight = budget();
        tight.wall_clock = Duration::from_millis(500);
        let started = Instant::now();
        let finished = ran_with(
            &[(
                "index.js",
                "exports.handler = function (event, context, callback) { \
                 setTimeout(function () { callback(null, { late: true }); }, 600000); };",
            )],
            "index.handler",
            None,
            &BTreeMap::new(),
            tight,
        );
        assert!(
            matches!(finished.completion, Completion::Stopped(Stop::Deadline)),
            "expected the deadline stop, got {:?}",
            finished.completion
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "expected the run to end at its deadline, took {:?}",
            started.elapsed()
        );
    }

    /// A response larger than one may be is stopped rather than answered.
    #[test]
    fn a_response_past_the_cap_is_stopped() {
        let mut tight = budget();
        tight.response_bytes = 1_024;
        let finished = ran_with(
            &[(
                "index.js",
                "exports.handler = async () => 'x'.repeat(4096);",
            )],
            "index.handler",
            None,
            &BTreeMap::new(),
            tight,
        );
        assert!(
            matches!(finished.completion, Completion::Stopped(Stop::Response)),
            "expected the response stop, got {:?}",
            finished.completion
        );
    }

    /// A run that reaches the memory ceiling the function's `MemorySize` sets is stopped
    /// by it: the ceiling is the engine's own allocation limit, so the run cannot grow
    /// past it.
    ///
    /// The run is judged against a clock that does not advance, so the ceiling is the only
    /// bound that can answer it: a host slow enough to spend the deadline while the engine
    /// is faulting those allocations in would otherwise answer the row with the deadline
    /// instead of with the ceiling the row is about.
    #[test]
    fn a_run_past_the_memory_ceiling_is_stopped() {
        let case = "a handler allocating past the engine's own ceiling";
        let finished = ran_against(
            &[(
                "index.js",
                "exports.handler = async () => { const held = []; \
                 for (;;) { held.push(new Array(100000).fill(7)); } };",
            )],
            "index.handler",
            None,
            &BTreeMap::new(),
            // The run's heap is the smallest `MemorySize` the model allows
            // ([`ran_against`]), so the ceiling is reached by allocating.
            budget(),
            &Clock::still(),
            case,
        );
        assert!(
            matches!(finished.completion, Completion::Stopped(Stop::Memory)),
            "case {case}: expected the memory stop, got {:?}",
            finished.completion
        );
    }

    /// Each bound the engine's interrupt handler spends is latched, and a spent budget is
    /// the run's answer however often it is asked again.
    #[test]
    fn a_spent_bound_is_latched() {
        let cases: &[(&str, Budget, Stop)] = &[
            (
                "a spent step ceiling",
                Budget {
                    wall_clock: Duration::from_secs(60),
                    ..budget()
                },
                Stop::Steps,
            ),
            (
                "a spent deadline",
                Budget {
                    wall_clock: Duration::ZERO,
                    ..budget()
                },
                Stop::Deadline,
            ),
        ];
        for (case, held, expected) in cases {
            let bounds = Bounds::new(held, Clock::host());
            if *expected == Stop::Steps {
                // Spend every check, so the next one is the one that stops.
                bounds.checks.store(0, Ordering::Relaxed);
            }
            bounds.start();
            let first = bounds.step();
            assert_eq!(
                first,
                Some(*expected),
                "case {case}: expected {expected:?}, got {first:?}"
            );
            for again in 0..4 {
                assert_eq!(
                    bounds.step(),
                    Some(*expected),
                    "case {case}: ask {again} must still report the latched {expected:?}"
                );
            }
            assert_eq!(
                bounds.stopped(),
                Some(*expected),
                "case {case}: the latched stop is the run's answer"
            );
        }
    }

    /// A run whose bounds are not spent carries on: the step hook is not a stop in itself.
    #[test]
    fn an_unspent_bound_lets_the_run_carry_on() {
        let bounds = Bounds::new(&budget(), Clock::host());
        bounds.start();
        for step in 0..8 {
            assert_eq!(
                bounds.step(),
                None,
                "step {step}: an unspent budget must let the run carry on"
            );
        }
        assert_eq!(
            bounds.stopped(),
            None,
            "an unspent budget latches no stop at all"
        );
    }

    /// The CORE MODULES a managed runtime supplies resolve and compute: one row per module
    /// this emulator serves, each exercised through the operation a handler reaches it for,
    /// under both the bare and the `node:` spelling of its name.
    #[test]
    fn a_handler_reaches_every_core_module_this_runtime_supplies() {
        let cases: &[(&str, &str, &str)] = &[
            ("path", "require('path').join('a', 'b', '../c')", r#""a/c""#),
            (
                "util",
                "require('node:util').format('%s=%d', 'n', 7)",
                r#""n=7""#,
            ),
            (
                "events",
                "(() => { const E = require('events'); const e = new E(); let seen = null; \
                 e.on('x', (v) => { seen = v; }); e.emit('x', 5); return seen; })()",
                "5",
            ),
            (
                "buffer",
                "require('buffer').Buffer.from('hi', 'utf8').toString('base64')",
                r#""aGk=""#,
            ),
            (
                "crypto",
                "require('node:crypto').createHash('sha256').update('abc').digest('hex')",
                r#""ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad""#,
            ),
            (
                "fs",
                "(() => { const fs = require('fs'); fs.writeFileSync('/tmp/x.txt', 'kept'); \
                 return fs.readFileSync('/tmp/x.txt', 'utf8'); })()",
                r#""kept""#,
            ),
            (
                "fs/promises",
                "typeof require('fs/promises').readFile",
                r#""function""#,
            ),
            ("timers", "typeof require('timers').setTimeout", r#""function""#),
            (
                "stream",
                "(() => { const { PassThrough } = require('stream'); \
                 return typeof new PassThrough().write; })()",
                r#""function""#,
            ),
            (
                "string_decoder",
                "(() => { const { StringDecoder } = require('string_decoder'); \
                 const d = new StringDecoder('utf8'); \
                 return d.write(Buffer.from([0xe2, 0x82])) + d.end(Buffer.from([0xac])); })()",
                r#""€""#,
            ),
            ("os", "require('node:os').tmpdir()", r#""/tmp""#),
            (
                "assert",
                "(() => { try { require('assert').strictEqual(1, 2); return 'no'; } \
                 catch (error) { return error.code; } })()",
                r#""ERR_ASSERTION""#,
            ),
            (
                "querystring",
                "require('querystring').stringify({ a: 1, b: 'x y' })",
                r#""a=1&b=x+y""#,
            ),
            (
                "url",
                "require('url').fileURLToPath('file:///var/task/index.mjs')",
                r#""/var/task/index.mjs""#,
            ),
        ];
        for (case, expression, expected) in cases {
            let source =
                format!("exports.handler = async () => ({{ value: {expression} }});");
            let finished = ran(&source, "index.handler", None);
            let actual = returned(&finished, case);
            let wanted: ::serde_json::Value = ::serde_json::from_str(&format!(
                "{{\"value\":{expected}}}"
            ))
            .expect("the expected JSON");
            assert_eq!(
                actual, wanted,
                "case {case}: expected {wanted}, got {actual}"
            );
        }
    }

    /// `timers/promises` really waits and then settles, which is what a handler awaiting it
    /// depends on — the queue is the host's, so this is the whole round trip.
    #[test]
    fn a_handler_awaits_the_promise_form_of_the_timer_queue() {
        let finished = ran(
            "const { setTimeout: wait } = require('timers/promises');\n\
             exports.handler = async () => ({ waited: await wait(5, 'done') });",
            "index.handler",
            None,
        );
        let actual = returned(&finished, "timers/promises");
        assert_eq!(
            actual,
            ::serde_json::json!({"waited": "done"}),
            "expected the awaited value back, got {actual}"
        );
    }

    /// The GLOBALS a managed runtime defines, each read the way a handler reads it.
    #[test]
    fn the_globals_a_managed_runtime_defines_are_defined() {
        let cases: &[(&str, &str, &str)] = &[
            ("Buffer", "typeof Buffer", r#""function""#),
            (
                "a Buffer round trip",
                "Buffer.from(Buffer.from('ünïcødé').toString('base64'), 'base64')\
                 .toString('utf8')",
                r#""ünïcødé""#,
            ),
            ("process.versions.node", "process.versions.node", r#""22.0.0""#),
            ("process.version", "process.version", r#""v22.0.0""#),
            ("process.platform", "typeof process.platform", r#""string""#),
            ("process.arch", "typeof process.arch", r#""string""#),
            ("process.cwd", "process.cwd()", r#""/var/task""#),
            ("URL", "new URL('https://h/p?q=1').pathname", r#""/p""#),
            (
                "URLSearchParams",
                "new URLSearchParams('a=1&a=2').getAll('a').join(',')",
                r#""1,2""#,
            ),
            ("TextEncoder", "new TextEncoder().encode('ab').length", "2"),
            (
                "TextDecoder",
                "new TextDecoder().decode(new Uint8Array([104, 105]))",
                r#""hi""#,
            ),
            ("btoa", "btoa('hi')", r#""aGk=""#),
            ("Error.stackTraceLimit", "Error.stackTraceLimit", "10"),
            (
                "crypto.randomUUID",
                "globalThis.crypto.randomUUID().length",
                "36",
            ),
            (
                "AbortController",
                "(() => { const c = new AbortController(); c.abort(); \
                 return c.signal.aborted; })()",
                "true",
            ),
        ];
        for (case, expression, expected) in cases {
            let source =
                format!("exports.handler = async () => ({{ value: {expression} }});");
            let finished = ran(&source, "index.handler", None);
            let actual = returned(&finished, case);
            let wanted: ::serde_json::Value =
                ::serde_json::from_str(&format!("{{\"value\":{expected}}}"))
                    .expect("the expected JSON");
            assert_eq!(
                actual, wanted,
                "case {case}: expected {wanted}, got {actual}"
            );
        }
    }

    /// An ES module knows its own url: `import.meta.url` is the `file:` URL of the module
    /// that is running, one row per place in the package a module can live.
    #[test]
    fn an_es_module_reports_its_own_url() {
        let cases: &[(&str, &str, &str)] = &[
            ("a module at the root", "index.mjs", "index.handler"),
            (
                "a module in a nested directory",
                "src/handlers/api.mjs",
                "src/handlers/api.handler",
            ),
        ];
        for (case, path, handler) in cases {
            let finished = ran_package(
                &[(
                    path,
                    "export const handler = async () => ({ file: import.meta.url });",
                )],
                handler,
                None,
            );
            let actual = returned(&finished, case);
            assert_eq!(
                actual,
                ::serde_json::json!({"file": format!("file:///var/task/{path}")}),
                "case {case}: expected the module's own file url, got {actual}"
            );
        }
    }

    /// The legacy completion API a node context carries: each of the three ends the
    /// invocation with what it was given, so a handler written against it answers.
    #[test]
    fn the_legacy_context_completion_api_answers_the_invocation() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "context.succeed",
                "exports.handler = function (event, context) { context.succeed({ via: 'succeed' }); };",
                r#"{"via":"succeed"}"#,
            ),
            (
                "context.done with a result",
                "exports.handler = function (event, context) { context.done(null, { via: 'done' }); };",
                r#"{"via":"done"}"#,
            ),
            (
                "the callback with a result",
                "exports.handler = function (event, context, callback) { callback(null, { via: 'callback' }); };",
                r#"{"via":"callback"}"#,
            ),
        ];
        for (case, source, expected) in cases {
            let finished = ran(source, "index.handler", None);
            let actual = returned(&finished, case);
            let wanted: ::serde_json::Value =
                ::serde_json::from_str(expected).expect("the expected JSON");
            assert_eq!(
                actual, wanted,
                "case {case}: expected {wanted}, got {actual}"
            );
        }
    }

    /// `context.fail` and the callback's error argument each report the error the handler
    /// gave them, with the trace head a node runtime reports.
    #[test]
    fn a_failing_completion_reports_the_error_it_was_given() {
        for (case, source) in [
            (
                "context.fail",
                "exports.handler = function (event, context) { \
                 context.fail(new RangeError('out of range')); };",
            ),
            (
                "the callback's error",
                "exports.handler = function (event, context, callback) { \
                 callback(new RangeError('out of range')); };",
            ),
        ] {
            let finished = ran(source, "index.handler", None);
            match &finished.completion {
                Completion::Raised {
                    error_type,
                    error_message,
                    stack_trace,
                } => {
                    assert_eq!(
                        (error_type.as_str(), error_message.as_str()),
                        ("RangeError", "out of range"),
                        "case {case}: expected the thrown error's class and message, got \
                         {error_type}/{error_message}"
                    );
                    assert_eq!(
                        stack_trace.first().map(String::as_str),
                        Some("RangeError: out of range"),
                        "case {case}: the trace's head line is the error's own line, got \
                         {stack_trace:?}"
                    );
                }
                other => panic!("case {case}: expected a raised error, got {other:?}"),
            }
        }
    }

    /// One row of the unfinished-invocation table: the case, the package's entries, the
    /// environment the invocation runs with, and the error type it is expected to report.
    type UnfinishedCase<'a> = (
        &'a str,
        &'a [(&'a str, &'a str)],
        &'a BTreeMap<String, String>,
        &'a str,
    );

    /// What a run that AWS fails is reported as, one row per way a node invocation fails
    /// without the handler raising: module syntax in a CommonJS file, and a handler whose
    /// promise never settles — which is the node PROCESS ending without an answer, and is
    /// the error class a managed node runtime reports for one.
    #[test]
    fn an_invocation_that_cannot_answer_reports_the_failure_it_is() {
        let cases: &[UnfinishedCase<'_>] = &[
            (
                "module syntax in a CommonJS .js file",
                &[("index.js", "export const handler = async () => 1;")],
                &BTreeMap::new(),
                "Runtime.UserCodeSyntaxError",
            ),
            (
                "a handler whose promise never settles",
                &[("index.js", "exports.handler = () => new Promise(() => {});")],
                &BTreeMap::new(),
                "Runtime.NodeJsExit",
            ),
        ];
        for (case, entries, environment, expected) in cases {
            let finished = ran_with(entries, "index.handler", None, environment, budget());
            match &finished.completion {
                Completion::Raised { error_type, .. } => assert_eq!(
                    error_type, expected,
                    "case {case}: expected the failure to be reported as {expected}, got \
                     {error_type}"
                ),
                other => panic!("case {case}: expected a reported failure, got {other:?}"),
            }
        }
    }

    /// `NODE_OPTIONS` is applied: a module it names is preloaded before the handler's own
    /// module, the stack depth it sets is the one an error carries, and a flag the runtime
    /// does not know makes the invocation report the runtime's exit.
    #[test]
    fn node_options_is_applied_before_the_handler_is_loaded() {
        let entries: &[(&str, &str)] = &[
            ("preload.js", "globalThis.preloaded = 'yes';"),
            (
                "index.js",
                "exports.handler = async () => ({ preloaded: globalThis.preloaded || null, \
                 stackTraceLimit: Error.stackTraceLimit });",
            ),
        ];
        let mut applied = BTreeMap::new();
        applied.insert(
            "NODE_OPTIONS".to_string(),
            "--require /var/task/preload.js --stack-trace-limit=3".to_string(),
        );
        let finished = ran_with(entries, "index.handler", None, &applied, budget());
        let actual = returned(&finished, "NODE_OPTIONS applied");
        assert_eq!(
            actual,
            ::serde_json::json!({"preloaded": "yes", "stackTraceLimit": 3}),
            "expected the preloaded module and the stack depth the options set, got {actual}"
        );

        let mut refused = BTreeMap::new();
        refused.insert("NODE_OPTIONS".to_string(), "--not-a-node-flag".to_string());
        let finished = ran_with(entries, "index.handler", None, &refused, budget());
        match &finished.completion {
            Completion::Raised {
                error_type,
                error_message,
                ..
            } => assert_eq!(
                (error_type.as_str(), error_message.as_str()),
                ("Runtime.NodeJsExit", "node: bad option: --not-a-node-flag"),
                "an option the runtime does not know ends the run before the handler, got \
                 {error_type}/{error_message}"
            ),
            other => panic!("expected the runtime's exit, got {other:?}"),
        }
    }

    /// `Intl` and the local clock a run reads: the zone `TZ` names decides every local
    /// reading, one row per class of zone a function can be configured with (a fixed zone,
    /// one with daylight saving, one named as an offset) at a fixed instant.
    #[test]
    fn the_local_clock_is_the_zone_the_environment_names() {
        // 2021-07-01T12:00:00Z, chosen inside northern summer so a zone with daylight
        // saving reads differently from the same zone in winter.
        let instant = 1_625_140_800_000_i64;
        let cases: &[(&str, i64, &str)] = &[
            ("UTC", 0, "12"),
            // IANA: America/New_York is UTC-4 while daylight saving is in force.
            ("America/New_York", -240, "8"),
            // IANA: Asia/Kolkata is UTC+5:30 all year.
            ("Asia/Kolkata", 330, "17"),
            // IANA: Australia/Sydney is UTC+10 outside its own daylight saving.
            ("Australia/Sydney", 600, "22"),
        ];
        for (zone, offset_minutes, hour) in cases {
            let mut environment = BTreeMap::new();
            environment.insert("TZ".to_string(), (*zone).to_string());
            let source = format!(
                "exports.handler = async () => {{ const at = new Date({instant}); return {{ \
                 hour: String(at.getHours()), offset: -at.getTimezoneOffset(), zone: \
                 Intl.DateTimeFormat().resolvedOptions().timeZone }}; }};"
            );
            let finished = ran_with(
                &[("index.js", source.as_str())],
                "index.handler",
                None,
                &environment,
                budget(),
            );
            let actual = returned(&finished, zone);
            assert_eq!(
                actual,
                ::serde_json::json!({
                    "hour": hour,
                    "offset": offset_minutes,
                    "zone": zone,
                }),
                "case {zone:?}: expected the local reading of that zone, got {actual}"
            );
        }
    }

    /// A zone no database holds is not a zone: `Intl` reports it as invalid rather than
    /// formatting in another one.
    #[test]
    fn a_zone_the_database_does_not_hold_is_refused() {
        let finished = ran(
            "exports.handler = async () => { try { \
             new Intl.DateTimeFormat('en-US', { timeZone: 'Mars/Olympus_Mons' }); \
             return { refused: null }; } catch (error) { \
             return { refused: error.name + ': ' + error.message }; } };",
            "index.handler",
            None,
        );
        let actual = returned(&finished, "an unknown zone");
        assert_eq!(
            actual,
            ::serde_json::json!({
                "refused": "RangeError: Invalid time zone specified: Mars/Olympus_Mons"
            }),
            "expected the zone to be refused as invalid, got {actual}"
        );
    }

    /// A run that resolves MANY distinct runtime-supplied specifiers answers inside its own
    /// budget, and nothing a specifier names is held past it: the names a client package
    /// exports are read from one cell per service this emulator can call
    /// (`node_sdk_guest::exports`), so a handler naming tokens no service answers to grows
    /// nothing outside the engine's own ceiling.
    #[test]
    fn a_run_that_names_many_runtime_specifiers_answers_inside_its_budget() {
        let finished = ran(
            "exports.handler = async () => {\n\
             let bound = 0;\n\
             for (let at = 0; at < 100; at += 1) {\n\
                 const held = require('@aws-sdk/client-absentservice' + at);\n\
                 bound += typeof held['AbsentService' + at + 'Client'] === 'function' ? 1 : 0;\n\
             }\n\
             return { bound };\n\
             };",
            "index.handler",
            None,
        );
        let actual = returned(&finished, "a hundred distinct client packages");
        assert_eq!(
            actual,
            ::serde_json::json!({ "bound": 100 }),
            "expected every specifier to resolve inside the run's budget, got {actual}"
        );
    }

    /// The AWS SDK packages this runtime BUNDLES are importable as ES modules by the names
    /// they export, with nothing in the deployment package — one row per package, each
    /// binding what that package exports and reading its own manifest subpath.
    #[test]
    fn an_es_module_imports_the_bundled_sdk_packages_by_name() {
        let cases: &[(&str, &str, ::serde_json::Value)] = &[
            (
                "a service client, its commands and its error classes",
                "import { SQSClient, GetQueueUrlCommand, QueueDoesNotExist, \
                 SQSServiceException } from '@aws-sdk/client-sqs';\n\
                 import manifest from '@aws-sdk/client-sqs/package.json' with { type: 'json' \
                 };\n\
                 export const handler = async () => ({\n\
                 client: SQSClient.name,\n\
                 command: new GetQueueUrlCommand({}).operation,\n\
                 error: new QueueDoesNotExist({ message: 'gone' }).name,\n\
                 modelled: new QueueDoesNotExist({}) instanceof SQSServiceException,\n\
                 package: manifest.name,\n\
                 });\n",
                ::serde_json::json!({
                    "client": "SQSClient",
                    "command": "GetQueueUrl",
                    "error": "QueueDoesNotExist",
                    "modelled": true,
                    "package": "@aws-sdk/client-sqs",
                }),
            ),
            (
                "another service's client package, with its own commands and error classes",
                "import { S3Client, PutObjectCommand, NoSuchKey } from '@aws-sdk/client-s3';\n\
                 export const handler = async () => ({\n\
                 client: S3Client.name,\n\
                 command: new PutObjectCommand({}).operation,\n\
                 error: new NoSuchKey({}).name,\n\
                 });\n",
                ::serde_json::json!({
                    "client": "S3Client",
                    "command": "PutObject",
                    "error": "NoSuchKey",
                }),
            ),
            (
                "a client package for a service this emulator makes no call to",
                "import { KinesisClient } from '@aws-sdk/client-kinesis';\n\
                 export const handler = async () => ({ client: KinesisClient.name });\n",
                ::serde_json::json!({ "client": "KinesisClient" }),
            ),
            (
                "the document client and its commands",
                "import { DynamoDBDocumentClient, PutCommand } from '@aws-sdk/lib-dynamodb';\n\
                 export const handler = async () => ({\n\
                 from: typeof DynamoDBDocumentClient.from,\n\
                 command: new PutCommand({}).operation,\n\
                 });\n",
                ::serde_json::json!({ "from": "function", "command": "PutItem" }),
            ),
            (
                "the value conversions",
                "import { marshall, unmarshall } from '@aws-sdk/util-dynamodb';\n\
                 export const handler = async () => ({\n\
                 round: unmarshall(marshall({ n: 1, s: 'x' })),\n\
                 });\n",
                ::serde_json::json!({ "round": { "n": 1, "s": "x" } }),
            ),
        ];
        for (case, source, expected) in cases {
            let finished = ran_package(&[("index.mjs", source)], "index.handler", None);
            let actual = returned(&finished, case);
            assert_eq!(
                &actual, expected,
                "case {case:?}: expected {expected}, got {actual}"
            );
        }
    }

    /// A specifier this runtime cannot resolve fails the way the node runtime reports a
    /// module it cannot find — the class of the INITIALIZATION phase, naming the specifier —
    /// whether the handler's module reaches it with an `import` or with a `require`, which is
    /// the one resolution.
    #[test]
    fn a_specifier_that_resolves_to_nothing_is_a_module_not_found() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "an ES module importing a scoped package no runtime supplies",
                "import held from '@aws-sdk/lib-absent';\n\
                 export const handler = async () => held;\n",
                "@aws-sdk/lib-absent",
            ),
            (
                "an ES module importing a bare package the package does not hold",
                "import held from 'left-pad';\nexport const handler = async () => held;\n",
                "left-pad",
            ),
            (
                "an ES module importing a relative module the package does not hold",
                "import held from './absent.mjs';\nexport const handler = async () => held;\n",
                "./absent.mjs",
            ),
            (
                "an ES module importing a core module this runtime does not supply",
                "import held from 'node:absent';\nexport const handler = async () => held;\n",
                "node:absent",
            ),
        ];
        for (case, source, specifier) in cases {
            let finished = ran_package(&[("index.mjs", source)], "index.handler", None);
            match &finished.completion {
                Completion::Raised {
                    error_type,
                    error_message,
                    ..
                } => {
                    assert_eq!(
                        error_type, "Runtime.ImportModuleError",
                        "case {case:?}: expected the class of a module that would not import, \
                         got {error_type} ({error_message})"
                    );
                    assert_eq!(
                        error_message,
                        &format!("Error: Cannot find module '{specifier}'"),
                        "case {case:?}: expected the specifier to be named as the module that \
                         was not found, got {error_message}"
                    );
                }
                other => panic!("case {case:?}: expected a raised error, got {other:?}"),
            }
        }
    }

    /// The MAJOR version and the release a `Runtime` identifier names — one row per form
    /// the identifier takes in the model's own value set (`smithy_query(query="shape",
    /// shape="Runtime")`).
    #[test]
    fn a_runtime_identifier_names_the_release_the_run_reports() {
        let cases: &[(&str, &str, &str)] = &[
            ("nodejs22.x", "22", "22.0.0"),
            ("nodejs20.x", "20", "20.0.0"),
            ("nodejs18.x", "18", "18.0.0"),
            ("nodejs8.10", "8", "8.10.0"),
            ("nodejs4.3-edge", "4", "4.3.0"),
            ("nodejs", "", "0.0.0"),
        ];
        for (runtime, major, version) in cases {
            let actual = (node_major(runtime), node_version(runtime));
            assert_eq!(
                (actual.0.as_str(), actual.1.as_str()),
                (*major, *version),
                "case {runtime:?}: expected major {major:?} and version {version:?}, got \
                 {actual:?}"
            );
        }
    }
}
