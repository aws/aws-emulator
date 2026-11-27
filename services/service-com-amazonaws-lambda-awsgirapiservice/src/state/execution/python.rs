// SPDX-License-Identifier: CC0-1.0

//! The PYTHON interpreter one invocation runs in, and everything that bounds and contains
//! it.
//!
//! A `python3.x` function's answer is whatever its handler returned, so the handler is
//! RUN: the deployment package's modules are compiled and executed by the interpreter this
//! module builds, in this process, and the value the handler returned is serialized by that
//! same interpreter. Nothing about a run is synthesised — a payload, a log line, an error
//! type or a duration here is the one the guest produced.
//!
//! # Containment
//!
//! The guest reaches exactly what this module registers, and nothing else:
//!
//! * the interpreter is built with the engine crate's host-reaching surface compiled OUT
//!   (`default-features = false`, see this crate's `Cargo.toml`), so no `ctypes` host
//!   loader, no host standard streams and no interpreter thread exist in the build;
//! * the native modules are an ALLOW-LIST ([`ALLOWED_NATIVE_MODULES`]) filtered out of the
//!   engine's own pool, so a module that reaches a socket, a subprocess, a memory map or a
//!   host library is never registered — it is therefore unreachable through `import`,
//!   `sys.modules`, `importlib` or `__import__`, however the guest rebinds or deletes
//!   names;
//! * the modules a Python program expects of its operating system are this module's OWN
//!   ([`host_posix_module`], [`host_thread_module`]): the environment is the invocation's
//!   own variables held in memory, the working directory is the task root the runtime
//!   reports, randomness comes from the host's own generator, and every call that would
//!   reach a host file, process or descriptor raises;
//! * `socket` is one of those: no socket EXTENSION is registered — `_socket` and `_ssl` stay
//!   off the allow-list above, so the standard library's own `socket` has nothing to be built
//!   on and no guest code holds a host socket or a host descriptor. What this runtime supplies
//!   on the import path instead ([`super::guest_http`], with the sockets of
//!   [`super::xray_daemon`] and the connections of [`super::guest_socket`]) reaches THIS
//!   emulator and nothing else: a datagram's one destination is the in-process X-Ray daemon,
//!   whose datagrams travel over the SAME bounded AWS SDK seam below, and a stream socket's
//!   one destination is an address this emulator itself opened a resource's data-plane
//!   listener at — every other destination raises the error a host answers for it, and no name
//!   but this machine's own resolves;
//! * the only host functions the guest can call are the ones on [`HOST_MODULE`]: the
//!   invocation log, a sleep clamped to what is left of the run, ONE AWS SDK call into this
//!   same emulator ([`super::aws_api`], which decides which services are reachable, signs the
//!   request itself and bounds it by what the run has left), the authentication token an
//!   SDK mints in its client, the one request a handler sends itself, the one URL it has
//!   signed, what a called operation's own model says about paging, the one call a handler
//!   makes on a stream socket it opened ([`super::guest_socket`], which decides which
//!   addresses are dialable and bounds every wait by what the run has left), ONE
//!   checkpoint of the durable execution this run is the run of
//!   ([`super::super::durable_steps`]), and the SOURCE of one module of the standard library
//!   this runtime ships ([`super::python_library`], which is the text a frame inside that
//!   library carries the line of, and which a real runtime keeps as a readable file of its own
//!   library directory). No host path,
//!   host environment variable, host credential or
//!   other function's state is reachable from guest code, and NO bound of the run — no
//!   deadline, no step count and no memory baseline — is reachable from it either: where one
//!   PHASE of a run ends and the next begins is decided by the HOST, between two calls it
//!   makes into the runtime ([`drive`]), rather than by anything the guest can reach.
//!
//! # Bounds
//!
//! Every bound lives in host code the guest cannot switch off:
//!
//! * STEPS and the DEADLINE are checked by [`Bounds::step`], which the interpreter's trace
//!   hook calls DIRECTLY: the hook is a host function ([`install_trace_hook`]) rather than
//!   Python, so no module attribute, global or default argument stands between a traced event
//!   and the bound, and the host then REMOVES `sys.settrace` / `sys.setprofile` /
//!   `sys.gettrace`, so no guest call can clear or replace it either. The stop is LATCHED:
//!   once the budget is spent the run's answer is that stop, whatever the guest goes on to
//!   return or catch.
//!   Every deadline of a run — the one the hook spends and the one the host's own watch
//!   samples — is read against the CLOCK the host hands the run ([`run_against`]), which is
//!   this host's own for every invocation this emulator serves. A run can therefore be
//!   judged against a clock whose span the host's own speed cannot spend, which is what lets
//!   a run asked about one of the bounds below be answered by THAT bound rather than by its
//!   deadline on a host slow enough to spend it. Such a clock states the HOST time one run
//!   may take beside it ([`Clock::within`]), which [`Bounds::step`] and the watch below read
//!   from this host's own clock — so the host a run of caller-supplied code runs on is
//!   bounded whichever clock that run's deadline is read against.
//! * MEMORY is sampled from outside the guest — the faulted pages of the run's OWN thread,
//!   because the interpreter exposes no allocation seam — and a sample past the run's own
//!   ceiling ends the run. It is measured from what that thread held once the interpreter was
//!   standing and before a line of the run's own code had been read
//!   ([`Bounds::measure_memory_from_here`]), and the HOST takes that baseline: nothing the
//!   guest can call re-takes it, so the ceiling cannot be lifted by asking. The reading is
//!   this one run's and no other's: memory another run, another environment or the rest of
//!   this emulator holds is never charged to it. What the runs in flight hold BETWEEN them is
//!   bounded where they are admitted ([`super::MAX_CONCURRENT_RUN_MEMORY_MB`]), and what the
//!   environments standing idle hold is bounded by the pool that keeps them
//!   ([`super::warm_pool`]).
//! * A long NATIVE call emits no trace event, so the deadline cannot land inside one. That is
//!   what the host's own watch ([`watch_run`]) is for: ONE thread of this process reads the
//!   same clock for every live run and LATCHES the stop, which the next event the interpreter
//!   delivers then raises. Beside it, the caller's own host wait and abandoned-run accounting
//!   ([`super::run_invocation`]) make sure the CALLER is always answered inside its ceiling,
//!   and a run still going after its budget was spent is given up on.
//! * RECURSION is clamped on the interpreter before any guest code runs, and
//!   `sys.setrecursionlimit` is removed so the guest cannot raise it.
//! * The LOG and the RESPONSE are each capped, so a small request cannot make an
//!   unbounded answer.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use rustpython_vm::function::{FuncArgs, OptionalArg};
use rustpython_vm::builtins::PyModule;
use rustpython_vm::{AsObject, Interpreter, PyObjectRef, PyRef, PyResult, VirtualMachine};

// The half of an execution environment's build that no function's configuration is read for,
// which is why it can stand before the invocation that is served in it arrives.
mod blank;

pub(super) use blank::Blank;

// What one run may spend and what it came to are the same for every engine, so they are
// the shared ones ([`super::outcome`]) rather than this interpreter's own.
pub use super::outcome::{Budget, Completion, Finished, Stop};

// So is the clock a run's time-based bounds are read against ([`super::run_clock`]): WHICH
// clock a run is judged by is a property of the run ([`run_against`]) rather than a call to
// the host buried inside each check.
pub use super::run_clock::Clock;

// Where a run of a DURABLE function reports the steps it took, so the execution it is the
// run of carries them in its history ([`super::super::durable_steps`]).
use super::super::durable_steps::DurableStepJournal;
use super::aws_api::{AwsAccess, CallerIdentity, Refusal, MAX_CALLS_PER_RUN};
use super::outcome::{captured, Capture};
use super::package::PackageModule;
use super::run_memory::{faulted_bytes, Held};

/// The name of the module the host functions live on, as the runtime source imports it.
const HOST_MODULE: &str = "_lambda_host";

/// The name of the module a SPARK program's statements are planned and run through, as the
/// Spark runtime this workspace supplies such a program imports it
/// ([`spark_host_module`]). It is that runtime's own name, read from where that runtime is
/// stated, so the module this interpreter registers and the module its source imports cannot
/// drift apart.
const SPARK_HOST_MODULE: &str = ::service_common::spark_runtime::HOST_MODULE;

/// How many CHILD PROCESSES one invocation may start.
///
/// A child is a program of the execution environment performed in this process
/// ([`super::guest_child_process`], whose own bounds hold what one of them costs), so this is
/// the bound on how many of them one handler may ask for — far above the handful a handler
/// that runs a tool of its layer starts, and far below what would cost this host anything
/// noticeable. It is held with the other per-invocation budgets, where no guest code can reach
/// it, and every invocation an environment serves has the whole of it.
const MAX_CHILDREN_PER_RUN: usize = 256;

/// The prefix a DATA entry of the deployment package is keyed under among the entries the
/// run is handed ([`super::package::python_resources_within`]).
///
/// A package's data file is not a module — nothing imports it — so it is keyed where no
/// module name can resolve to it: the runtime's own finder resolves a module name to
/// `<name>.py` or `<name>/__init__.py` ([`super::bootstrap`]), and no module name begins
/// with this. What reads these entries is the resource reading of the standard library, which
/// the runtime's loader answers from the same held entries.
pub const RESOURCE_ID_PREFIX: &str = "resource:";

/// The prefix an entry of one of the function's LAYERS is keyed under among the entries the
/// run is handed.
///
/// A layer's content is mounted at the LAYER ROOT rather than at the task root, so its
/// entries are keyed apart from the deployment package's and resolved from their own mount
/// ([`super::bootstrap`]): what follows this prefix is the entry's own path inside the layer
/// archive, which is its path under the layer root. A DATA entry of a layer carries
/// [`RESOURCE_ID_PREFIX`] after this one, exactly as a data entry of the deployment package
/// carries it, so no module name resolves to one.
pub const LAYER_ID_PREFIX: &str = "layer:";

/// The prefix an entry whose bytes are NOT TEXT is keyed under among the entries the run is
/// handed ([`super::package::python_binaries_within`]), carrying the base64 of those bytes.
///
/// A mount holds such an entry as the BYTES it is, decoded where the mounts are built
/// ([`super::bootstrap`]), so a handler and a program it starts read the file the archive
/// really carried. Nothing imports one: no module name begins with this, exactly as none
/// begins with [`RESOURCE_ID_PREFIX`].
pub const BINARY_ID_PREFIX: &str = "binary:";

/// Every native module this interpreter registers, out of the engine's own two pools.
///
/// This list IS the containment of the native surface: a name absent from it is dropped
/// from the interpreter's module table before any guest code runs, so no guest-side import
/// of it can succeed — not through `import`, not through `importlib`, not through
/// `__import__`, and not by dropping the module from `sys.modules` and importing it again
/// ([`prepare`] takes the builtin-module creation calls away too).
///
/// What is here is the interpreter's own machinery (the import system, the exception and
/// object model support, the bytecode and codec support) and the arithmetic, text,
/// hashing and data-structure modules the frozen standard library needs for a handler to
/// `import json`, `datetime`, `decimal`, `hashlib`, `base64`, `random`, `re` or `uuid` —
/// none of which reaches outside this process.
///
/// Deliberately absent, so that each stays unregistered: `_socket`, `_ssl`, `select`,
/// `_posixsubprocess`, `mmap`, `_ctypes`, `fcntl`, `termios`, `resource`, `pwd`, `grp`,
/// `syslog`, `_signal`, `_interpreters`, `_interpchannels`, `_interpqueues`,
/// `_testinternalcapi`, and every other module whose calls land on the host. `posix` and
/// `_thread` are absent for the same reason, and this module supplies its OWN of each
/// instead (see [`host_posix_module`], [`host_thread_module`]). Where one of those names a
/// module of the standard library a handler imports (`socket`, `ssl`, `select`, `signal`,
/// `subprocess`), the module ITSELF is supplied on the import path in Python
/// ([`runtime_stdlib`]), so the import succeeds and only the call needing the absent
/// capability is refused.
///
/// Two of the names here are registered but PATCHED before guest code runs, because the
/// frozen standard library needs the rest of each: `_io`, whose host file-opening calls
/// are replaced by refusals, and `time`, whose `sleep` is replaced by the clamped one. See
/// [`prepare`].
const ALLOWED_NATIVE_MODULES: &[&str] = &[
    // The interpreter's own machinery, which nothing runs without.
    "_abc",
    "_ast",
    "_codecs",
    "_collections",
    "_functools",
    "_imp",
    "_io",
    "_operator",
    // The opcode table `dis` and `inspect` read (`traceback` imports both). Pure data.
    "_opcode",
    "_sre",
    "_stat",
    "_string",
    // The tokenizer the standard library's own source tools use (`tokenize`, which
    // `traceback` imports). Pure text processing.
    "_tokenize",
    "_types",
    "_typing",
    "_warnings",
    "_weakref",
    "atexit",
    "errno",
    "gc",
    "itertools",
    "marshal",
    "time",
    // Arithmetic, text, hashing and data structures: computation, and nothing else.
    "_bisect",
    "_blake2",
    "_contextvars",
    "_csv",
    "_datetime",
    "_hashlib",
    "_heapq",
    "_json",
    "_md5",
    "_queue",
    "_random",
    "_sha1",
    "_sha2",
    "_sha256",
    "_sha3",
    "_sha512",
    "_statistics",
    "_struct",
    "array",
    "binascii",
    "cmath",
    "math",
    "unicodedata",
    "zlib",
];

/// The modules this emulator SUPPLIES in place of ones it does not register: the host
/// functions the runtime source calls.
const SUPPLIED_MODULES: &[&str] = &[HOST_MODULE, SPARK_HOST_MODULE, "posix"];

/// One invocation to run: the package, the handler, and everything the runtime reports to
/// it.
#[derive(Debug)]
pub struct Run<'a> {
    /// The deployment package's modules.
    pub modules: &'a [PackageModule],
    /// The `Handler` configured for the function (`module.attribute`).
    pub handler: &'a str,
    /// The environment the invocation runs with: the function's own variables plus the
    /// ones the runtime sets.
    pub environment: &'a BTreeMap<String, String>,
    /// The `Architectures` entry the function is configured for, which is the instruction
    /// set its execution environment has — what the run reports as its own machine
    /// ([`super::host_posix::install_environment_identity`]).
    pub architecture: &'a str,
    /// The event payload, as the caller sent it.
    pub event: Option<&'a str>,
    /// The invocation's request id.
    pub request_id: &'a str,
    /// The ARN the invocation addressed, as `context.invoked_function_arn`.
    pub invoked_function_arn: &'a str,
    /// The `ClientContext` the caller passed, already decoded to its JSON text.
    pub client_context: Option<&'a str>,
    /// The `TenantId` the caller passed.
    pub tenant_id: Option<&'a str>,
    /// The Amazon Cognito identity the invocation was made BY
    /// ([`super::super::caller_cognito_identity`]), and `None` for one made by a credential no
    /// identity pool vended — whose handler reads a `context.identity` carrying no identity.
    pub cognito_identity: Option<&'a crate::state::InvocationIdentity>,
    /// What this run may spend.
    pub budget: Budget,
    /// How the handler reaches the rest of this emulator through the AWS SDK this runtime
    /// supplies it, or `None` for a run with no seam to dispatch a call on — whose
    /// `boto3` reports every call as one that was not made.
    pub aws: Option<&'a AwsAccess>,
    /// Where this run reports the STEPS of the durable execution it is the run of
    /// ([`super::super::durable_steps`]), and `None` for an invocation that is no durable
    /// execution's run — whose durable SDK then records no checkpoint, because there is no
    /// execution to record one on.
    pub durable: Option<&'a DurableStepJournal>,
    /// The FILE SYSTEMS this run's execution environment mounts: each Amazon EFS access point
    /// the function's `FileSystemConfigs` name, resolved to the files behind it
    /// ([`super::super::file_system_mounts`]). Empty for a function that configures none,
    /// which is an environment with no mount of its own.
    pub file_systems: &'a [crate::state::FileSystemMount],
}

/// Run `request`'s handler to completion.
///
/// Called on a thread of its own with no state guard of this service held, because the
/// interpreter is synchronous and CPU-bound.
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
    let mut finished = match Prepared::start(request, clock) {
        Ok(prepared) => prepared.serve_once(request, clock),
        Err(finished) => *finished,
    };
    // The environment was built for this one invocation and torn down after it, so what the
    // caller waited for is both.
    finished.elapsed = started.elapsed();
    finished
}

/// What a run this emulator could not CONTAIN is refused with: the one sentence both ways of
/// building an environment state it with ([`Prepared::start`], [`Blank::stand_up`]), because
/// it is the same refusal — an interpreter whose native surface is not the allow-list is
/// never handed guest code.
const UNCONTAINED_INTERPRETER: &str =
    "the interpreter's native modules could not be narrowed to the allow-list";

/// An execution ENVIRONMENT of one function version: the interpreter standing with that
/// version's handler imported, ready to serve an invocation.
///
/// Building one is the INIT phase — the interpreter, the host surface, this emulator's
/// runtime and the import of the handler's own module — and none of it depends on the
/// invocation that is served in it, which is why an environment serves invocation after
/// invocation ([`super::warm_pool`]) rather than being built for each. What one invocation
/// brings is its own event, request, log, bounds and SDK seam, and those are bound onto the
/// environment per invocation ([`Session`], `__lambda_bind`).
///
/// It is NOT `Send`: every object here belongs to the one interpreter and the one thread it
/// was built on, so an environment is reached by handing an invocation to that thread.
pub(super) struct Prepared {
    /// The interpreter itself, finalized by [`Self::finalize`] and not before.
    interpreter: Interpreter,
    /// The invocation this environment is serving, which the host functions and the trace
    /// hook read.
    session: Arc<Session>,
    /// Where the installed trace hook is held, for the life of the environment.
    held: TraceCell,
    /// The entry points this environment is driven through ([`super::bootstrap`]), and the
    /// two mappings its invocations have their own variables written into
    /// ([`EnvironmentSinks`]).
    entries: Entries,
    /// The log and the bounds the INITIALIZATION was spent against, until the invocation it
    /// was built for has been served.
    ///
    /// That invocation keeps them: the module-level code of its own deployment package ran in
    /// the initialization, so what that code wrote belongs in its log and what it allocated is
    /// measured from the baseline the host took before the import ([`initialize`]). Every
    /// later invocation brings its own, because nothing of the environment it is served in is
    /// its doing.
    initializing: ::std::cell::RefCell<Option<Spending>>,
    /// What this thread had faulted in when the environment was BUILT, which is what the
    /// environment's own growth is measured against ([`Self::grown_bytes`]).
    faulted_at_build: u64,
}

/// What one invocation writes its log into and what it spends, which [`Session`] binds onto
/// the environment serving it.
type Spending = (Arc<Mutex<Capture>>, Arc<Bounds>);

impl Prepared {
    /// Build the environment and run the INIT phase in it, for `request` — the first
    /// invocation it will serve, whose own bounds the initialization is spent against.
    ///
    /// This is ONE pass over both halves of the build, which is what an invocation that finds
    /// no environment standing for it pays: its own log and bounds are the ones the whole
    /// build is spent against and the host watches it from the first instant, because all of
    /// it is inside that invocation's own wait. The half that reads no function's
    /// configuration can also be stood up BEFORE an invocation arrives, and is adopted by the
    /// first one that needs an environment ([`Blank`]) — the same two steps, with only this
    /// emulator's own work ahead of the invocation.
    ///
    /// `Err` carries what an initialization that failed is answered with: the error document
    /// the runtime wrote for it, or the internal failure of an interpreter that could not be
    /// stood up. No environment is kept in either case.
    pub(super) fn start(request: &Run<'_>, clock: &Clock) -> Result<Self, Box<Finished>> {
        let capture = Arc::new(Mutex::new(Capture::new(request.budget.log_bytes)));
        let bounds = Arc::new(Bounds::new(&request.budget, clock.clone()));
        let session = Session::new(
            &capture,
            &bounds,
            request.aws,
            request.durable,
            request.file_systems,
        );
        let input = match runtime_input(request, &session) {
            Ok(input) => input,
            Err(error) => {
                return Err(Box::new(settled(
                    Completion::Internal(error),
                    &bounds,
                    &capture,
                    Duration::ZERO,
                    true,
                )))
            }
        };
        let contained = Arc::new(AtomicBool::new(false));
        // From here until the environment stands, what is in flight is this emulator's own
        // work and none of the function's, so it is bounded by the span that work is allowed
        // rather than by either of the run's own spans.
        bounds.begin_provisioning();
        let interpreter = interpreter(&contained);
        // A run only happens when the native surface really was narrowed to the allow-list. If
        // it could not be, nothing is run at all: an uncontained interpreter is never handed
        // guest code.
        if !contained.load(Ordering::SeqCst) {
            return Err(Box::new(settled(
                Completion::Internal(UNCONTAINED_INTERPRETER.to_string()),
                &bounds,
                &capture,
                Duration::ZERO,
                true,
            )));
        }
        // The hook the bounds are spent through, held HERE: it stays installed and answering
        // until the interpreter has been finalized, because finalization runs the guest's own
        // `atexit` callbacks and those frames are traced like every other guest frame.
        let held: TraceCell = ::std::rc::Rc::new(::std::cell::RefCell::new(None));
        // The host's own watch over the initialization, sampling the clock from OUTSIDE the
        // guest for as long as it lasts ([`watch_run`]).
        watch_run(&bounds);
        let built = interpreter.enter(|vm| {
            let entries = match stand_up(vm, &session, &held) {
                Ok(entries) => entries,
                Err(exception) => return Err(formatted_exception(vm, &exception)),
            };
            let initialized = match initialize(vm, request, &input, &entries, &bounds) {
                Ok(initialized) => initialized,
                Err(exception) => {
                    Initialized::Answered(Completion::Internal(formatted_exception(vm, &exception)))
                }
            };
            Ok((entries, initialized))
        });
        let (entries, initialized) = match built {
            Ok(built) => built,
            // The environment itself could not be stood up, so there is nothing to keep: the
            // interpreter is finalized here, exactly as it is after a run.
            Err(why) => {
                interpreter.finalize(None);
                held.borrow_mut().take();
                return Err(Box::new(settled(
                    Completion::Internal(why),
                    &bounds,
                    &capture,
                    Duration::ZERO,
                    true,
                )));
            }
        };
        match initialized {
            Initialized::Ready => Ok(Self {
                interpreter,
                session,
                held,
                entries,
                initializing: ::std::cell::RefCell::new(Some((capture, bounds))),
                // Taken on the environment's OWN thread, which is the one every invocation
                // served in it runs on: what it has faulted in beyond this is what the
                // environment holds of the host.
                faulted_at_build: faulted_bytes(),
            }),
            Initialized::Answered(completion) => {
                // Nothing of a failed initialization is kept: the interpreter is finalized
                // here, exactly as it is after a run, and the objects it built go first.
                drop(entries);
                interpreter.finalize(None);
                held.borrow_mut().take();
                Err(Box::new(settled(
                    completion,
                    &bounds,
                    &capture,
                    Duration::ZERO,
                    true,
                )))
            }
        }
    }

    /// Serve ONE invocation in this environment: bind what the invocation brings, then run
    /// the handler's own phase.
    pub(super) fn invoke(&self, request: &Run<'_>, clock: &Clock) -> Finished {
        let ran = self.ran(request, clock);
        let finished = settled(
            ran.completion,
            &ran.bounds,
            &ran.capture,
            ran.elapsed,
            ran.initialized,
        );
        // The invocation is over, so the environment stops holding the seam it reached this
        // emulator through: an environment standing idle holds no caller's credential, and
        // the next invocation served in it brings its own.
        self.session.release_access();
        finished
    }

    /// Serve one invocation and tear the environment down, for a caller that keeps no
    /// environment standing.
    ///
    /// The teardown happens while the invocation's own bounds are still bound, so a guest
    /// `atexit` callback — which runs during the interpreter's finalization, after the handler
    /// returned — is bounded exactly as the handler was, and a stop it latches is what the
    /// invocation is answered with.
    pub(super) fn serve_once(self, request: &Run<'_>, clock: &Clock) -> Finished {
        let ran = self.ran(request, clock);
        let bounds = Arc::clone(&ran.bounds);
        let capture = Arc::clone(&ran.capture);
        let initialized = ran.initialized;
        let started = Instant::now();
        self.torn_down();
        settled(
            ran.completion,
            &bounds,
            &capture,
            ran.elapsed.saturating_add(started.elapsed()),
            initialized,
        )
    }

    /// How much memory this environment has added to its thread since it was BUILT.
    ///
    /// The same reading the per-run ceiling is measured with ([`Bounds::step`]), taken on the
    /// environment's own thread: the pages this thread has faulted in, which is what the
    /// interpreter standing in it has taken of the host. The count never falls, so what it
    /// reports is an UPPER bound on what the environment is holding now — a page freed and
    /// touched again is counted twice — which is the side to err on for a bound: an
    /// environment is retired on this reading, so the reading may retire one that had given
    /// memory back and never keeps one that has not.
    ///
    /// It is what bounds a STANDING environment ([`super::warm_pool`]), where a per-invocation
    /// delta says nothing: the share of the run envelope an invocation held goes back when the
    /// invocation is answered, and this is what the environment still holds afterwards.
    pub(super) fn grown_bytes(&self) -> u64 {
        faulted_bytes().saturating_sub(self.faulted_at_build)
    }

    /// Bind one invocation onto this environment and run it.
    fn ran(&self, request: &Run<'_>, clock: &Clock) -> Ran {
        let started = Instant::now();
        // The invocation the environment was BUILT for keeps the log and the bounds the
        // initialization was spent against; every later invocation brings its own, so what
        // one run wrote and what it spent are never another run's.
        let (capture, bounds, reused) = match self.initializing.borrow_mut().take() {
            Some((capture, bounds)) => (capture, bounds, false),
            None => {
                let capture = Arc::new(Mutex::new(Capture::new(request.budget.log_bytes)));
                let bounds = Arc::new(Bounds::new(&request.budget, clock.clone()));
                self.session.bind(
                    &capture,
                    &bounds,
                    request.aws,
                    request.durable,
                    request.file_systems,
                );
                watch_run(&bounds);
                (capture, bounds, true)
            }
        };
        let input = match invocation_input(request) {
            Ok(input) => input,
            Err(error) => {
                return Ran {
                    completion: Completion::Internal(error),
                    bounds,
                    capture,
                    elapsed: started.elapsed(),
                    initialized: !reused,
                }
            }
        };
        // The INIT phase of this environment is over and this invocation's own begins: what
        // the module-level code of the package wrote while the environment was being built is
        // the initialization's output, which precedes the invocation's opening frame in the
        // function's log ([`Capture::begin_invocation`]).
        captured(&capture).begin_invocation();
        let completion = self.interpreter.enter(|vm| {
            match served(vm, self, &input, request.environment, &bounds, reused) {
                Ok(completion) => completion,
                Err(exception) => Completion::Internal(formatted_exception(vm, &exception)),
            }
        });
        Ran {
            completion,
            bounds,
            capture,
            elapsed: started.elapsed(),
            initialized: !reused,
        }
    }

    /// Tear the environment down, once no further invocation will be served in it.
    ///
    /// The interpreter is FINALIZED rather than only dropped, which is what releases the
    /// objects it built: an interpreter that is only dropped leaves its own reference cycles
    /// — the module table, the code objects, the exception types — uncollected, so the
    /// environment would hold its interpreter's memory for the life of this process. Finalizing
    /// it is also what the language itself does at the end of a program, so a handler's
    /// `atexit` callback runs the way it runs on the real runtime — and it runs BOUNDED: the
    /// hook is still installed and still answering itself as each frame's local tracer, and
    /// the bounds bound here are a span of their own rather than the spent ones of the
    /// invocation this environment last served, so the callbacks of an environment that is
    /// retired long after its last invocation are neither unbounded nor stopped before their
    /// first event.
    pub(super) fn finalize(self, budget: Budget, clock: &Clock) {
        let capture = Arc::new(Mutex::new(Capture::new(budget.log_bytes)));
        let bounds = Arc::new(Bounds::new(&budget, clock.clone()));
        // No SDK seam: a callback running as the environment is torn down belongs to no
        // invocation, so there is no caller whose identity a call could be made as — and no
        // durable execution either, so nothing it does is checkpointed on one.
        // No mounts either: what the environment mounted belongs to the invocations it served,
        // and a callback running as it is torn down is serving none.
        self.session.bind(&capture, &bounds, None, None, &[]);
        watch_run(&bounds);
        self.torn_down();
    }

    /// Finalize the interpreter, against whatever is bound on the session.
    fn torn_down(self) {
        let Self {
            interpreter,
            session: _,
            held,
            entries,
            initializing: _,
            faulted_at_build: _,
        } = self;
        // The entry points and the mappings this environment wrote every invocation's
        // variables through are the interpreter's objects, so they go before it is finalized.
        drop(entries);
        interpreter.finalize(None);
        // Only now: the hook and this cell hold each other, so emptying it is what lets the
        // one the environment installed go.
        held.borrow_mut().take();
    }
}

/// What one invocation came to, before the latch and the log are read off it ([`settled`]).
struct Ran {
    /// What the phases of this invocation answered.
    completion: Completion,
    /// The bounds it was spent against.
    bounds: Arc<Bounds>,
    /// The log it wrote.
    capture: Arc<Mutex<Capture>>,
    /// How long it took.
    elapsed: Duration,
    /// Whether this invocation paid the INITIALIZATION of the environment it ran in.
    initialized: bool,
}

/// What an invocation brings to the environment serving it, which the host functions and the
/// trace hook of that environment read.
///
/// An environment's host surface is installed ONCE, with its interpreter, and then serves
/// invocation after invocation: every one of those functions reads the invocation being
/// served from here rather than closing over one invocation's own values. So the log a
/// handler writes to, the bounds its run is spent against, the SDK seam it reaches this
/// emulator through and how many calls it has made are the invocation's own, and nothing of
/// one invocation is readable by the next.
pub(super) struct Session {
    /// The invocation being served.
    serving: Mutex<Serving>,
    /// How many calls into this emulator the invocation being served has made.
    calls: AtomicU64,
    /// How many CHILD PROCESSES the invocation being served has started
    /// ([`MAX_CHILDREN_PER_RUN`]).
    children: AtomicU64,
    /// The CONNECTIONS the invocation being served has open, and what it has carried on them
    /// ([`super::guest_socket`]). Held by the environment, like the session itself, and given
    /// up with the invocation that opened them.
    sockets: super::guest_socket::Sockets,
    /// The DATABASES this environment holds open ([`super::guest_sqlite`]), and the engine
    /// behind them.
    ///
    /// Held by the environment and NOT given up between invocations, unlike the connections
    /// above: a database is memory the handler's own code allocated — a handler that opens one
    /// in its module body and keeps it in a global reads it again in the next invocation that
    /// environment serves, exactly as it reads any other module global — and it goes when the
    /// environment does.
    databases: super::guest_sqlite::Databases,
    /// The FILE SYSTEMS the invocation being served has mounted
    /// ([`super::guest_file_system`]): the trees the host answers this environment's
    /// filesystem calls on paths of those mounts out of. Resolved per invocation, so what a
    /// handler reaches is the file system Amazon EFS holds at the instant it runs.
    file_systems: Mutex<super::guest_file_system::GuestFileSystems>,
}

/// The invocation an environment is serving right now.
struct Serving {
    /// Where its output goes.
    capture: Arc<Mutex<Capture>>,
    /// What it may spend.
    bounds: Arc<Bounds>,
    /// How its handler reaches the rest of this emulator, when it was given a seam.
    aws: Option<AwsAccess>,
    /// Where it reports the steps of the durable execution it is the run of, when it is one.
    durable: Option<DurableStepJournal>,
}

impl Session {
    /// The session of an environment about to serve its first invocation.
    fn new(
        capture: &Arc<Mutex<Capture>>,
        bounds: &Arc<Bounds>,
        aws: Option<&AwsAccess>,
        durable: Option<&DurableStepJournal>,
        file_systems: &[crate::state::FileSystemMount],
    ) -> Arc<Self> {
        Arc::new(Self {
            serving: Mutex::new(Serving {
                capture: Arc::clone(capture),
                bounds: Arc::clone(bounds),
                aws: aws.cloned(),
                durable: durable.cloned(),
            }),
            calls: AtomicU64::new(0),
            children: AtomicU64::new(0),
            sockets: super::guest_socket::Sockets::default(),
            databases: super::guest_sqlite::Databases::default(),
            file_systems: Mutex::new(super::guest_file_system::GuestFileSystems::attach(
                file_systems,
            )),
        })
    }

    /// Bind the NEXT invocation onto this environment: its own log, its own bounds, its own
    /// seam, and a call count of its own — so no budget of the invocation before it is
    /// carried over.
    fn bind(
        &self,
        capture: &Arc<Mutex<Capture>>,
        bounds: &Arc<Bounds>,
        aws: Option<&AwsAccess>,
        durable: Option<&DurableStepJournal>,
        file_systems: &[crate::state::FileSystemMount],
    ) {
        let mut serving = self.serving.lock().unwrap_or_else(PoisonError::into_inner);
        serving.capture = Arc::clone(capture);
        serving.bounds = Arc::clone(bounds);
        serving.aws = aws.cloned();
        serving.durable = durable.cloned();
        drop(serving);
        self.calls.store(0, Ordering::SeqCst);
        // Nor a budget of children: how many programs one invocation may start is that
        // invocation's own ([`MAX_CHILDREN_PER_RUN`]).
        self.children.store(0, Ordering::SeqCst);
        // Nothing of the invocation before it is carried over on a connection either: every
        // socket it opened is closed here, and the next invocation starts with none.
        self.sockets.release();
        // The mounts are this invocation's own too: the file systems its function's
        // configuration names are resolved for it, so a handler reaches the file system Amazon
        // EFS holds now and what that function's role may do to it now.
        *self
            .file_systems
            .lock()
            .unwrap_or_else(PoisonError::into_inner) =
            super::guest_file_system::GuestFileSystems::attach(file_systems);
    }

    /// What the host answers for ONE call the invocation being served made on a path of the
    /// file systems it mounted ([`super::guest_file_system`]).
    fn file_system_call(&self, document: &str) -> String {
        self.file_systems
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .answered(document)
    }

    /// The mount table the GUEST is handed, which says where each file system is mounted and
    /// what the handler may do there.
    fn file_system_table(&self) -> ::serde_json::Value {
        self.file_systems
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .table()
    }

    /// Where the invocation being served writes its output.
    fn capture(&self) -> Arc<Mutex<Capture>> {
        Arc::clone(
            &self
                .serving
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .capture,
        )
    }

    /// What the invocation being served may spend.
    fn bounds(&self) -> Arc<Bounds> {
        Arc::clone(
            &self
                .serving
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .bounds,
        )
    }

    /// Where the invocation being served reports the steps of the durable execution it is
    /// the run of, and `None` for one that is no execution's run.
    fn durable(&self) -> Option<DurableStepJournal> {
        self.serving
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .durable
            .clone()
    }

    /// How the invocation being served reaches the rest of this emulator.
    fn aws(&self) -> Option<AwsAccess> {
        self.serving
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .aws
            .clone()
    }

    /// Stop holding the SDK seam of the invocation just served, so an environment standing
    /// idle holds no credential of the caller whose invocation it served — and close every
    /// connection that invocation opened, so an environment standing idle holds no socket of
    /// it either.
    fn release_access(&self) {
        let mut serving = self.serving.lock().unwrap_or_else(PoisonError::into_inner);
        serving.aws = None;
        // Nor the durable execution it was the run of: an environment standing idle is no
        // run, so there is no execution for it to checkpoint on.
        serving.durable = None;
        drop(serving);
        self.sockets.release();
    }

    /// ONE call the invocation being served made on a SOCKET of its own
    /// ([`super::guest_socket`]): the connections are this environment's, WHERE a connection
    /// may be opened is decided by the register of the planes this emulator has opened and by
    /// the VPC the function is attached to, and the plane's own handler runs on the async
    /// runtime the invocation's seam holds. An invocation with no seam reaches no plane and no
    /// VPC, which is what a run with no seam is.
    fn socket_call(&self, verb: &str, document: &str, within: Duration) -> Option<String> {
        let access = self
            .serving
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .aws
            .clone()?;
        Some(self.sockets.answered(
            access.dialing().as_ref(),
            access.inside_vpc().as_ref(),
            &access.runtime(),
            verb,
            document,
            within,
        ))
    }

    /// ONE call the invocation being served made on a DATABASE of this execution environment
    /// ([`super::guest_sqlite`]): the databases are the ENVIRONMENT's, so a connection a
    /// handler opened in its module body is still open for the next invocation it serves, and
    /// every statement is compiled and run by the engine this crate embeds.
    fn database_call(&self, verb: &str, document: &str, within: Duration) -> String {
        self.databases.answered(verb, document, within)
    }

    /// Whether the invocation being served may make one more call into this emulator, which
    /// spends one of the calls ONE invocation is allowed ([`MAX_CALLS_PER_RUN`]).
    fn may_call(&self) -> bool {
        self.calls.fetch_add(1, Ordering::Relaxed) < MAX_CALLS_PER_RUN as u64
    }

    /// Whether the invocation being served may start another CHILD PROCESS, which spends one
    /// of the children ONE invocation is allowed ([`MAX_CHILDREN_PER_RUN`]).
    ///
    /// A program is performed in process and costs this host the work its own bounds allow
    /// ([`super::guest_child_process`]), so how many of them one invocation may ask for is a
    /// bound of its own — held here, where no guest code can reach it.
    fn may_start_a_child(&self) -> bool {
        self.children.fetch_add(1, Ordering::Relaxed) < MAX_CHILDREN_PER_RUN as u64
    }
}

/// How a completion is reported once the phase that produced it is over: the latched stop
/// wins over whatever the guest answered, and the log is what the invocation itself wrote.
fn settled(
    completion: Completion,
    bounds: &Arc<Bounds>,
    capture: &Arc<Mutex<Capture>>,
    elapsed: Duration,
    initialized: bool,
) -> Finished {
    // The stop LATCH wins over whatever the guest answered: a run whose budget was spent
    // is a stopped run even if the guest caught the exception the hook raised.
    let completion = match bounds.stopped() {
        Some(stop) => Completion::Stopped(stop),
        None => completion,
    };
    let written = captured(capture);
    Finished {
        completion,
        log: written.text.clone(),
        log_records: written.invocation_records(),
        init_records: written.init_records(),
        initialized,
        // How long this environment's initialization took is measured by the pool that holds
        // it ([`super::warm_pool`]), which is where the span begins.
        init_duration: None,
        // WHICH stream the records of this run belong to is the pool's, which holds the
        // environment they were written in ([`super::warm_pool`]).
        log_stream: None,
        elapsed,
        // What the handler asked to WAIT for that no host time passed over: the run's own
        // execution lasted that much longer than this host's elapsed time, which is what the
        // invocation holds the function's concurrency for
        // ([`Bounds::unwaited`]).
        unwaited: bounds.unwaited(),
        max_memory_bytes: Some(bounds.max_memory_bytes()),
    }
}

/// What building an environment came to.
enum Initialized {
    /// The environment is standing, with the function's own initialization done in it.
    Ready,
    /// The initialization answered rather than completed, and this is what it answered.
    Answered(Completion),
}

/// The entry points one prepared environment is driven through ([`super::bootstrap`]).
struct Entries {
    /// The handler's own run.
    invoke: PyObjectRef,
    /// Binding the next invocation's request onto the environment.
    bind: PyObjectRef,
    /// The INIT phase: the import of the handler's own module, which the function this
    /// environment is adopted by is read for.
    initialize: PyObjectRef,
    /// Where this environment's invocations have their variables written
    /// ([`EnvironmentSinks`]).
    environ: EnvironmentSinks,
}

/// Install the host surface every execution environment has and evaluate the runtime source
/// in it: the whole of the build that reads no function's configuration.
///
/// None of it depends on which function is served here, which is why it can be stood up
/// before there is an invocation to serve at all ([`blank`]): the interpreter is the same one
/// whatever function adopts it, the surface it is narrowed to is the same surface, and the
/// runtime source is this emulator's own.
///
/// The TRACE HOOK is installed last, once the runtime's own source has been evaluated, so
/// none of this emulator's own frames is traced and nothing of this work is charged to a run.
fn stand_up(vm: &VirtualMachine, session: &Arc<Session>, held: &TraceCell) -> PyResult<Entries> {
    let environ = prepare(vm, session)?;
    let scope = vm.new_scope_with_builtins();
    let code = vm
        .compile(
            super::bootstrap::RUNTIME_SOURCE,
            ::rustpython_vm::compiler::Mode::Exec,
            "<lambda-runtime>".to_owned(),
        )
        .map_err(|error| {
            vm.new_runtime_error(format!("the Lambda runtime source does not compile: {error}"))
        })?;
    vm.run_code_obj(code, scope.clone())?;
    let entries = Entries {
        invoke: scope.globals.get_item("__lambda_invoke", vm)?,
        bind: scope.globals.get_item("__lambda_bind", vm)?,
        initialize: scope.globals.get_item("__lambda_prepare", vm)?,
        environ,
    };
    install_trace_hook(vm, session, held)?;
    Ok(entries)
}

/// Apply the FUNCTION's own configuration to a standing environment and run the INIT phase
/// in it, for `request` — the first invocation that environment serves.
fn initialize(
    vm: &VirtualMachine,
    request: &Run<'_>,
    input: &str,
    entries: &Entries,
    bounds: &Arc<Bounds>,
) -> PyResult<Initialized> {
    install_function_surface(vm, &entries.environ, request)?;
    // The interpreter is standing and nothing of the RUN's own code has been read yet, so
    // this is where the memory the run itself takes is measured from, and where the span its
    // initialization is allowed starts running. The host takes both, never the guest.
    bounds.measure_memory_from_here();
    bounds.begin_initialization();
    // The INIT phase, under the span an initialization is allowed rather than the
    // function's `Timeout`. It answers nothing when the environment is ready, and the
    // document of a failure that belongs to no invocation when it is not.
    let answered = entries
        .initialize
        .call((vm.ctx.new_str(input.to_owned()),), vm)?;
    let answered: String = answered.try_into_value(vm)?;
    if !answered.is_empty() {
        return Ok(Initialized::Answered(read_completion(
            &answered,
            bounds.response_bytes,
            Phase::Initialization,
        )));
    }
    Ok(Initialized::Ready)
}

/// Install everything of the host surface that one FUNCTION's configuration states, onto an
/// environment whose function-independent surface is already standing ([`stand_up`]): what
/// the execution environment IS, the variables the invocation runs with, how deep its calls
/// may nest and which release of the language its runtime names.
///
/// This is the half of the build that cannot be stood up before the function is known, and
/// it happens before a line of the function's own code is read — the order the installations
/// are in among themselves is the order they were in when one call did both halves.
///
/// WHY it may happen after the rest of the surface and after this emulator's own runtime
/// source has been evaluated, which is what changed when the build was split in two. Each of
/// these is READ at the instant a program asks for it, and the only reader of any of them is
/// the GUEST, whose first line runs in the INIT phase below:
///
/// * what the environment IS is an attribute [`super::host_posix::module`]'s `uname` answers
///   from when it is CALLED, and the ordering the codec registry needed is the one that is
///   still in place — the patched `io` and the COMPLETE `os`, which [`prepare`] installs
///   before it ([`super::host_posix::install_data_surface`]) — rather than the identity of a
///   function nothing had yet;
/// * the invocation's own variables are written into the mappings the host captured, and
///   nothing of this emulator's own runtime reads a variable while it is evaluated: the
///   runtime's library directory and its log settings are read out of the request document
///   the INIT phase is handed ([`super::bootstrap`]'s `_runtime_library`, `_main_prepare`),
///   not out of `os.environ`;
/// * the release and the implementation are what `sys` reports, read by the guest's own
///   imports (the standard library modules this runtime supplies read them at THEIR import,
///   which is a guest import — [`super::guest_sysconfig`]), and the one reading the
///   interpreter itself fixed before any of this (`importlib`'s cache tag) was already fixed
///   when the interpreter was built, in this order and in the one before it.
///
/// What a run observes is therefore the same either way, which
/// `tests/all/pre_provisioned_execution_environments.rs` pins over an environment that was
/// stood up before the invocation: the machine, the release, the implementation and the
/// variables a run reports are its own function's.
fn install_function_surface(
    vm: &VirtualMachine,
    environ: &EnvironmentSinks,
    request: &Run<'_>,
) -> PyResult<()> {
    // What the execution environment this run is in IS, which `os.uname` answers and the
    // standard library's `platform` module is built on: the HOST states it, from the
    // function's own configuration ([`super::host_posix::install_environment_identity`]),
    // rather than anything of the machine this process is on.
    super::host_posix::install_environment_identity(vm, request.environment, request.architecture)?;
    // The variables the invocation runs with, written into the mappings the host captured
    // while no guest code had run ([`EnvironmentSinks`]). Nothing is REPLACED: this is the
    // first invocation this environment serves, so there is nothing in them yet.
    environ.install(vm, request.environment, false)?;
    clamp_recursion(vm, request.budget.recursion_limit)?;
    // WHICH RELEASE of the language this execution environment is and WHICH IMPLEMENTATION of
    // it, both of which the function's own `Runtime` names ([`super::python_release`]).
    // Installed LAST, so the identity the run reports is read by the guest's own imports and
    // by nothing else.
    super::python_release::install(vm, request.environment, request.architecture)?;
    Ok(())
}

/// Serve one invocation in a prepared environment: bind its request, then run the handler.
///
/// The environment the invocation runs with is installed again here, because a reused
/// environment serves an invocation whose own request and environment are its own: what the
/// handler reads off its context and out of `os.environ` is this invocation's.
///
/// `reused` says whether an invocation has been served in this environment BEFORE, which
/// decides two things about this one.
///
/// Its environment VARIABLES: the invocation an environment was built for has them installed
/// by the initialization, and every later invocation's REPLACE the ones of the invocation
/// before it, so what a handler reads out of `os.environ` is its own invocation's environment
/// (the X-Ray trace header AWS documents as changing with each invocation included) and never
/// a variable left behind ([`EnvironmentSinks::install`]). What the GUEST itself left in the
/// interpreter, its module-level objects and whatever it cached in them, does stand: that is
/// the reuse AWS documents.
///
/// And its memory BASELINE: the invocation an environment was built for had it taken before
/// the module-level code of its own package was read ([`initialize`]), because that code is
/// its own work; every later invocation has it taken here, because the environment it is
/// served in was built before it and memory the invocation did not allocate is not a bound it
/// breached.
fn served(
    vm: &VirtualMachine,
    prepared: &Prepared,
    input: &str,
    environment: &BTreeMap<String, String>,
    bounds: &Arc<Bounds>,
    reused: bool,
) -> PyResult<Completion> {
    prepared.entries.environ.install(vm, environment, reused)?;
    if reused {
        bounds.measure_memory_from_here();
    }
    // Binding the invocation is the runtime's own work on RECEIVING one: it unmarshals the
    // payload the caller sent and builds the context object out of this invocation's own
    // members ([`super::bootstrap`]), neither of which is the handler's run — so both happen
    // before the invoke phase is begun below and the handler is told it has its whole
    // `Timeout` left when it is entered.
    let bound = prepared
        .entries
        .bind
        .call((vm.ctx.new_str(input.to_owned()),), vm)?;
    let bound: String = bound.try_into_value(vm)?;
    if !bound.is_empty() {
        return Ok(read_completion(
            &bound,
            bounds.response_bytes,
            Phase::Invocation,
        ));
    }
    // The INVOKE phase begins HERE, which is the handler's own run: from this instant the
    // deadline is the function's `Timeout`, and the instant it falls at is what the handler
    // is told it has left.
    let deadline_epoch_ms = bounds.start();
    let answered = prepared
        .entries
        .invoke
        .call((vm.ctx.new_int(deadline_epoch_ms),), vm)?;
    let text: String = answered.try_into_value(vm)?;
    Ok(read_completion(
        &text,
        bounds.response_bytes,
        Phase::Invocation,
    ))
}

/// Have the host's own watch sample this run's deadline while the run lasts.
///
/// The watch is the SHARED one of this emulator's Python substrate
/// ([`::python_guest_substrate::watch_run`]): ONE thread for the whole process, holding a
/// WEAK reference to each run's bounds, so a run it is sampling is one that is still going
/// and a run that ended simply falls out of its list. A long NATIVE call emits no traced
/// event, so a deadline checked only from the trace hook is one the guest decides the timing
/// of; the watch reads the same clock from outside the run, so a run that stops producing
/// events is still noticed and still stopped.
fn watch_run(bounds: &Arc<Bounds>) {
    ::python_guest_substrate::watch_run(bounds);
}

/// What the watch does with a sample of one run of a function's handler, which is this
/// module's own reading of its budget.
impl ::python_guest_substrate::WatchedRun for Bounds {
    fn watch(&self) {
        Self::watch(self);
    }
}

/// The interpreter itself: the frozen standard library and the ALLOW-LISTED native
/// modules, and nothing else ([`::python_guest_substrate::contained_interpreter`], shared
/// with the other service of this emulator that runs caller-supplied Python).
///
/// `contained` reports whether the narrowing really happened: a run is refused rather than
/// served if it did not.
fn interpreter(contained: &Arc<AtomicBool>) -> Interpreter {
    ::python_guest_substrate::contained_interpreter(
        ALLOWED_NATIVE_MODULES,
        // The `posix` module a run is built on is THIS crate's (see [`super::host_posix`]):
        // the import system requires a module of that name to exist, and the engine's own —
        // whose calls are the host's — needs an engine feature this crate does not build at
        // all, so there is none in the binary to register.
        |vm| vec![super::host_posix::module::module_def(&vm.ctx)],
        contained,
    )
}

/// Install everything the guest sees of the host that is the same for EVERY execution
/// environment, before any guest code runs: the modules this emulator supplies in place of
/// the ones it does not register, the refusals that replace the host-reaching calls of the
/// two it does, and the log the handler's output goes into.
///
/// What ONE function's configuration states is installed beside it, by
/// [`install_function_surface`], when an environment is adopted by a function.
fn prepare(vm: &VirtualMachine, session: &Arc<Session>) -> PyResult<EnvironmentSinks> {
    let modules = vm.sys_module.as_object().get_attr("modules", vm)?;
    // Every allow-listed native module is created ONCE, here, so each is the single
    // patched object `sys.modules` holds from now on. Nothing is imported from a host
    // path: these are the interpreter's own built-in modules.
    for name in ALLOWED_NATIVE_MODULES.iter().chain(["posix"].iter()) {
        let _ = vm.import(*name, 0);
    }
    refuse_host_file_access(vm)?;
    narrow_posix(vm)?;
    // The DATA `os` re-exports from the module it is built on — the `stat_result` type and
    // the `open` flags — installed before any import of `os`, because that is where `os`
    // takes them from ([`super::host_posix::install_data_surface`]).
    super::host_posix::install_data_surface(vm)?;
    // The codec registry, AFTER the installation above: whatever it imports then sees the
    // same patched `io` and the same complete `os` every later import sees, so it can hold
    // neither an unpatched way of opening a host file nor a half-built `os`.
    install_codec_registry(vm)?;
    // The mappings the invocation's environment is written into, captured by the HOST while
    // no guest code has run ([`EnvironmentSinks`]). What is written THROUGH them is the
    // adopting function's ([`install_function_surface`]): an environment no function has
    // adopted yet holds no variable of one.
    let environ = EnvironmentSinks::capture(vm)?;
    refuse_guest_threads(vm)?;
    clamp_guest_sleep(vm, session)?;
    // Put the host module in `sys.modules` under the name the runtime source imports it
    // by. The import machinery answers from there first, and the name is NOT in the native
    // module table ([`ALLOWED_NATIVE_MODULES`]), so there is no other module of that name
    // to be had: a guest that drops this one cannot import a second.
    modules.set_item(HOST_MODULE, host_module(vm, session)?.into(), vm)?;
    // And the module a SPARK program's statements are planned and run through, registered the
    // same way and for the same reason: the name is not in the native module table either, so
    // a guest that drops it cannot import a second.
    modules.set_item(
        SPARK_HOST_MODULE,
        spark_host_module(vm, session)?.into(),
        vm,
    )?;
    // A built-in module can no longer be CREATED: every one the guest can reach is the
    // patched object already in `sys.modules`, and a guest that drops one cannot build
    // itself an unpatched replacement. `is_builtin` stays, because the import system asks
    // it on every import; what is taken away is the creation itself.
    let importer = vm.import("_imp", 0)?;
    let refusing = vm.new_function("create_builtin", |_args: FuncArgs, vm: &VirtualMachine| {
        Err::<PyObjectRef, _>(vm.new_import_error(
            "a Lambda function in this emulator cannot create a built-in module".to_owned(),
            vm.ctx.new_str("<builtin>"),
        ))
    });
    for name in [
        "create_builtin",
        "exec_builtin",
        "create_dynamic",
        "exec_dynamic",
    ] {
        // Checked: a creation call left in place would let a guest build itself an unpatched
        // native module, so one that could not be replaced refuses the run.
        let refusing: PyObjectRef = refusing.clone().into();
        importer.set_attr(name, refusing.clone(), vm)?;
        installed_refusal(vm, &importer, name, &refusing, "_imp")?;
    }
    // `os` decides which operating-system module to build itself on from this tuple, so it
    // names the one this emulator supplies.
    let names: Vec<PyObjectRef> = SUPPLIED_MODULES
        .iter()
        .chain(ALLOWED_NATIVE_MODULES.iter())
        .map(|name| vm.ctx.new_str(*name).into())
        .collect();
    vm.sys_module
        .as_object()
        .set_attr("builtin_module_names", vm.ctx.new_tuple(names), vm)?;
    Ok(environ)
}

/// WHERE an invocation's own environment is written: the two mappings a handler reads its
/// variables out of, each captured by the HOST.
///
/// `posix.environ` is the mapping [`super::host_posix`] created for this interpreter, and
/// `os.environ` is the copy `os` builds from it. Both objects are taken ONCE, by [`capture`]
/// while no guest code has run, and every invocation's variables are written through the
/// objects held here rather than through whatever those two attribute names resolve to at the
/// time. `posix` and `os` are modules a guest can import and whose attributes it can rebind,
/// and what a guest leaves in a standing execution environment survives into the invocations
/// served in it after it ([`served`]) — so a sink resolved per invocation could be an object a
/// PREVIOUS invocation put there, and the host would then write the NEXT invocation's
/// variables into it. Holding the host's own objects is what keeps one invocation's
/// environment out of another's reach.
///
/// Each attribute is re-asserted to the captured object and read back before anything is
/// written ([`Self::held`]), so an environment whose guest rebound either name has the host's
/// own mapping back, and one where that cannot be restored serves no invocation at all.
struct EnvironmentSinks {
    /// The mapping `os` builds `os.environ` out of, which starts EMPTY, so what a handler
    /// reads is the function's own variables and nothing of this process's environment.
    posix: PyObjectRef,
    /// The copy `os` keeps once it has been imported. Writing through it reaches the mapping
    /// above ([`super::host_posix`]'s `putenv`) and nothing of this process's own environment.
    system: PyObjectRef,
}

impl EnvironmentSinks {
    /// Take both mappings, before a line of guest code is read.
    ///
    /// `os` is imported HERE, by the host, for the same reason: the copy it builds is captured
    /// while nothing of the guest's has run, so the object this environment writes through is
    /// never one guest code chose.
    fn capture(vm: &VirtualMachine) -> PyResult<Self> {
        Ok(Self {
            posix: vm.import("posix", 0)?.get_attr("environ", vm)?,
            system: vm.import("os", 0)?.get_attr("environ", vm)?,
        })
    }

    /// Install `variables` as the environment of the invocation about to be served.
    ///
    /// Done HERE rather than from the runtime source, so the guest's environment is the host's
    /// statement of it and no Python code of this emulator's own needs an operating-system
    /// module. The entries of the POSIX mapping are bytes, which is the form `os` decodes a
    /// POSIX environment from; the copy takes the same names and values as text, which is what
    /// a handler reads off `os.environ`.
    ///
    /// `replacing` is set for an invocation served in an environment that has already served
    /// one: both mappings are CLEARED first, so the variables an invocation sees are exactly
    /// the ones of its own environment — a variable the invocation before it had, or one its
    /// guest code wrote itself, is not left for this one to read, and the variable AWS
    /// documents as changing with each invocation (the X-Ray trace header) is this
    /// invocation's.
    ///
    /// Every step is CHECKED, because this emptying is the whole of what keeps one
    /// invocation's environment out of the next one's reach: `os.environ` is a mapping of a
    /// guest-reachable class, so a clear or a write that raised would leave the invocation
    /// about to be served reading what the invocation before it left there. An environment
    /// whose sinks cannot be emptied and written serves no invocation at all — the failure is
    /// answered to the caller and the environment is not kept ([`super::warm_pool`]).
    fn install(
        &self,
        vm: &VirtualMachine,
        variables: &BTreeMap<String, String>,
        replacing: bool,
    ) -> PyResult<()> {
        self.held(vm, "posix", &self.posix)?;
        self.held(vm, "os", &self.system)?;
        if replacing {
            self.emptied(vm, "posix", &self.posix)?;
            self.emptied(vm, "os", &self.system)?;
        }
        for (name, value) in variables {
            let key: PyObjectRef = vm.ctx.new_bytes(name.as_bytes().to_vec()).into();
            let held: PyObjectRef = vm.ctx.new_bytes(value.as_bytes().to_vec()).into();
            self.posix.set_item(&*key, held, vm)?;
            let key: PyObjectRef = vm.ctx.new_str(name.clone()).into();
            let held: PyObjectRef = vm.ctx.new_str(value.clone()).into();
            self.system.set_item(&*key, held, vm)?;
        }
        Ok(())
    }

    /// Empty `mapping`, which is `<module>.environ`, and confirm it IS empty.
    ///
    /// The call and the reading are both the host's: a mapping whose `clear` raised, and one
    /// that answered and left entries behind, are the same thing here — a sink still holding
    /// the variables of the invocation served before this one. Either refuses the run, in the
    /// shape [`installed_refusal`] confirms a containment patch in, and the refusal states
    /// what the mapping raised ([`formatted_exception`]) so the containment failure can be
    /// read rather than guessed at.
    fn emptied(
        &self,
        vm: &VirtualMachine,
        module: &'static str,
        mapping: &PyObjectRef,
    ) -> PyResult<()> {
        let refused = |vm: &VirtualMachine, why: String| {
            vm.new_runtime_error(format!(
                "the environment mapping this emulator holds for {module}.environ could not be \
                 emptied ({why}), so no further invocation is served in this execution \
                 environment"
            ))
        };
        vm.call_method(mapping, "clear", ())
            .map_err(|raised| refused(vm, formatted_exception(vm, &raised)))?;
        let left = mapping
            .length(vm)
            .map_err(|raised| refused(vm, formatted_exception(vm, &raised)))?;
        if left == 0 {
            return Ok(());
        }
        Err(refused(vm, format!("{left} variable(s) are still in it")))
    }

    /// Re-assert `<module>.environ` as the mapping the host captured and read it back: the
    /// host's own confirmation that the sink about to be written is still its own object, in
    /// the shape [`installed_refusal`] confirms a containment patch in.
    ///
    /// A name a guest rebound is restored by the assertion; a name that cannot be restored is
    /// a sink of someone else's choosing, so no invocation is served through it.
    fn held(
        &self,
        vm: &VirtualMachine,
        module: &'static str,
        mapping: &PyObjectRef,
    ) -> PyResult<()> {
        let holder = vm.import(module, 0)?;
        holder.set_attr("environ", mapping.clone(), vm)?;
        if holder.get_attr("environ", vm)?.is(mapping) {
            return Ok(());
        }
        Err(vm.new_runtime_error(format!(
            "the environment mapping this emulator holds for {module}.environ is no longer the \
             one it created, so no function is run in this interpreter"
        )))
    }
}

/// Put the standard library's CODEC REGISTRY on its feet, the way a Python interpreter's own
/// start-up does, before any guest code runs.
///
/// Every text encoding a program names by its name — `bytes.decode("cp437")`,
/// `str.encode("cp1252")`, `codecs.lookup(..)` — is resolved through the search function the
/// standard library's `encodings` package registers while it is being imported, and the
/// interpreter resolves only a handful of encodings on its own without it. The engine crate
/// performs that import at start-up only under its own `encodings` feature, which this crate
/// does not take (it takes `compiler` alone, see this crate's `Cargo.toml`, so the rest of
/// that crate's start-up surface stays out of the build), so the import is performed here
/// instead: without it a module of the standard library that names one of the other encodings
/// — the archive reader decoding an entry's name is one — raises where it reads cleanly on a
/// managed runtime, and whether it raised would depend on whether the handler happened to
/// import `encodings` itself.
///
/// The package is the FROZEN standard library's own ([`interpreter`]), so no host path is read
/// to import it and nothing of the host becomes reachable through it.
fn install_codec_registry(vm: &VirtualMachine) -> PyResult<()> {
    vm.import("encodings", 0).map(|_| ())
}

/// Take away every way of opening a HOST file: the TYPE that opens one, and each of the three
/// names a Python program reaches it through.
///
/// The refusal is put on the type's own construction, not only on the names: a type that has
/// been created stays in the interpreter's type graph and is reachable by traversal from
/// `object`, so replacing `_io.FileIO` alone would leave the type that opens the file one
/// `__subclasses__` walk away. With `__new__` and `__init__` refusing, an instance obtained
/// however it was reached opens nothing. The guest sees the names it expects — `open`,
/// `io.open`, `_io.FileIO` — and the call raises. The rest of `_io` (the in-memory streams the
/// standard library is built on) is left exactly as it is.
///
/// What a run opens instead is the FILESYSTEM of its own execution environment: the runtime
/// source answers `open` from the entries of the function's own mounts and calls the refusal
/// installed here for every path under none of them ([`super::bootstrap`]), so a file the
/// deployment package ships is read out of this process's memory and a path of the host is
/// refused here, as it is for a run no package was mounted for at all.
///
/// Every one of those installations is CHECKED, and a failure refuses the whole run: a patch
/// whose result was thrown away would leave this emulator running caller-supplied code in an
/// interpreter that can still open a host file, and no later reading would say so. The names
/// are read back afterwards and each must answer the refusal this function installed
/// ([`installed_refusal`]).
fn refuse_host_file_access(vm: &VirtualMachine) -> PyResult<()> {
    ::python_guest_substrate::refuse_host_file_access(
        vm,
        "a Lambda function in this emulator cannot open a file of the host",
        REFUSED_RUN,
    )
}

/// What this crate refuses when a containment patch did not take effect, named as this
/// service's own run.
const REFUSED_RUN: &str = "no function is run in this interpreter";

/// Read `attribute` of `holder` back and answer whether it IS the refusal that was just put
/// there — the host's own confirmation that a containment patch took effect
/// ([`::python_guest_substrate::installed_refusal`], shared with the other service of this
/// emulator that runs caller-supplied Python).
///
/// A patch that was applied and did not take — a name the interpreter would not let this host
/// replace — is a containment hole no later reading would report, so it is an error here and
/// the run is refused before any guest code is driven ([`prepare`]).
fn installed_refusal(
    vm: &VirtualMachine,
    holder: &::rustpython_vm::PyObject,
    attribute: &'static str,
    refusing: &PyObjectRef,
    held_by: &str,
) -> PyResult<()> {
    ::python_guest_substrate::installed_refusal(vm, holder, attribute, refusing, held_by, REFUSED_RUN)
}

/// Replace the interpreter's own `sleep` with one clamped to what the run has left, on the
/// single clock module `sys.modules` holds.
///
/// The clamp reads the bounds of the invocation the environment is SERVING, so a sleep is
/// clamped to what that invocation has left however many invocations this environment has
/// served.
fn clamp_guest_sleep(vm: &VirtualMachine, session: &Arc<Session>) -> PyResult<()> {
    let sleeping = Arc::clone(session);
    ::python_guest_substrate::clamp_guest_sleep(vm, REFUSED_RUN, move |seconds| {
        bounded_sleep(&sleeping.bounds(), seconds);
    })
}

/// Wait for `seconds`, but never past what the run has left: a blocking wait the guest
/// asked for cannot outlast the span its function's `Timeout` allows it.
///
/// The wait is spent through the run's own bounds ([`Bounds::wait`]), which is what charges
/// the span the guest asked for against that `Timeout` and really waits it out, up to
/// [`super::GUEST_WAIT_HOST_SPAN`].
///
/// A value that is not a finite number of seconds is no wait at all, so nothing is waited
/// for: the guest decides how long to ask for, never how the host reads it.
fn bounded_sleep(bounds: &Arc<Bounds>, seconds: f64) {
    if !seconds.is_finite() || seconds <= 0.0 {
        return;
    }
    bounds.wait(Duration::from_secs_f64(seconds.min(MAX_GUEST_SLEEP_SECONDS)));
}

/// The longest a guest may ask to wait for in one call, before the run's own remaining
/// time clamps it further.
const MAX_GUEST_SLEEP_SECONDS: f64 = 3_600.0;

/// How long the host waits between two readings of a CALLBACK of the durable execution a run is
/// the run of, while that callback is still open.
///
/// The window a callback is open for is the host's, and so is the instant it closes, so a
/// reading only ever reports what already happened: this is how often the wait looks. Short
/// against the shortest timeout a handler would configure, so a callback a client signals is
/// noticed about as soon as it is signalled, and long enough that a wait of minutes is a
/// handful of readings a second rather than a spin. The span is spent SUSPENDED
/// ([`Bounds::suspend`]), so the number of readings a long wait takes costs the run's own span
/// nothing.
const CALLBACK_POLL_SPAN: Duration = Duration::from_millis(50);

/// Which PHASE of a run answered, which is what the failure of that answer belongs to: the
/// INITIALIZATION of the execution environment, or the invocation being served in it.
///
/// The phase is the HOST's own reading — it is the host that calls one phase after the other
/// ([`initialize`], [`served`]) — rather than anything read off the document the runtime
/// answered with, because the class a failure carries says nothing about when it happened:
/// the top-level code of a deployment package fails the initialization under whatever class
/// it raised.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The execution environment was still being built: the handler's module was being
    /// imported and its entry point resolved.
    Initialization,
    /// An invocation was being served in a standing environment.
    Invocation,
}

/// The runtime's answer, as this module reports it.
fn read_completion(answered: &str, response_bytes: usize, phase: Phase) -> Completion {
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
            if payload.len() > response_bytes {
                return Completion::Stopped(Stop::Response);
            }
            Completion::Returned(payload)
        }
        Some("raised") => Completion::Raised {
            error_type: member("errorType"),
            error_message: member("errorMessage"),
            initialization: phase == Phase::Initialization,
            stack_trace: document
                .get("stackTrace")
                .and_then(::serde_json::Value::as_array)
                .map(|frames| {
                    frames
                        .iter()
                        .filter_map(::serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        },
        _ => Completion::Internal("the runtime answered with no outcome".into()),
    }
}

/// The JSON document the runtime source is handed.
fn runtime_input(request: &Run<'_>, session: &Session) -> Result<String, String> {
    // The entries of the deployment package and the entries the function's LAYERS mount are
    // two MOUNTS of the execution environment, so they are handed over apart: each is
    // resolved from its own root and reports its own path ([`super::bootstrap`]).
    let mut modules = ::serde_json::Map::new();
    let mut layer_modules = ::serde_json::Map::new();
    for module in request.modules {
        let source = ::serde_json::Value::String(module.source.clone());
        match module.path.strip_prefix(LAYER_ID_PREFIX) {
            // A later layer's entry wins over an earlier one's, which is what mounting one
            // layer's content over another's at the same root leaves.
            Some(path) => layer_modules.insert(path.to_string(), source),
            None => modules.insert(module.path.clone(), source),
        };
    }
    let environment: ::serde_json::Map<String, ::serde_json::Value> = request
        .environment
        .iter()
        .map(|(name, value)| (name.clone(), ::serde_json::Value::String(value.clone())))
        .collect();
    let document = ::serde_json::json!({
        "handler": request.handler,
        "modules": modules,
        "layerModules": layer_modules,
        "runtimeModules": runtime_modules(),
        "runtimeStdlib": runtime_stdlib(),
        "environment": environment,
        "event": request.event,
        "requestId": request.request_id,
        "invokedFunctionArn": request.invoked_function_arn,
        "clientContext": request.client_context,
        "tenantId": request.tenant_id,
        // The IDENTITY the invocation was made by, under the member names the runtime
        // interface carries it in ([`crate::state::InvocationIdentity`]), and `null` for an
        // invocation made by no Amazon Cognito identity.
        "cognitoIdentity": crate::state::InvocationIdentity::runtime_member(
            request.cognito_identity,
        ),
        // The EPHEMERAL STORAGE of the environment: how large the `/tmp` the runtime's own
        // filesystem serves is ([`super::guest_ephemeral`]), and how much of it this host
        // really holds ([`super::MAX_EPHEMERAL_HELD_BYTES`]) — a function configured for more
        // than that reports the size it has and writes as far as this host can hold.
        "ephemeralStorageBytes": request.budget.ephemeral_bytes,
        "ephemeralWritableBytes": request
            .budget
            .ephemeral_bytes
            .min(super::MAX_EPHEMERAL_HELD_BYTES),
        // The NETWORK FILE SYSTEMS of the environment: where each Amazon EFS access point the
        // function's `FileSystemConfigs` name is mounted, the identity requests through it are
        // made as, and whether the function's role may write to it
        // ([`super::guest_file_system`]). It is what the HOST really attached, so a
        // configuration naming an access point this emulator's Amazon EFS does not hold is not
        // in it and that path is no path of the environment.
        "fileSystems": session.file_system_table(),
    });
    ::serde_json::to_string(&document).map_err(|error| error.to_string())
}

/// The document the per-invocation BIND call is handed: what ONE invocation brings to the
/// environment serving it, and nothing of the environment itself.
///
/// The modules, the handler and this runtime's own libraries are the ENVIRONMENT's
/// ([`runtime_input`]) and are not sent again: they are the same for every invocation one
/// environment serves.
fn invocation_input(request: &Run<'_>) -> Result<String, String> {
    let environment: ::serde_json::Map<String, ::serde_json::Value> = request
        .environment
        .iter()
        .map(|(name, value)| (name.clone(), ::serde_json::Value::String(value.clone())))
        .collect();
    // The `ClientContext` travels as the TEXT the caller sent (decoded from its base64),
    // because it is the RUNTIME that reads it: a managed runtime parses that document
    // itself and refuses an invocation whose document it cannot parse
    // ([`super::bootstrap`]), so a document this host parsed and dropped on a fault would
    // hand the handler no client context where the runtime raises.
    let document = ::serde_json::json!({
        "environment": environment,
        "event": request.event,
        "requestId": request.request_id,
        "invokedFunctionArn": request.invoked_function_arn,
        "clientContext": request.client_context,
        "tenantId": request.tenant_id,
        // The identity the invocation was made by, as above: it is ONE invocation's own, so
        // the environment serving a second invocation is bound that one's identity instead.
        "cognitoIdentity": crate::state::InvocationIdentity::runtime_member(
            request.cognito_identity,
        ),
    });
    ::serde_json::to_string(&document).map_err(|error| error.to_string())
}

/// The modules this RUNTIME supplies on the handler's import path, beside the deployment
/// package's own and the content the function's layers mount: the AWS SDK a managed
/// `python3.x` runtime ships ([`super::aws_sdk_guest`]), and the durable-execution
/// programming model it ships beside it ([`super::durable_sdk_guest`]).
fn runtime_modules() -> ::serde_json::Map<String, ::serde_json::Value> {
    super::aws_sdk_guest::runtime_modules()
        .into_iter()
        .chain(super::durable_sdk_guest::runtime_modules())
        .map(|module| (module.path, ::serde_json::Value::String(module.source)))
        .collect()
}

/// The modules of the STANDARD LIBRARY this runtime supplies in place of the frozen ones:
/// the socket module a Python program expects of its operating system, the HTTP client
/// built on one and the TLS layer beside it ([`super::guest_http`]), the waiting
/// interface the standard library's own `selectors` is built out of
/// ([`super::guest_select`]), the signal and child-process interfaces beside them
/// ([`super::guest_signal`], [`super::guest_process`]) and the database interface whose
/// statements run on the engine this crate embeds ([`super::guest_sqlite`]). Resolved before
/// the frozen standard
/// library and after the deployment package's own entries and the content the function's
/// layers mount, which is where a runtime's own standard library sits.
fn runtime_stdlib() -> ::serde_json::Map<String, ::serde_json::Value> {
    super::guest_http::runtime_stdlib_modules()
        .into_iter()
        .chain(super::guest_select::runtime_stdlib_modules())
        .chain(super::guest_signal::runtime_stdlib_modules())
        .chain(super::guest_process::runtime_stdlib_modules())
        .chain(super::guest_sqlite::runtime_stdlib_modules())
        .chain(super::guest_zoneinfo::runtime_stdlib_modules())
        .chain(super::guest_sysconfig::runtime_stdlib_modules())
        .chain(super::guest_pipes::runtime_stdlib_modules())
        .map(|module| (module.path, ::serde_json::Value::String(module.source)))
        .collect()
}

/// Install the trace hook the step and deadline bounds are checked from, then take away
/// every way the guest could clear or replace it.
///
/// The hook is a HOST function, not Python: what the interpreter calls on every traced event
/// is [`Bounds::step`] itself, reached through no name the guest can rebind — there is no
/// module attribute, no global and no default argument between the event and the bound. The
/// bounds it spends are the ones of the invocation the environment is SERVING ([`Session`]),
/// so an event of the invocation being served now is never charged to the one before it. A
/// trace function has to answer the local tracer to use, and the local tracer is this same
/// function, so the one it answers is read from `held` — the cell [`Prepared`] owns, which
/// stays filled until the interpreter has been FINALIZED, because a guest `atexit` callback
/// runs during finalization and its line events have to be traced too.
///
/// The stop the hook delivers is raised INSIDE the guest, which guest code may catch — so the
/// hook ARMS THE TRACE SLOT AGAIN on its way out. An interpreter stops tracing once a trace
/// function raises, so without that the stop this hook raises would be the last check of the
/// run: a handler that swallows it would go on running the host's thread with its budget spent
/// and nothing left to notice. Armed again, every event after the catch meets the latched stop
/// again ([`Bounds::step`] answers the latch), so the run keeps being stopped for as long as it
/// keeps running. The arming is itself guarded against re-entering the hook, so no event the
/// arming call delivers can start a second one.
fn install_trace_hook(
    vm: &VirtualMachine,
    session: &Arc<Session>,
    held: &TraceCell,
) -> PyResult<()> {
    let answered = ::std::rc::Rc::clone(held);
    let stepping = Arc::clone(session);
    // The call that arms the interpreter's trace slot, held for the hook to use. Filled from
    // the host below, before any guest code runs and before the name it was read off is taken
    // away, so no guest can reach it through this cell either.
    let arming: ::std::rc::Rc<::std::cell::RefCell<Option<PyObjectRef>>> =
        ::std::rc::Rc::new(::std::cell::RefCell::new(None));
    let armed = ::std::rc::Rc::clone(&arming);
    let arming_now = ::std::rc::Rc::new(::std::cell::Cell::new(false));
    let hook = vm.new_function(
        "__lambda_trace",
        move |_args: FuncArgs, vm: &VirtualMachine| {
            if let Some(stop) = stepping.bounds().step() {
                // The slot again, with this same hook, so a guest that catches what follows
                // is traced — and stopped — at its next event too.
                if !arming_now.replace(true) {
                    let arm = armed.borrow().clone();
                    let hook = answered.borrow().clone();
                    if let (Some(arm), Some(hook)) = (arm, hook) {
                        let _ = arm.call((hook,), vm);
                    }
                    arming_now.set(false);
                }
                return Err(vm.new_runtime_error(stop.reason().to_owned()));
            }
            // The local tracer for the frame this event belongs to is this same hook, so
            // every event of every frame spends the budget.
            let held = answered.borrow();
            Ok(held.clone().unwrap_or_else(|| vm.ctx.none()))
        },
    );
    let hook: PyObjectRef = hook.into();
    *held.borrow_mut() = Some(hook.clone());
    let setter = vm.sys_module.as_object().get_attr("settrace", vm)?;
    *arming.borrow_mut() = Some(setter.clone());
    setter.call((hook,), vm)?;
    // With these gone, no guest call can reach the interpreter's trace slot at all: the
    // hook that spends the budget is the only one there will be for this run. Each name is
    // read back, so one that could not be taken away refuses the run rather than leaving the
    // guest a call that clears every bound.
    for name in ["settrace", "setprofile", "gettrace", "getprofile"] {
        let _ = vm.sys_module.as_object().del_attr(name, vm);
        if vm.sys_module.as_object().get_attr(name, vm).is_ok() {
            return Err(vm.new_runtime_error(format!(
                "sys.{name} could not be taken away, so no function is run in this interpreter"
            )));
        }
    }
    Ok(())
}

/// Where the installed trace hook is held, for the life of one execution environment.
///
/// The hook answers ITSELF as each frame's local tracer, so this cell holds the hook and the
/// hook holds this cell. [`Prepared`] owns it and empties it once the interpreter has been
/// finalized — not before, because a guest `atexit` callback runs during finalization and has
/// to be traced like every other guest frame — which is what keeps one environment's hook from
/// outliving the environment that installed it.
type TraceCell = ::std::rc::Rc<::std::cell::RefCell<Option<PyObjectRef>>>;

/// Clamp how deep the guest may recurse, and take away the call that would raise it.
fn clamp_recursion(vm: &VirtualMachine, limit: usize) -> PyResult<()> {
    ::python_guest_substrate::clamp_recursion(vm, limit, REFUSED_RUN)
}

/// The module the host functions live on, and the whole of what guest code can call of
/// this process: `log`, `sleep`, `aws`, `db_auth_token`, `request`, `presign`, `pagination`,
/// `net`, `durable`, `ephemeral`, `files` and `library_source` — the invocation log, a sleep clamped to what the run has left, ONE AWS SDK
/// call into this same emulator, the authentication token an SDK mints in its client, the one
/// request a handler sends itself, the one URL it has signed, what the called service's
/// own model states about paging one operation through, the one call a handler makes on a
/// stream socket it opened to a data-plane listener of this emulator, one checkpoint of
/// the durable execution this invocation is the run of, what the run wrote to the
/// ephemeral storage its function is configured for, one call on a path of a file system the
/// function mounted, and the source of one module of the standard library this runtime ships.
/// Nothing else of the
/// host is reachable from guest code, and nothing here moves a bound.
///
/// Installed ONCE per execution environment, with its interpreter: every one of these
/// functions reads the invocation being served from the environment's [`Session`], so the log
/// it writes, the bounds it spends and the seam it reaches are the invocation's own however
/// many invocations the environment has served.
fn host_module(vm: &VirtualMachine, session: &Arc<Session>) -> PyResult<PyRef<PyModule>> {
    let module = vm.new_module(HOST_MODULE, vm.ctx.new_dict(), None);
    let logging = Arc::clone(session);
    set_function(vm, &module, "log", move |args: FuncArgs, vm: &VirtualMachine| {
        let text: String = args.bind(vm)?;
        captured(&logging.capture()).write(&text);
        Ok(vm.ctx.none())
    });
    // The same log, written a RECORD at a time: what this runtime's own logging handler
    // hands the platform is a whole record, so it is framed as one however many line breaks
    // its formatted message carries — a record carrying a traceback
    // ([`super::outcome::Capture::write_record`]).
    let framed = Arc::clone(session);
    set_function(
        vm,
        &module,
        "record",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let text: String = args.bind(vm)?;
            captured(&framed.capture()).write_record(&text);
            Ok(vm.ctx.none())
        },
    );
    // What the invocation has written to the function's EPHEMERAL STORAGE: the `/tmp` the
    // runtime's own filesystem serves is held in this process's memory, and this is how the run
    // reports the bytes it put there so that the memory ceiling is judged on the memory the
    // function allocated rather than on the disk it wrote. It moves no bound: what a run may
    // leave out is held both to the size its own configuration states and to the pool the runs
    // of this process share, and the grant goes back when the run ends
    // ([`Bounds::wrote_to_ephemeral_storage`]).
    let storing = Arc::clone(session);
    set_function(
        vm,
        &module,
        "ephemeral",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let written: u64 = args.bind(vm)?;
            storing.bounds().wrote_to_ephemeral_storage(written);
            Ok(vm.ctx.none())
        },
    );
    let sleeping = Arc::clone(session);
    set_function(
        vm,
        &module,
        "sleep",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let seconds: f64 = args.bind(vm)?;
            // A sleep the guest asked for never outlasts the run: it is clamped to what
            // the run has left, so no guest call can wait past the deadline.
            bounded_sleep(&sleeping.bounds(), seconds);
            Ok(vm.ctx.none())
        },
    );
    // Where the INIT phase ends and the handler's own run begins is the HOST's to decide,
    // so there is no name for it on this module at all: [`served`] binds the invocation and
    // begins the invoke phase before calling the runtime ([`Bounds::start`]).
    // The hook that spends the step, deadline and memory budget is NOT here either: the
    // interpreter calls it directly ([`install_trace_hook`]), so there is no name on this
    // module for a guest to rebind and no way for guest code to reach the bounds at all.
    // The ONE call a handler reaches another service of this emulator by. The service
    // table, the signature and the destination are all [`super::aws_api`]'s, so what the
    // guest decides is only which operation of which reachable service it asks for and
    // what it sends; the two bounds that are this run's — how many calls it may make and
    // how long one may take — are held here, where no guest code can reach them.
    let calling = Arc::clone(session);
    set_function(
        vm,
        &module,
        "aws",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let (service, operation, region, body, credentials, endpoint): (
                String,
                String,
                String,
                String,
                OptionalArg<String>,
                OptionalArg<String>,
            ) = args.bind(vm)?;
            let call_bounds = calling.bounds();
            // A run whose budget is already spent makes no further call: the latched stop
            // is its answer, so work it starts afterwards is work nothing will report.
            if let Some(stop) = call_bounds.stopped() {
                return Err(vm.new_runtime_error(stop.reason().to_owned()));
            }
            // How many calls this INVOCATION has made, counted once for every way it can
            // make one: an SDK call and a request the handler sends itself spend the same
            // budget, and each invocation an environment serves has a budget of its own.
            if !calling.may_call() {
                return Err(vm.new_runtime_error(format!(
                    "one invocation may make at most {MAX_CALLS_PER_RUN} AWS SDK calls"
                )));
            }
            let Some(access) = calling.aws() else {
                return Ok(vm
                    .ctx
                    .new_str(refusal_answer(
                        "not_made",
                        "this invocation was given no AWS SDK seam, so no call was made",
                    ))
                    .into());
            };
            // Signed as the credentials the SDK CLIENT this call was made through was built
            // with, where it was built with any, and otherwise as the invocation's own
            // identity ([`signing_as_client`]).
            let access = match signing_as_client(access, credentials.as_ref().map(String::as_str).into()) {
                Ok(access) => access,
                Err(detail) => {
                    return Ok(vm.ctx.new_str(refusal_answer("not_made", &detail)).into())
                }
            };
            // Addressed at the endpoint the SDK CLIENT this call was made through was built
            // with, where it was built with one: an API whose resource is named only by its
            // endpoint is addressed nowhere else
            // ([`super::aws_api::ClientEndpoint`]).
            let addressed: Option<&str> = endpoint.as_ref().map(String::as_str).into();
            let access = access.addressed_by_client(
                addressed.and_then(super::aws_api::ClientEndpoint::configured),
            );
            let remaining = call_bounds.remaining();
            if remaining.is_zero() {
                return Err(vm.new_runtime_error(Stop::Deadline.reason().to_owned()));
            }
            let answered = access.call(&service, &operation, &region, &body, remaining);
            Ok(vm.ctx.new_str(call_answer(&answered)).into())
        },
    );
    // The AUTHENTICATION TOKEN an SDK mints in the CLIENT for a database login
    // ([`super::db_auth_token`]). It is not a call — nothing is dispatched and no service
    // answers — but it is signed with the credential this seam holds, which is why the host
    // mints it and the guest only states which endpoint and which database user it is for.
    // It counts against the same per-run budget as a call, so no run can mint tokens
    // without end.
    let minting = Arc::clone(session);
    set_function(
        vm,
        &module,
        "db_auth_token",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let (host, port, user, region, credentials): (
                String,
                u16,
                String,
                String,
                OptionalArg<String>,
            ) = args.bind(vm)?;
            if let Some(stop) = minting.bounds().stopped() {
                return Err(vm.new_runtime_error(stop.reason().to_owned()));
            }
            if !minting.may_call() {
                return Err(vm.new_runtime_error(format!(
                    "one invocation may make at most {MAX_CALLS_PER_RUN} AWS SDK calls"
                )));
            }
            let Some(access) = minting.aws() else {
                return Ok(vm
                    .ctx
                    .new_str(refusal_answer(
                        "not_made",
                        "this invocation was given no AWS SDK seam, so no authentication \
                         token was minted",
                    ))
                    .into());
            };
            let access = match signing_as_client(access, credentials.as_ref().map(String::as_str).into()) {
                Ok(access) => access,
                Err(detail) => {
                    return Ok(vm.ctx.new_str(refusal_answer("not_made", &detail)).into())
                }
            };
            let answer = match access.db_auth_token(&host, port, &user, &region) {
                Ok(token) => ::serde_json::json!({ "token": token }).to_string(),
                Err(Refusal::NotMade(detail)) => refusal_answer("not_made", &detail),
                Err(_other) => refusal_answer(
                    "not_made",
                    "the authentication token could not be minted",
                ),
            };
            Ok(vm.ctx.new_str(answer).into())
        },
    );
    // The ONE request a handler SENDS ITSELF, for a service whose data plane is reached
    // over HTTP rather than through an SDK operation. The address it may name, the
    // signature and the destination are all [`super::guest_http`]'s; the two bounds that
    // are this run's — the call count it shares with the SDK seam above and how long one
    // request may take — are held here, where no guest code can reach them.
    let sending = Arc::clone(session);
    set_function(
        vm,
        &module,
        "request",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let (method, url, headers, body): (String, String, String, String) = args.bind(vm)?;
            let request_bounds = sending.bounds();
            if let Some(stop) = request_bounds.stopped() {
                return Err(vm.new_runtime_error(stop.reason().to_owned()));
            }
            if !sending.may_call() {
                return Err(vm.new_runtime_error(format!(
                    "one invocation may make at most {MAX_CALLS_PER_RUN} calls into this \
                     emulator"
                )));
            }
            let Some(access) = sending.aws() else {
                return Ok(vm
                    .ctx
                    .new_str(refusal_answer(
                        "not_made",
                        "this invocation was given no seam to send a request on, so none was \
                         sent",
                    ))
                    .into());
            };
            let remaining = request_bounds.remaining();
            if remaining.is_zero() {
                return Err(vm.new_runtime_error(Stop::Deadline.reason().to_owned()));
            }
            let answered = access.send(&method, &url, &headers, &body, remaining);
            // What one request came to is the same document for every runtime's own HTTP
            // client ([`super::guest_http::answer_document`]), which this module's
            // `http.client` reads.
            Ok(vm
                .ctx
                .new_str(super::guest_http::answer_document(&answered))
                .into())
        },
    );
    // The URL a handler asks the SDK to PRESIGN. Nothing is sent, so this spends neither
    // the call count nor the deadline; the operation's own binding and the signature are
    // host code's ([`super::aws_api::presigned`]), so what the guest decides is only which
    // operation of which reachable service the URL names.
    let presigning = Arc::clone(session);
    set_function(
        vm,
        &module,
        "presign",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let (service, operation, region, body, expires, credentials): (
                String,
                String,
                String,
                String,
                u64,
                OptionalArg<String>,
            ) = args.bind(vm)?;
            let Some(access) = presigning.aws() else {
                return Ok(vm
                    .ctx
                    .new_str(refusal_answer(
                        "not_made",
                        "this invocation was given no AWS SDK seam, so no URL was signed",
                    ))
                    .into());
            };
            let access = match signing_as_client(access, credentials.as_ref().map(String::as_str).into()) {
                Ok(access) => access,
                Err(detail) => {
                    return Ok(vm.ctx.new_str(refusal_answer("not_made", &detail)).into())
                }
            };
            let signed = access.presigned(
                &service,
                &operation,
                &region,
                &body,
                Duration::from_secs(expires),
            );
            let answered = match signed {
                Ok(url) => ::serde_json::json!({ "url": url }).to_string(),
                Err(refused) => refusal_answer_for(&refused),
            };
            Ok(vm.ctx.new_str(answered).into())
        },
    );
    // The calls a handler makes on a SOCKET of its own: one on a STREAM socket, for a data
    // plane whose wire protocol is its own rather than HTTP, and the route lookup and the
    // datagram of a function attached to a VPC. WHERE a connection may be opened, the
    // connections themselves, which addresses a datagram reaches and how many bytes one
    // invocation may carry are all [`super::guest_socket`]'s; the two bounds that are this
    // run's — how many calls it may make when it OPENS one, and how long any one call may wait
    // — are held here, where no guest code can reach them.
    let dialing = Arc::clone(session);
    set_function(
        vm,
        &module,
        "net",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let (verb, document): (String, String) = args.bind(vm)?;
            let call_bounds = dialing.bounds();
            if let Some(stop) = call_bounds.stopped() {
                return Err(vm.new_runtime_error(stop.reason().to_owned()));
            }
            // OPENING a connection is one of the calls an invocation is allowed; carrying
            // bytes on one that is already open is not, because a wire protocol's session is
            // many reads and writes on the one connection.
            if verb == "connect" && !dialing.may_call() {
                return Err(vm.new_runtime_error(format!(
                    "one invocation may make at most {MAX_CALLS_PER_RUN} calls into this \
                     emulator"
                )));
            }
            let remaining = call_bounds.remaining();
            if remaining.is_zero() {
                return Err(vm.new_runtime_error(Stop::Deadline.reason().to_owned()));
            }
            let answered = dialing.socket_call(&verb, &document, remaining).unwrap_or_else(
                || {
                    ::serde_json::json!({
                        "error": {
                            "kind": "other",
                            "errno": ::serde_json::Value::Null,
                            "message": "this invocation was given no seam to open a \
                                        connection on, so none was opened",
                        }
                    })
                    .to_string()
                },
            );
            Ok(vm.ctx.new_str(answered).into())
        },
    );
    // ONE CALL a handler makes on a DATABASE of this execution environment, which the
    // `sqlite3` module of this runtime's standard library performs
    // ([`super::guest_sqlite`]). The databases, the engine's own containment and the bounds on
    // one statement are all that module's; the bound that is this run's — a statement never
    // outlives the invocation that asked for it — is held here, where no guest code can reach
    // it. It is no call into another service, so it spends none of the per-run call count.
    let querying = Arc::clone(session);
    set_function(
        vm,
        &module,
        "sqlite",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let (verb, document): (String, String) = args.bind(vm)?;
            let call_bounds = querying.bounds();
            // A run whose budget is already spent runs no further statement: the latched stop
            // is its answer, so work it starts afterwards is work nothing will report.
            if let Some(stop) = call_bounds.stopped() {
                return Err(vm.new_runtime_error(stop.reason().to_owned()));
            }
            let remaining = call_bounds.remaining();
            if remaining.is_zero() {
                return Err(vm.new_runtime_error(Stop::Deadline.reason().to_owned()));
            }
            Ok(vm
                .ctx
                .new_str(querying.database_call(&verb, &document, remaining))
                .into())
        },
    );
    // ONE reading of a TIME ZONE the handler named, which the `zoneinfo` package of this
    // runtime's standard library performs ([`super::guest_zoneinfo`]): the zones, and every
    // offset, daylight-saving reading and abbreviation of one, are the compiled-in IANA
    // database's. Nothing is dispatched, no host path is opened and no bound is spent: it is
    // the environment's own database, read as data.
    set_function(
        vm,
        &module,
        "zone",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let (verb, named): (String, String) = args.bind(vm)?;
            Ok(vm
                .ctx
                .new_str(super::guest_zoneinfo::answered(&verb, &named))
                .into())
        },
    );
    // ONE CHECKPOINT of the durable execution this invocation is the run of: the runtime's
    // own durable-execution SDK reports each step it starts, succeeds at or fails an attempt
    // of through here, and the HOST is what writes it onto that execution's history
    // ([`super::super::durable_steps`]). Nothing is dispatched and no service answers, so
    // this spends neither the call count nor the deadline; what the guest decides is only
    // what its own run DID. An invocation that is no durable execution's run has no
    // execution to record on, so its checkpoints record nothing.
    //
    // The one bound that is this run's is held HERE, where no guest code can reach it, for the
    // reason every other guest-to-host seam of this module holds one: a handler decides what it
    // checkpoints, so a document larger than one checkpoint may carry
    // ([`DurableStepJournal`]'s own ceiling) is REFUSED rather than written, and one invocation
    // cannot turn a step into an unbounded write.
    let checkpointing = Arc::clone(session);
    set_function(
        vm,
        &module,
        "durable",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let document: String = args.bind(vm)?;
            if document.len() > super::super::durable_steps::MAX_CHECKPOINT_BYTES {
                return Err(vm.new_runtime_error(format!(
                    "a durable execution's checkpoint may carry at most {} bytes",
                    super::super::durable_steps::MAX_CHECKPOINT_BYTES
                )));
            }
            if let Some(journal) = checkpointing.durable() {
                journal.record(&document);
            }
            Ok(vm.ctx.none())
        },
    );
    // ONE CALLBACK of that same durable execution: the wait the handler takes for something
    // OUTSIDE the execution to finish ([`super::super::durable_callbacks`]). The run MINTS the
    // callback through here — which is what appends the `CallbackStarted` event the id travels
    // in — and reads what that callback has come to while it waits. Nothing is dispatched and
    // no service answers, so this spends neither the call count nor the deadline; what the
    // guest decides is only which of its own waits it is asking about. The bound is the one a
    // checkpoint carries, held where no guest code can reach it.
    let waiting = Arc::clone(session);
    set_function(
        vm,
        &module,
        "durable_callback",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let document: String = args.bind(vm)?;
            // The one bound this seam has, held here where no guest code can reach it, exactly
            // as the checkpoint seam above holds it: a handler decides what it asks about, so
            // a document larger than one checkpoint may carry is REFUSED rather than written.
            if document.len() > super::super::durable_steps::MAX_CHECKPOINT_BYTES {
                return Err(vm.new_runtime_error(format!(
                    "a durable execution's callback may carry at most {} bytes",
                    super::super::durable_steps::MAX_CHECKPOINT_BYTES
                )));
            }
            let Some(journal) = waiting.durable() else {
                // An invocation that is no durable execution's run has no execution to mint a
                // callback on, and says so rather than answering an id nothing can signal.
                return Ok(vm
                    .ctx
                    .new_str(
                        ::serde_json::json!({
                            "refused": "this invocation is no durable execution's run, so it \
                                        has no callback to wait for",
                        })
                        .to_string(),
                    )
                    .into());
            };
            let answered = journal.callback(&document);
            // The WAIT between two readings of a callback that is still OPEN is the host's, and
            // it is the EXECUTION's wait rather than this run's: a durable execution suspends
            // while it waits for something outside itself, so the span is charged to neither
            // the function's `Timeout` nor this host's ceiling on one run
            // ([`Bounds::suspend`]) — which is what lets a callback signalled, or a window
            // closed, long after the invocation began still reach the handler waiting on it.
            //
            // The answer SAYS whether that wait was taken, because the reader is a loop: a
            // reading this host did not wait inside is one the guest waits after itself
            // ([`super::durable_callbacks::answer_waited`]), so no number of readings can spin.
            if super::super::durable_callbacks::answer_is_still_waiting(&answered) {
                let waited = waiting.bounds().suspend(CALLBACK_POLL_SPAN);
                return Ok(vm
                    .ctx
                    .new_str(super::super::durable_callbacks::answer_waited(
                        &answered, waited,
                    ))
                    .into());
            }
            Ok(vm.ctx.new_str(answered).into())
        },
    );
    // What the SDK's own PAGINATOR for one operation drives: the input member a continuation
    // value is sent back in, the output member it arrives in, the member a page's size is
    // asked for and the members a page's results arrive in, read out of the CALLED service's
    // own model ([`super::sdk_model`]). Nothing is dispatched and no service answers, so this
    // spends neither the call count nor the deadline; what the guest decides is only which
    // operation of which service it asks about.
    set_function(
        vm,
        &module,
        "pagination",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let (service, operation): (String, String) = args.bind(vm)?;
            Ok(vm
                .ctx
                .new_str(super::aws_sdk_guest::pagination_config(&service, &operation))
                .into())
        },
    );
    // What the SDK's own EXCEPTION CLASSES are: the error shapes the called service's model
    // declares, read out of that service's own model ([`super::sdk_model`]), so a refusal is
    // raised as the class its service really has for it and a code no shape is declared for is
    // raised as the SDK's base class. Asked about ONE operation, it answers the errors that
    // operation can be answered with, which is what decides the class of a refusal of a call of
    // it. Nothing is dispatched and no service answers, so this spends neither the call count
    // nor the deadline; what the guest decides is only which service and operation it asks
    // about.
    set_function(
        vm,
        &module,
        "error_shapes",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let (service, operation): (String, String) = args.bind(vm)?;
            // The seam is string-in, string-out, so the guest names no operation by sending an
            // empty one: that is the question about the SERVICE's whole list of classes.
            let operation = Some(operation.as_str()).filter(|named| !named.is_empty());
            Ok(vm
                .ctx
                .new_str(super::aws_sdk_guest::error_shapes(&service, operation))
                .into())
        },
    );
    // The SOURCE of one module of the standard library this RUNTIME ships
    // ([`super::python_library`]). The interpreter embedded here imports that library from a
    // frozen copy — compiled code with no text behind it — so the text is held by the host and
    // answered for the module a guest names. Nothing is dispatched, no host path is opened and
    // no bound is spent: it is the environment's own library, the same text the frozen code was
    // compiled from.
    //
    // The RUNTIME does not read it: a frame standing in a frozen module names that module and
    // carries no source text, exactly as a managed runtime serving its library frozen renders
    // one ([`super::bootstrap`]'s `_frame_origin`). This seam is what the environment answers a
    // GUEST that asks for a library module's own text with.
    set_function(
        vm,
        &module,
        "library_source",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let named: String = args.bind(vm)?;
            // The seam is string-in, string-out: a name this library holds no module under
            // answers the empty text, which is the library holding no such module.
            Ok(vm
                .ctx
                .new_str(super::python_library::library_source(&named).unwrap_or_default())
                .into())
        },
    );
    // The ONE PROGRAM a handler starts as a CHILD. Which program the execution environment
    // has, how it is resolved and what performing it comes to are all
    // [`super::guest_child_process`]'s; the bound that is this run's — how many children one
    // invocation may start — is held here, where no guest code can reach it, because a handler
    // decides how many it asks for.
    let starting = Arc::clone(session);
    set_function(
        vm,
        &module,
        "child",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let document: String = args.bind(vm)?;
            let child_bounds = starting.bounds();
            // A run whose budget is already spent starts nothing further: the latched stop is
            // its answer, so work it starts afterwards is work nothing will report.
            if let Some(stop) = child_bounds.stopped() {
                return Err(vm.new_runtime_error(stop.reason().to_owned()));
            }
            if !starting.may_start_a_child() {
                return Err(vm.new_runtime_error(format!(
                    "one invocation may start at most {MAX_CHILDREN_PER_RUN} child processes"
                )));
            }
            Ok(vm
                .ctx
                .new_str(super::guest_child_process::answered(&document))
                .into())
        },
    );
    // ONE CALL on a path of a FILE SYSTEM this function has mounted
    // ([`super::guest_file_system`]): the Amazon EFS access points its `FileSystemConfigs`
    // name. WHERE those mounts are, what is in them and what this function's role may do to
    // them are all the host's — the files are the file system's own, held by the service that
    // owns it — so what the guest decides is only which call it makes on which path of its own
    // mounts. Nothing is dispatched to another service and no bound of the run is reachable
    // from here: every write is bounded by the file system's own bounds.
    let reading = Arc::clone(session);
    set_function(
        vm,
        &module,
        "files",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let document: String = args.bind(vm)?;
            Ok(vm
                .ctx
                .new_str(reading.file_system_call(&document))
                .into())
        },
    );
    // CHECKED, once for the whole surface: the runtime source is driven through these
    // functions, so one that did not take its place is reported here rather than left for
    // guest code to meet as a missing name.
    for name in HOST_MODULE_FUNCTIONS {
        if module.as_object().get_attr(*name, vm).is_err() {
            return Err(vm.new_runtime_error(format!(
                "the host function {HOST_MODULE}.{name} could not be installed, so no function \
                 is run in this interpreter"
            )));
        }
    }
    Ok(module)
}

/// The module a SPARK program's STATEMENTS are planned and run through: one statement in, what
/// the engine answered out.
///
/// The ENGINE is the calling service's (`service_common::spark_runtime::SessionSql`) — it holds
/// the catalog the names a statement carries resolve in and the data behind them — and a run
/// reaches it through the seam the invocation was given, exactly as it reaches the rest of this
/// emulator. A run given no engine is answered the refusal that says so, so a program that
/// submits a statement is told nothing ran rather than handed a value nothing computed.
///
/// The bounds that are the RUN's are held here, where no guest code can reach them: a run whose
/// budget is spent runs no further statement, and one statement spends one of the calls one run
/// may make, because running it reaches the catalog and the store the data lives in.
fn spark_host_module(vm: &VirtualMachine, session: &Arc<Session>) -> PyResult<PyRef<PyModule>> {
    let module = vm.new_module(SPARK_HOST_MODULE, vm.ctx.new_dict(), None);
    let querying = Arc::clone(session);
    set_function(
        vm,
        &module,
        ::service_common::spark_runtime::HOST_QUERY_FUNCTION,
        move |args: FuncArgs, vm: &VirtualMachine| {
            let document: String = args.bind(vm)?;
            let bounds = querying.bounds();
            if let Some(stop) = bounds.stopped() {
                return Err(vm.new_runtime_error(stop.reason().to_owned()));
            }
            if bounds.remaining().is_zero() {
                return Err(vm.new_runtime_error(Stop::Deadline.reason().to_owned()));
            }
            if !querying.may_call() {
                return Err(vm.new_runtime_error(format!(
                    "one invocation may make at most {MAX_CALLS_PER_RUN} calls into this \
                     emulator"
                )));
            }
            let engine = querying
                .aws()
                .and_then(|access| access.session_sql().cloned());
            let answered = match engine {
                Some(engine) => engine.answered(&document),
                None => NO_ENGINE_ANSWER.to_string(),
            };
            Ok(vm.ctx.new_str(answered).into())
        },
    );
    // CHECKED, for the reason the surface above is: a function that did not take its place is
    // reported here rather than left for guest code to meet as a missing name.
    if module
        .as_object()
        .get_attr(::service_common::spark_runtime::HOST_QUERY_FUNCTION, vm)
        .is_err()
    {
        return Err(vm.new_runtime_error(format!(
            "the host function {SPARK_HOST_MODULE}.{} could not be installed, so no program is \
             run in this interpreter",
            ::service_common::spark_runtime::HOST_QUERY_FUNCTION
        )));
    }
    Ok(module)
}

/// What a run that was given NO engine answers a statement with: the refusal shape that seam
/// carries, saying nothing ran — never an empty answer, which would say the statement ran and
/// found nothing.
const NO_ENGINE_ANSWER: &str = concat!(
    r#"{"error":{"class":"SparkException","message":"this program was given no engine to plan "#,
    r#"and run a statement on, so none was run"}}"#,
);

/// The whole of what guest code can call of this process ([`host_module`]), which is also
/// what that function checks it really installed.
const HOST_MODULE_FUNCTIONS: &[&str] = &[
    "log",
    "record",
    "sleep",
    "aws",
    "child",
    "db_auth_token",
    "request",
    "presign",
    "pagination",
    "error_shapes",
    "net",
    "sqlite",
    "zone",
    "durable",
    "durable_callback",
    "ephemeral",
    "files",
    "library_source",
];

/// What the guest reads back from one AWS SDK call: the service's own answer, or the
/// reason no call was made.
///
/// A document rather than a Python object, for the reason the runtime's own protocol is one
/// ([`super::bootstrap`]): the host's interpreter surface stays a string in and a string
/// out.
fn call_answer(answered: &Result<super::aws_api::Answer, Refusal>) -> String {
    match answered {
        Ok(answer) => ::serde_json::json!({
            "status": answer.status,
            "body": answer.body,
            "errorType": answer.error_type,
            "queryErrorCode": answer.query_error_code,
            "requestId": answer.request_id,
            "errorMembers": answer.error_members,
            "streams": answer.streams,
            "blobs": answer.blobs,
            "payloadBlobs": answer.payload_blobs,
        })
        .to_string(),
        Err(refused) => refusal_answer_for(refused),
    }
}

/// The answer for a call or a signature this seam did not make, naming which refusal it
/// was. Shared by every host call the guest makes, so one refusal is reported one way.
fn refusal_answer_for(refused: &Refusal) -> String {
    match refused {
        Refusal::UnknownService => refusal_answer(
            "unknown_service",
            &super::aws_api::reachable_services().join(", "),
        ),
        Refusal::UnknownOperation => refusal_answer("unknown_operation", ""),
        Refusal::TimedOut => refusal_answer("timed_out", ""),
        Refusal::RequestTooLarge => refusal_answer(
            "not_made",
            &format!(
                "the request document is larger than the {} bytes one AWS SDK call may carry",
                super::aws_api::MAX_REQUEST_BYTES
            ),
        ),
        Refusal::ResponseTooLarge => refusal_answer(
            "not_made",
            &format!(
                "the answer is larger than the {} bytes one AWS SDK call may read back",
                super::aws_api::MAX_RESPONSE_BYTES
            ),
        ),
        Refusal::NotMade(detail) => refusal_answer("not_made", detail),
    }
}

/// The answer for a call that was not made, naming which refusal it was.
fn refusal_answer(refused: &str, detail: &str) -> String {
    ::serde_json::json!({ "refused": refused, "detail": detail }).to_string()
}

/// The access ONE call of an SDK client the guest built is made with: the seam the invocation
/// was given, signing as the credentials THAT CLIENT was built with where it was built with
/// any, and otherwise as the identity the invocation itself was given.
///
/// Every AWS SDK signs a call with the credentials its client holds, so a handler that was
/// vended a session — by assuming another role, for instance — and built a client with it is
/// calling as that session and is judged on the policies in force on it
/// ([`CallerIdentity::supplied`]). `Err` carries why a document that is not a credential at
/// all made no call, so such a call is never quietly signed as the invocation's own identity.
fn signing_as_client(access: AwsAccess, credentials: Option<&str>) -> Result<AwsAccess, String> {
    match CallerIdentity::supplied(credentials.unwrap_or_default())? {
        Some(identity) => Ok(access.signing_as(Some(identity))),
        None => Ok(access),
    }
}

/// The names Python's `os` looks for on the module it is built on, every one of which
/// lands on the HOST where a real runtime answers it: a file, a directory, a descriptor, a
/// process, a user or the host clock's own accounting.
///
/// Each one EXISTS on this emulator's `posix` (`os` builds itself out of the names it
/// finds, so an absent one changes what `os` offers) and each one RAISES when it is
/// called: a Lambda function in this emulator has no host filesystem and no host process
/// to reach. Nothing here is a stub that pretends to have done the work.
///
/// The names that would end THIS PROCESS are refused here too (`_exit`, `abort`), beside the
/// ones that would start or signal another (`fork`, `execv`, `posix_spawn`, `kill`) and the ones
/// that would change what this process RUNS AS or what it can see of the host (`setuid`,
/// `setresuid`, `initgroups`, `chroot`): the module this list narrows carries no such call
/// today, and a refusal installed under the name is what keeps that true of an interpreter
/// release that adds one — a guest call that started a host process or ended this one would
/// take every other invocation's run and every tenant's state with it, past every bound a run is
/// held to. Every POSIX spelling of each is named, because a name absent from this list is a
/// name whose call would be whatever the module carries under it: the spawn pair beside the
/// `exec` and `fork` families, `fexecve` beside `execve`, `pipe2` beside `pipe` and `dup3`
/// beside `dup2`. What a run may do to its own ENVIRONMENT is not on this list and is not
/// host-reaching: the environment mapping, the working directory it reports and the random
/// bytes it reads are answered from the environment's own memory ([`super::host_posix`]), which
/// is also where the fork registration a library makes is answered.
const REFUSED_POSIX_CALLS: &[&str] = &[
    "_exit", "abort",
    "access", "chdir", "chmod", "chown", "chroot", "close", "closerange", "confstr", "cpu_count",
    "device_encoding", "dup", "dup2", "dup3",
    "execv", "execve", "fchdir", "fchmod", "fchown", "fdatasync", "fdopen", "fexecve", "fork",
    "forkpty",
    "fstat", "fstatvfs", "fsync", "ftruncate", "get_blocking", "get_inheritable",
    "get_terminal_size", "getegid", "geteuid", "getgid", "getgrouplist", "getgroups", "getlogin",
    "getppid", "getpriority", "getresgid", "getresuid", "getsid", "getuid", "initgroups",
    "isatty", "kill", "killpg", "lchown", "link",
    "listdir", "lseek", "lstat", "major", "makedev", "minor", "mkdir", "mkfifo", "mknod", "nice",
    "open", "openpty", "pipe", "pipe2", "popen", "posix_spawn", "posix_spawnp", "pread",
    "putenv_host", "pwrite", "read", "readlink",
    "readv", "remove", "rename", "replace", "rmdir", "scandir", "sched_yield", "sendfile",
    "set_blocking", "set_inheritable", "setegid", "seteuid", "setgid", "setgroups", "setpgid",
    "setpgrp", "setpriority", "setregid", "setresgid", "setresuid", "setreuid", "setsid",
    "setuid", "spawnv", "spawnve",
    "pathconf", "fpathconf", "process_cpu_count", "sched_getaffinity", "sched_setaffinity",
    "sysconf", "waitstatus_to_exitcode", "lockf", "getrandom",
    "stat", "statvfs", "symlink", "sync", "system", "tcgetpgrp", "tcsetpgrp", "times", "truncate",
    "ttyname", "umask", "unlink", "utime", "wait", "wait3", "wait4", "waitpid", "write",
    "writev",
];

/// Put a refusal on `posix` under every name of [`REFUSED_POSIX_CALLS`].
///
/// The module itself is this crate's ([`super::host_posix`]) and carries only the calls a
/// run can be answered from its own memory; these are the names `os` looks for beyond
/// those, so that `os` is built out of a complete surface and every one of them raises.
fn narrow_posix(vm: &VirtualMachine) -> PyResult<()> {
    let system = vm.import("posix", 0)?;
    // `os` builds the sets that say which of its calls take a directory descriptor or
    // follow symbolic links out of this list. It is EMPTY, which is what says that none of
    // them does: there is no host descriptor and no host link to follow.
    system.set_attr("_have_functions", vm.ctx.new_list(Vec::new()), vm)?;
    for name in REFUSED_POSIX_CALLS {
        let refusing = vm.new_function(name, |_args: FuncArgs, vm: &VirtualMachine| {
            Err::<PyObjectRef, _>(vm.new_os_error(
                "a Lambda function in this emulator has no host filesystem or process to reach"
                    .to_owned(),
            ))
        });
        let refusing: PyObjectRef = refusing.into();
        // Checked, like every containment patch here: a name that kept its host-reaching
        // call refuses the run rather than being left reachable.
        system.set_attr(*name, refusing.clone(), vm)?;
        installed_refusal(vm, &system, name, &refusing, "posix")?;
    }
    Ok(())
}

/// Take away every call that would start a HOST thread, on the single `_thread` module
///
/// A real guest thread would run outside the step budget, the stop latch and the memory
/// sample, so none can be started. The build registers no native thread module at all (the
/// engine feature that would is not taken, see this crate's `Cargo.toml`), so the one a
/// guest imports is the engine's own thread-less stub; these refusals are what make that
/// explicit at every name that starts something.
fn refuse_guest_threads(vm: &VirtualMachine) -> PyResult<()> {
    ::python_guest_substrate::refuse_guest_threads(
        vm,
        "a Lambda function in this emulator cannot start a thread",
        REFUSED_RUN,
    )?;
    install_thread_local(vm)?;
    Ok(())
}

/// Install the THREAD-LOCAL type of that same `_thread` module, which is what
/// `threading.local` is and what the standard library's own modules keep their per-thread
/// state in.
///
/// An invocation runs on ONE thread — a handler can start none ([`refuse_guest_threads`]) — so
/// the data one thread holds is the data the object holds, which is ordinary attribute lookup:
/// an attribute SET on the object is read back off it, and one the object does not carry falls
/// back to the CLASS's own, exactly as a thread-local does for a subclass that declares a
/// default (`asyncio.events`'s own `_RunningLoop.loop_pid` is such a default, and the engine's
/// thread-less stub answers an `AttributeError` for it instead).
fn install_thread_local(vm: &VirtualMachine) -> PyResult<()> {
    let threads = vm.import("_thread", 0)?;
    for name in ["local", "_local"] {
        let built = super::host_posix::built_type(vm, "local", THREAD_LOCAL_SOURCE)?;
        threads.set_attr(name, built, vm)?;
        if threads.get_attr(name, vm).is_err() {
            return Err(vm.new_runtime_error(format!(
                "the thread-local type _thread.{name} could not be installed, so no function is \
                 run in this interpreter"
            )));
        }
    }
    Ok(())
}

/// `_thread.local`: the type the standard library keeps per-thread state in; see
/// [`install_thread_local`].
const THREAD_LOCAL_SOURCE: &str = r#"
class local:
    """Data held for one thread, of which an invocation runs exactly one."""

    def __new__(cls, *args, **kwargs):
        if (args or kwargs) and cls.__init__ is local.__init__:
            raise TypeError("Initialization arguments are not supported")
        return object.__new__(cls)
"#;

/// Put one host function on `module` under `name`.
///
/// Whether every one of them really took its place is checked once, by [`host_module`], which
/// refuses the run when one did not.
fn set_function<F>(
    vm: &VirtualMachine,
    module: &PyRef<PyModule>,
    name: &'static str,
    function: F,
) where
    F: Fn(FuncArgs, &VirtualMachine) -> PyResult<PyObjectRef> + Send + Sync + 'static,
{
    let callable = vm.new_function(name, function);
    let _ = module.as_object().set_attr(name, callable, vm);
}

/// How an exception of the interpreter ITSELF is reported — never to a caller as a result,
/// only as the internal failure it is.
fn formatted_exception(
    vm: &VirtualMachine,
    exception: &::rustpython_vm::builtins::PyBaseExceptionRef,
) -> String {
    let mut rendered = String::new();
    if vm.write_exception(&mut rendered, exception).is_err() {
        rendered.push_str("an exception that could not be rendered");
    }
    rendered
}

/// The run's budget, as the hook spends it. Shared with the host functions and read by the
/// caller after the run.
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
    /// How many traced events are left.
    steps: AtomicU64,
    /// How much memory the run may add to this process.
    memory_bytes: u64,
    /// How large the function's EPHEMERAL STORAGE is, in bytes: the most of what this run has
    /// faulted in that can be the `/tmp` it wrote rather than its own memory
    /// ([`Self::wrote_to_ephemeral_storage`]).
    ephemeral_bytes: u64,
    /// How much of that this run has been GRANTED out of the process's own pool
    /// ([`EPHEMERAL_POOL`]), which is what the memory reading leaves out and what this run
    /// gives back when its bounds are dropped.
    ephemeral_granted: AtomicU64,
    /// How large the handler's response may be.
    response_bytes: usize,
    /// What this run holds of the host, measured from the instant the HOST marked as the
    /// run's own ([`Self::measure_memory_from_here`]) — which is what the run's own ceiling
    /// is judged on and what the run is reported to have used
    /// ([`Self::max_memory_bytes`]). Only the host marks that instant, so no guest can raise
    /// the baseline it is judged against.
    held: Held,
    /// Which bound was breached, once one was. Latched: the answer to a run that spent its
    /// budget is that stop, whatever the guest does afterwards.
    stopped: Mutex<Option<Stop>>,
    /// Whether a bound was breached at all, read without taking the lock.
    breached: AtomicBool,
    /// Whether the INVOKE phase has begun ([`Self::start`]), which is when the handler's
    /// own deadline starts running.
    spending: AtomicBool,
    /// How much of the span the handler's own WAITS asked for this host did not really wait
    /// out ([`Self::wait`]), in nanoseconds. A wait is charged to the function's `Timeout` in
    /// full, and this is the part of it no host time passed over.
    waived_waiting: AtomicU64,
    /// How much of [`super::GUEST_WAIT_HOST_SPAN`] the waits of this run have really spent,
    /// in nanoseconds.
    host_waiting: AtomicU64,
    /// How long this run has sat SUSPENDED on a callback of the durable execution it is the
    /// run of ([`Self::suspend`]), in nanoseconds — a span charged to neither the function's
    /// `Timeout` nor this host's ceiling on one run, and bounded by
    /// [`super::CALLBACK_SUSPENSION_CEILING`].
    suspended: AtomicU64,
    /// How much of [`super::INIT_PHASE_LIMIT`] the code OUTSIDE the handler has asked to wait
    /// for, in nanoseconds — the waits taken before the invoke phase began
    /// ([`Self::start`]).
    ///
    /// The INIT phase has a span of its own, which the function's `Timeout` does not bound, so
    /// what an initialization spends is counted here and judged against that span
    /// ([`Self::past_the_init_limit`]).
    init_waiting: AtomicU64,
}

impl Bounds {
    /// The bounds of a run with `budget`, in its INIT phase: the deadline is the one the
    /// initialization of the execution environment has, which is not the function's
    /// `Timeout` — the phase has a limit of its own ([`super::INIT_PHASE_LIMIT`]), and AWS
    /// retries an initialization that outlasts it under the configured function timeout, so
    /// what an initialization is given up on after is the two spans together. The handler's
    /// own deadline is taken by [`Self::start`], when the invoke phase begins.
    ///
    /// A run for which this emulator still has to BUILD an execution environment says so
    /// ([`Self::begin_provisioning`]) and has the initialization's own deadline read from the
    /// instant that environment is standing ([`Self::begin_initialization`]): the span this
    /// process spends on its own interpreter is a span of neither phase of the run.
    ///
    /// The MEMORY baselines are taken by [`Self::measure_memory_from_here`], once the
    /// interpreter this emulator builds is standing and before any of the run's own code is
    /// read — by the host, because a baseline a guest could re-take is no ceiling at all.
    fn new(budget: &Budget, clock: Clock) -> Self {
        Self {
            deadline: Mutex::new(
                clock.now()
                    + super::INIT_PHASE_LIMIT.saturating_add(clock.host_span(budget.wall_clock)),
            ),
            wall_clock: budget.wall_clock,
            clock,
            deadline_epoch_ms: ::std::sync::atomic::AtomicI64::new(0),
            steps: AtomicU64::new(budget.steps),
            memory_bytes: budget.memory_bytes,
            ephemeral_bytes: budget
                .ephemeral_bytes
                .min(super::MAX_EPHEMERAL_HELD_BYTES),
            ephemeral_granted: AtomicU64::new(0),
            response_bytes: budget.response_bytes,
            held: Held::new(),
            stopped: Mutex::new(None),
            breached: AtomicBool::new(false),
            spending: AtomicBool::new(false),
            waived_waiting: AtomicU64::new(0),
            host_waiting: AtomicU64::new(0),
            suspended: AtomicU64::new(0),
            init_waiting: AtomicU64::new(0),
        }
    }

    /// Measure this run's memory from HERE: the faulted pages of this run's own thread as they
    /// are now, which is what its ceiling judges the run's own growth against.
    ///
    /// Called by the HOST only ([`drive`]), once the interpreter is standing and before a
    /// line of the run's own code has been read, so what it measures is the memory the run
    /// itself takes and not the environment this emulator built for it. It is deliberately
    /// not on the module the guest can reach: a run that could re-take its own baseline
    /// could hold the ceiling's worth of memory again after every call.
    fn measure_memory_from_here(&self) {
        self.held.measure_from_here();
    }

    /// Begin STANDING THE ENVIRONMENT UP: from here the deadline is the span this emulator
    /// may spend on that work ([`super::ENVIRONMENT_BUILD_SPAN`]) and not a span of the run's
    /// own, because none of the function's code has been read yet — building the interpreter,
    /// narrowing its native surface and evaluating this emulator's own runtime is this
    /// process's work on whatever host it is running on, and the function's `Timeout` and the
    /// span an initialization is allowed bound the GUEST's doing instead.
    ///
    /// Called by the HOST, before the interpreter is built ([`Prepared::start`]), and
    /// reachable from no guest code: a run served in an environment that is already standing
    /// never reaches it, because there is nothing to build for it.
    fn begin_provisioning(&self) {
        *self.deadline.lock().unwrap_or_else(PoisonError::into_inner) =
            self.clock.now() + super::ENVIRONMENT_BUILD_SPAN;
    }

    /// Begin the INIT phase: the span an initialization is allowed runs from HERE — the
    /// instant the environment is standing and nothing of the run's own code has been read —
    /// so what the deadline measures is the module-level code of the deployment package
    /// rather than the environment this emulator built to read it in.
    ///
    /// The same instant the run's memory baseline is taken at
    /// ([`Self::measure_memory_from_here`]), and for the same reason: a span this process
    /// spent on its own interpreter is no more the run's doing than a page that interpreter
    /// faulted in. What the deadline then allows is the two spans [`Self::new`] states — the
    /// initialization's own and the function's `Timeout` it is retried under.
    ///
    /// Called by the HOST, from the one place that knows the environment is standing
    /// ([`initialize`]), and reachable from no guest code.
    fn begin_initialization(&self) {
        *self.deadline.lock().unwrap_or_else(PoisonError::into_inner) =
            self.clock.now() + super::INIT_PHASE_LIMIT.saturating_add(self.host_span());
    }

    /// Begin the INVOKE phase: the handler's deadline runs from HERE, so what the deadline
    /// measures is the handler's own work rather than the execution environment this
    /// emulator built to run it in — the interpreter, the runtime and the import of the
    /// handler's own module, none of which the function's `Timeout` bounds. Answers when
    /// that deadline falls, in milliseconds since the epoch, which is what the handler is
    /// told it has left.
    ///
    /// Called by the HOST, between the two calls it makes into the runtime ([`drive`]), and
    /// reachable from no guest code. It is ONE-SHOT and moves NOTHING but this one deadline
    /// besides: a second call answers the same instant, and no memory baseline, step count
    /// or latched stop is touched.
    fn start(&self) -> i64 {
        if self.spending.swap(true, Ordering::SeqCst) {
            return self.deadline_epoch_ms.load(Ordering::Relaxed);
        }
        *self.deadline.lock().unwrap_or_else(PoisonError::into_inner) =
            self.clock.now() + self.host_span();
        // What the handler is TOLD it has left is a host wall-clock instant rather than a
        // bound: the bound is the deadline above, which this run's own clock is read for.
        let epoch_ms = super::epoch_millis(SystemTime::now() + self.wall_clock);
        self.deadline_epoch_ms.store(epoch_ms, Ordering::Relaxed);
        epoch_ms
    }

    /// Sample the run's DEADLINE from OUTSIDE it, and latch the stop when it is spent.
    ///
    /// Called by the host's own watch thread ([`watch_run`]) rather than by the interpreter,
    /// so what a run is stopped on is the host's own reading and not the guest's traced events:
    /// a long NATIVE call emits none, and this is what notices a run that has stopped producing
    /// them while it sits past its deadline. The stop is latched here and raised at the next
    /// event the interpreter does deliver, which is the only place an interpreter can be
    /// interrupted; the caller's own wait and abandoned-run accounting
    /// ([`super::run_invocation`]) answer the caller whatever the run goes on doing.
    ///
    /// MEMORY is not judged here, and cannot be: what a run holds is the faulted pages of its
    /// OWN thread, a reading only that thread can take ([`Self::past_run_ceiling`], read at
    /// every traced event). The reading a thread of the HOST can take is the whole process's
    /// resident size, which is every run's, every standing environment's and the rest of this
    /// emulator's together — it says nothing about what any one run allocated, so nothing here
    /// is decided on it. What the runs in flight may hold BETWEEN them is bounded where they
    /// are admitted ([`super::MAX_CONCURRENT_RUN_MEMORY_MB`]), and what the environments
    /// standing idle may hold is bounded by the pool that keeps them ([`super::warm_pool`]).
    ///
    /// The deadline judged is the one of the PHASE the run is in: a run still initializing
    /// its execution environment is judged against the span that initialization is allowed
    /// ([`super::INIT_PHASE_LIMIT`]), and one whose handler is running against the
    /// function's own `Timeout` — so neither phase is unbounded and neither is charged the
    /// other's span.
    fn watch(&self) {
        if self.stopped().is_some() {
            return;
        }
        // The HOST's own time this run may take, which is the bound a run whose own clock
        // this host's time does not move is bounded on this host by — and the reading that
        // notices one sitting in a long NATIVE call, because no traced event is coming.
        if self.clock.past_host_ceiling() {
            self.breach(Stop::HostCeiling);
            return;
        }
        if self.clock.now() >= self.deadline()
            || self.past_the_configured_timeout()
            || self.past_the_init_limit()
        {
            self.breach(Stop::Deadline);
        }
    }

    /// When the run must be over.
    fn deadline(&self) -> Instant {
        *self.deadline.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The span of THIS HOST's own time the run may be held for: the span its function's
    /// `Timeout` allows it, held to the most of this host one run may take
    /// ([`Clock::host_span`]).
    fn host_span(&self) -> Duration {
        self.clock.host_span(self.wall_clock)
    }

    /// How much of the span its `Timeout` allows the handler this run has spent: the host time
    /// its work has taken, plus the waits it asked for that this host did not wait out
    /// ([`Self::wait`]).
    fn spent_against_the_timeout(&self) -> Duration {
        self.spent_at(self.remaining())
    }

    /// The same quantity, read against ONE reading of this run's clock: how much of the span
    /// its `Timeout` allows the handler it has spent when the run has `remaining` of its host
    /// deadline left.
    ///
    /// Taken as a parameter so a caller that reads the clock once — [`Self::wait`], which both
    /// charges a wait against the span and holds it inside what is left of the deadline — does
    /// its whole arithmetic at one instant: read twice, the gap between the two readings would
    /// be waived off every wait as a span no bound withheld.
    fn spent_at(&self, remaining: Duration) -> Duration {
        self.host_span()
            .saturating_sub(remaining)
            .saturating_add(Duration::from_nanos(
                self.waived_waiting.load(Ordering::Relaxed),
            ))
    }

    /// How much of the span this run's handler asked to WAIT for no host time passed over
    /// ([`Self::wait`]).
    ///
    /// A wait is charged to the function's `Timeout` in full and really passes on this host
    /// only up to [`super::GUEST_WAIT_HOST_SPAN`], so a run can answer with this much of its
    /// own execution span still to pass; the invocation holds the function's concurrency for
    /// that remainder too ([`super::super::execution_slot`]).
    fn unwaited(&self) -> Duration {
        Duration::from_nanos(self.waived_waiting.load(Ordering::Relaxed))
    }

    /// Whether the handler has had the whole span its function's `Timeout` allows it, which is
    /// what a run reported to have run past its timeout ran past.
    fn past_the_configured_timeout(&self) -> bool {
        self.spent_against_the_timeout() >= self.wall_clock
    }

    /// Spend a WAIT the handler asked for.
    ///
    /// The span asked for is charged against what the function's `Timeout` allows the handler
    /// in FULL, so a handler that waits past its own `Timeout` runs out of it exactly as it
    /// does on AWS — the wait is cut at what is left of that span, and the next bound check
    /// stops the run on its deadline. The wait also REALLY passes on this host, up to
    /// [`super::GUEST_WAIT_HOST_SPAN`]: a wait is how time goes by for the caller, so a client
    /// that observes the run while it is in flight, or that measures how long the call took,
    /// sees the span the handler asked for. What is left of a wait longer than that is time
    /// the run has spent without the host waiting it out, which is what keeps a handler
    /// waiting far longer than this host may hold a run answering at all.
    ///
    /// The caller states what the guest asked for; no guest code decides how much of it the
    /// host spends.
    fn wait(&self, asked: Duration) {
        if self.initializing() {
            self.wait_in_init(asked);
            return;
        }
        // ONE reading of the run's clock answers both halves of the decision below, so the
        // span the wait is charged and the span it is held for are of the same instant: read
        // twice, the sliver of time between the two readings would be waived off every wait as
        // though a bound had withheld it, and the invocation would hold its slot past a run
        // this host waited out in full ([`Self::unwaited`]).
        let remaining = self.remaining();
        let allowed = self.wall_clock.saturating_sub(self.spent_at(remaining));
        let held = asked.min(allowed);
        let host_left = super::GUEST_WAIT_HOST_SPAN
            .saturating_sub(Duration::from_nanos(self.host_waiting.load(Ordering::Relaxed)));
        let really = held.min(remaining).min(host_left);
        self.host_waiting
            .fetch_add(nanoseconds(really), Ordering::Relaxed);
        self.waived_waiting
            .fetch_add(nanoseconds(held.saturating_sub(really)), Ordering::Relaxed);
        self.clock.wait(really);
    }

    /// Spend a WAIT the run takes while the durable execution it is the run of is SUSPENDED:
    /// the wait for a CALLBACK of that execution to be signalled or for its window to close.
    ///
    /// Such a wait is the EXECUTION's and not the run's. A durable execution suspends while it
    /// waits for something outside itself — the invocation that started it is not the thing
    /// doing the waiting, and the execution continues when a client signals the callback or the
    /// window the callback was configured with closes — so neither the function's `Timeout` nor
    /// the most of this host one run may be held for is spent by it: the span is waited out and
    /// the run's own deadline moves with it, leaving what the handler has left exactly as it
    /// was.
    ///
    /// What one run may sit suspended for in all is this HOST's own bound
    /// ([`super::CALLBACK_SUSPENSION_CEILING`]), because what it holds open is an execution
    /// environment of this process: past it the span is a wait of the run's own
    /// ([`Self::wait`]) and the run's bounds end it, exactly as they end a handler that waits
    /// too long.
    ///
    /// The caller states the span; no guest code reaches it. Answers whether the span was spent
    /// SUSPENDED, which is what lets the seam tell the guest whether the wait between two
    /// readings has already been taken for it: a reading the host did not wait inside is one
    /// the guest has to wait after itself, so no loop of readings can spin.
    fn suspend(&self, span: Duration) -> bool {
        let spent = Duration::from_nanos(self.suspended.load(Ordering::Relaxed));
        let suspended = span.min(super::CALLBACK_SUSPENSION_CEILING.saturating_sub(spent));
        if suspended.is_zero() {
            self.wait(span);
            return false;
        }
        self.suspended
            .fetch_add(nanoseconds(suspended), Ordering::Relaxed);
        // The deadline moves BEFORE the span is waited out, so the host's own watch over the
        // run does not read a deadline the suspension has not been credited to yet.
        {
            let mut deadline = self.deadline.lock().unwrap_or_else(PoisonError::into_inner);
            *deadline += suspended;
        }
        // And so does the instant the handler is TOLD it has until: a resumed execution has the
        // span its function is configured for, not what a client's signalling took off it.
        let told = self.deadline_epoch_ms.load(Ordering::Relaxed);
        if told > 0 {
            self.deadline_epoch_ms.store(
                told.saturating_add(i64::try_from(suspended.as_millis()).unwrap_or(i64::MAX)),
                Ordering::Relaxed,
            );
        }
        self.clock.wait(suspended);
        // Whatever the ceiling no longer covered is a wait of the run's own.
        let charged = span.saturating_sub(suspended);
        if !charged.is_zero() {
            self.wait(charged);
        }
        true
    }

    /// Spend a WAIT the code OUTSIDE the handler asked for: one taken while the execution
    /// environment is still being initialized.
    ///
    /// The INIT phase is not bounded by the function's `Timeout` — "the amount of time that
    /// Lambda allows a FUNCTION to run" is the handler's span — but it is not unbounded
    /// either: the phase "is limited to 10 seconds" ([`super::INIT_PHASE_LIMIT`]), so a wait
    /// taken outside the handler is cut at what is LEFT of that span, counted against it, and
    /// the initialization is given up on once it has spent the whole of it
    /// ([`Self::past_the_init_limit`]) — which is what a caller of a function whose
    /// initialization outlasts that span is answered, rather than a result the initialization
    /// never reached.
    ///
    /// How much of THIS HOST's own time passes over it is bounded as it is for the handler's
    /// own waits ([`super::GUEST_WAIT_HOST_SPAN`]).
    fn wait_in_init(&self, asked: Duration) {
        let spent = Duration::from_nanos(self.init_waiting.load(Ordering::Relaxed));
        let held = asked.min(super::INIT_PHASE_LIMIT.saturating_sub(spent));
        let host_left = super::GUEST_WAIT_HOST_SPAN
            .saturating_sub(Duration::from_nanos(self.host_waiting.load(Ordering::Relaxed)));
        let really = held.min(host_left);
        self.host_waiting
            .fetch_add(nanoseconds(really), Ordering::Relaxed);
        // The span the initialization ASKED for is what it spends of its phase, so a wait
        // reaching past the phase's span spends the rest of that span and no more: what
        // follows is the phase being given up on rather than a longer phase.
        self.init_waiting.fetch_add(
            nanoseconds(asked.min(super::INIT_PHASE_LIMIT)),
            Ordering::Relaxed,
        );
        self.clock.wait(really);
    }

    /// Whether the run is still INITIALIZING its execution environment: the invoke phase has
    /// not begun ([`Self::start`]), so the code running is the deployment package's own
    /// module-level code rather than the handler.
    fn initializing(&self) -> bool {
        !self.spending.load(Ordering::SeqCst)
    }

    /// Whether an initialization still in flight has spent the whole span the INIT phase is
    /// allowed ([`super::INIT_PHASE_LIMIT`]).
    ///
    /// Read from what the initialization ASKED to wait for ([`Self::wait_in_init`]) rather
    /// than from this host's own clock, so the phase is judged on what the package's code did
    /// and not on how long this emulator took to build the interpreter it runs in.
    fn past_the_init_limit(&self) -> bool {
        self.initializing()
            && Duration::from_nanos(self.init_waiting.load(Ordering::Relaxed))
                >= super::INIT_PHASE_LIMIT
    }

    /// Spend one traced event: `None` to carry on, `Some(stop)` for the bound that is now
    /// breached.
    fn step(&self) -> Option<Stop> {
        if let Some(stop) = self.stopped() {
            return Some(stop);
        }
        if self.steps.fetch_sub(1, Ordering::Relaxed) == 0 {
            return Some(self.breach(Stop::Steps));
        }
        // The host time this run may take, read from this host's own clock whatever clock the
        // run's deadline is read against ([`Clock::within`]).
        if self.clock.past_host_ceiling() {
            return Some(self.breach(Stop::HostCeiling));
        }
        if self.clock.now() >= self.deadline()
            || self.past_the_configured_timeout()
            || self.past_the_init_limit()
        {
            return Some(self.breach(Stop::Deadline));
        }
        // The run's OWN memory is sampled at EVERY traced event, so there is no sampling
        // interval for an allocating guest to outrun: the faulted pages of this run's own
        // thread are a counter this process already keeps, and they are the only reading that
        // belongs to this one run.
        if self.past_run_ceiling() {
            return Some(self.breach(Stop::Memory));
        }
        None
    }

    /// Hold `bytes` of what this run has faulted in to be the function's EPHEMERAL STORAGE
    /// rather than its memory.
    ///
    /// `/tmp` is a DISK on the real runtime and its bytes are not what `MemorySize` bounds:
    /// "Each Lambda function receives an ephemeral /tmp directory", sized by
    /// `EphemeralStorage.Size` on its own. This emulator holds those bytes in this process's
    /// memory, so the run's own filesystem reports what it wrote there and the reading the
    /// memory ceiling is judged on leaves that much out ([`Held::discount`]) — without it, a
    /// function filling the `/tmp` it is configured for would be reported as running out of
    /// memory it never allocated.
    ///
    /// What a run may have left out is the HOST's to decide and is bounded TWICE, so no figure
    /// a guest states moves a bound: by the size that function's own configuration states, and
    /// by the pool the runs of this process share ([`claimed_from_the_ephemeral_pool`]). A run
    /// that reports more than it wrote therefore gains at most the storage its configuration
    /// already allows it, the runs in flight together gain at most that pool, and every grant
    /// goes back when the run's bounds are dropped.
    fn wrote_to_ephemeral_storage(&self, bytes: u64) {
        let granted = self.ephemeral_granted.load(Ordering::Relaxed);
        let wanted = granted
            .saturating_add(bytes)
            .min(self.ephemeral_bytes)
            .saturating_sub(granted);
        if wanted == 0 {
            return;
        }
        let held = granted.saturating_add(claimed_from_the_ephemeral_pool(wanted));
        self.ephemeral_granted.store(held, Ordering::Relaxed);
        self.held.discount(held);
    }

    /// Whether the run has grown past its own ceiling, sampled from OUTSIDE the guest: the
    /// interpreter exposes no allocation seam, so the bound is read from this thread's
    /// faulted pages, against what this thread had faulted when the run's own code was
    /// about to be read ([`Self::measure_memory_from_here`]).
    fn past_run_ceiling(&self) -> bool {
        self.held.sample() > self.memory_bytes
    }

    /// The MOST memory this run was measured holding, in bytes ([`Held::max_bytes`]).
    ///
    /// Called on the run's own thread, where the reading is this run's.
    fn max_memory_bytes(&self) -> u64 {
        self.held.max_bytes()
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

impl Drop for Bounds {
    /// Give the run's grant out of the ephemeral pool back, so what the runs of this process
    /// have left out of their memory readings is what the runs still in flight were granted.
    fn drop(&mut self) {
        let granted = self.ephemeral_granted.load(Ordering::Relaxed);
        if granted > 0 {
            EPHEMERAL_POOL.fetch_sub(granted, Ordering::SeqCst);
        }
    }
}

/// How much of this process's memory the runs in flight may leave out of their own memory
/// readings TOGETHER, as the ephemeral storage their functions are configured for
/// ([`Bounds::wrote_to_ephemeral_storage`]).
///
/// The per-run bound is that function's own `EphemeralStorage.Size`; this is the bound on the
/// SUM, so what this process may be made to hold does not grow with the number of invocations
/// that are writing to their `/tmp` at once. It is the same figure one environment may hold
/// there at all ([`super::MAX_EPHEMERAL_HELD_BYTES`]), and the runs share it: a run that cannot
/// be granted its share is judged on its memory ceiling like any other run, which stops it.
static EPHEMERAL_POOL: AtomicU64 = AtomicU64::new(0);

/// Claim up to `wanted` bytes of that pool, answering how much of it was granted.
fn claimed_from_the_ephemeral_pool(wanted: u64) -> u64 {
    let mut granted = 0;
    let _ = EPHEMERAL_POOL.try_update(Ordering::SeqCst, Ordering::SeqCst, |held| {
        granted = wanted.min(super::MAX_EPHEMERAL_HELD_BYTES.saturating_sub(held));
        (granted > 0).then(|| held.saturating_add(granted))
    });
    granted
}

/// A span in nanoseconds, for the counters the run's waits are accounted in: a span no
/// `u64` of nanoseconds can hold is counted as the longest one that fits, which is longer
/// than any run.
fn nanoseconds(span: Duration) -> u64 {
    u64::try_from(span.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A budget small enough to exhaust in a test and large enough for any handler one
    /// runs.
    ///
    /// Its memory ceiling is out of reach ([`CEILING_OUT_OF_REACH`]), because a row that is
    /// not about the ceiling must not be answered by it: a row's own handler faults pages of
    /// its thread simply by importing what it needs. A row about the ceiling gives itself one
    /// it can reach.
    fn budget() -> Budget {
        Budget {
            wall_clock: Duration::from_secs(5),
            steps: 2_000_000,
            memory_bytes: CEILING_OUT_OF_REACH,
            ephemeral_bytes: crate::state::execution::ephemeral_storage_bytes(
                crate::state::DEFAULT_EPHEMERAL_STORAGE_MB,
            ),
            log_bytes: 16 * 1024,
            response_bytes: 64 * 1024,
            recursion_limit: 2_000,
        }
    }

    /// How long this module waits for a run's own thread to end before it reports the run as
    /// one that did not end at all.
    ///
    /// NOT a bound on the run: every bound a run has is the host's own, read inside the run
    /// ([`Bounds`]). This is the backstop that keeps a run which will not end from wedging
    /// the suite — the host gives such a run up and answers its caller anyway
    /// ([`super::run_invocation`]), and so does this module. It is far above any span a
    /// row's own bound takes, so no row is ever answered by it.
    const RUN_ANSWER_CEILING: Duration = Duration::from_secs(240);

    /// How far ONE reading drives the clock a row about the DEADLINE is judged against.
    ///
    /// A deadline read against a driven clock ([`Clock::driven`]) falls after a NUMBER of
    /// readings — the run's own traced events, and the host watch's samples of it — rather
    /// than after a span of this host's time, so which bound answers such a row is the same
    /// on an idle host and on a loaded one. It is small against the span it has to drive the
    /// clock across, so the phase a row is about is reached long before the readings taken
    /// getting there have driven the deadline past it.
    const DRIVEN_TICK: Duration = Duration::from_micros(10);

    /// A memory ceiling a row about any OTHER bound cannot reach: the largest `MemorySize` a
    /// function may have (`smithy_query(query="shape", shape="MemorySize")`:
    /// `@range(min: 128, max: 32768)`), in bytes.
    ///
    /// The ceiling is read from the run's OWN thread ([`Bounds::past_run_ceiling`]), which the
    /// run's own work faults pages of whatever the row is about — importing a module, building
    /// the value it returns. A row that names another bound is therefore given the largest
    /// ceiling a function may have, which that work cannot reach, so the bound the row names is
    /// the only one that can answer it.
    const CEILING_OUT_OF_REACH: u64 = 32_768 * 1024 * 1024;

    /// Run `source` as the module `lambda_function` of a one-file package, on a thread with
    /// the stack a run is given — the same way [`super::run_invocation`] runs one.
    fn ran(source: &str, handler: &str, event: Option<&str>) -> Finished {
        ran_with(source, handler, event, &BTreeMap::new(), budget())
    }

    /// [`ran`] with an environment and a budget of its own, against this host's own clock.
    fn ran_with(
        source: &str,
        handler: &str,
        event: Option<&str>,
        environment: &BTreeMap<String, String>,
        budget: Budget,
    ) -> Finished {
        ran_against(
            source,
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
    /// not advance, and a row that names the deadline against one the run's own readings
    /// drive.
    ///
    /// The run's answer is waited for rather than joined on, so a run that will not end
    /// fails the row that asked for it ([`RUN_ANSWER_CEILING`]) instead of holding the suite.
    fn ran_against(
        source: &str,
        handler: &str,
        event: Option<&str>,
        environment: &BTreeMap<String, String>,
        budget: Budget,
        clock: &Clock,
        case: &str,
    ) -> Finished {
        let modules = vec![PackageModule {
            path: "lambda_function.py".to_string(),
            source: source.to_string(),
        }];
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
                        environment: &environment,
                        architecture: crate::state::DEFAULT_ARCHITECTURE,
                        event: event.as_deref(),
                        request_id: "11111111-2222-3333-4444-555555555555",
                        invoked_function_arn:
                            "arn:aws:lambda:us-west-2:000000000000:function:probe",
                        client_context: None,
                        tenant_id: None,
                        // These runs report on the interpreter itself, so they are the runs
                        // of no caller's credential and carry no Cognito identity.
                        cognito_identity: None,
                        budget,
                        // These runs are the interpreter's own containment and bounds: no
                        // emulator is standing behind them, so there is no SDK seam either.
                        aws: None,
                        // Nor a durable execution: nothing here is the run of one.
                        durable: None,
                        // Nor a mounted file system: these runs are the interpreter's own.
                        file_systems: &[],
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
    /// return — so the answer is the run's, not a shape this emulator fills in.
    #[test]
    fn the_answer_is_what_the_handler_returned() {
        let cases: &[(&str, &str, &str)] = &[
            ("a bool member", "{'ok': True}", r#"{"ok":true}"#),
            ("a computed number", "{'n': sum(range(10))}", r#"{"n":45}"#),
            ("a string", "'hello'", r#""hello""#),
            ("a list", "[1, 2, 3]", "[1,2,3]"),
            ("null", "None", "null"),
            (
                "the event echoed back",
                "{'event': event}",
                r#"{"event":{"k":"v"}}"#,
            ),
            (
                "a value from the standard library",
                "{'encoded': __import__('base64').b64encode(b'ab').decode()}",
                r#"{"encoded":"YWI="}"#,
            ),
            (
                "a value from json itself",
                "{'parsed': __import__('json').loads('[1, 2]')}",
                r#"{"parsed":[1,2]}"#,
            ),
        ];
        for (case, expression, expected) in cases {
            let source = format!("def handler(event, context):\n    return {expression}\n");
            let finished = ran(&source, "lambda_function.handler", Some(r#"{"k": "v"}"#));
            let actual = returned(&finished, case);
            let wanted: ::serde_json::Value =
                ::serde_json::from_str(expected).expect("the expected JSON");
            assert_eq!(
                actual, wanted,
                "case {case}: expected {wanted}, got {actual} (log {:?})",
                finished.log
            );
        }
    }

    /// RULE: the IDENTITY of the interpreter a run happens on is the identity of the managed
    /// runtime its function is configured with — the implementation that runtime is and the
    /// release it names — however the interpreter this emulator embeds identifies itself.
    ///
    /// The rows are `python` members of the model's own `Runtime` enum
    /// (`smithy_query(query="shape", shape="Runtime")`), including releases the issue behind
    /// this test never cited, and each is read through every reading of the identity a Python
    /// program has: the implementation record, the version text the standard library's
    /// `platform` module derives the running implementation from, and the release beside them.
    #[test]
    fn a_run_reports_the_implementation_its_runtime_is() {
        let source = "import platform, sys\n\
                      def handler(event, context):\n    \
                          return {\n        \
                              'platform_implementation': platform.python_implementation(),\n        \
                              'name': sys.implementation.name,\n        \
                              'cache_tag': sys.implementation.cache_tag,\n        \
                              'implementation_release': list(sys.implementation.version[:2]),\n        \
                              'same_hexversion': sys.implementation.hexversion == sys.hexversion,\n        \
                              'release': list(sys.version_info[:2]),\n        \
                              'version_names_another': 'rustpython' in sys.version.lower(),\n    \
                          }\n";
        // The runtime the function is configured with, and the release it names.
        let cases: &[(&str, u64, u64)] = &[
            ("python3.9", 3, 9),
            ("python3.12", 3, 12),
            ("python3.13", 3, 13),
        ];
        for (runtime, major, minor) in cases {
            let mut environment = BTreeMap::new();
            environment.insert(
                super::super::EXECUTION_ENV_VARIABLE.to_string(),
                format!("{}{runtime}", super::super::EXECUTION_ENV_PREFIX),
            );
            let finished = ran_with(
                source,
                "lambda_function.handler",
                None,
                &environment,
                budget(),
            );
            let actual = returned(&finished, runtime);
            let expected = ::serde_json::json!({
                "platform_implementation": "CPython",
                "name": "cpython",
                "cache_tag": format!("cpython-{major}{minor}"),
                "implementation_release": [major, minor],
                "same_hexversion": true,
                "release": [major, minor],
                "version_names_another": false,
            });
            assert_eq!(
                actual, expected,
                "case {runtime}: expected the identity of that managed runtime, got {actual} \
                 (log {:?})",
                finished.log
            );
        }
    }

    /// RULE: no name a run can reach on the module its operating system is served by starts a
    /// host process, ends this one, or changes what this process runs as — every one of them
    /// raises, and the names the environment answers from its own memory still answer.
    ///
    /// Source of the names: the POSIX process, signal and privilege calls Python's `os` exposes
    /// on the module it is built on — each family in both of its spellings, so a name added
    /// beside one already refused (`posix_spawn` beside `spawnv`, `fexecve` beside `execve`,
    /// `pipe2` beside `pipe`, `dup3` beside `dup2`, `setresuid` beside `setuid`) is read here
    /// too. The containment is the module this emulator supplies and the refusals installed on
    /// it ([`REFUSED_POSIX_CALLS`]), and this is the reading of it from GUEST code.
    #[test]
    fn no_posix_name_a_run_reaches_starts_or_ends_a_process() {
        let names = [
            "_exit",
            "abort",
            "chroot",
            "dup",
            "dup2",
            "dup3",
            "execv",
            "execve",
            "fexecve",
            "fork",
            "forkpty",
            "initgroups",
            "kill",
            "killpg",
            "open",
            "pipe",
            "pipe2",
            "popen",
            "posix_spawn",
            "posix_spawnp",
            "setgid",
            "setresgid",
            "setresuid",
            "setsid",
            "setuid",
            "spawnv",
            "spawnve",
            "system",
            "wait",
            "waitpid",
        ];
        let source = "import posix\n\
                      def handler(event, context):\n    \
                          outcome = {}\n    \
                          for name in event['names']:\n        \
                              held = getattr(posix, name, None)\n        \
                              if held is None:\n            \
                                  outcome[name] = 'absent'\n            \
                                  continue\n        \
                              try:\n            \
                                  held()\n            \
                                  outcome[name] = 'returned'\n        \
                              except OSError:\n            \
                                  outcome[name] = 'refused'\n        \
                              except BaseException as error:\n            \
                                  outcome[name] = type(error).__name__\n    \
                          return {\n        \
                              'outcome': outcome,\n        \
                              'cwd': posix.getcwd(),\n        \
                              'pid': posix.getpid() > 0,\n    \
                          }\n";
        let event = ::serde_json::json!({ "names": names }).to_string();
        let finished = ran(source, "lambda_function.handler", Some(&event));
        let answered = returned(&finished, "the posix surface a run reaches");
        for name in names {
            let actual = answered
                .get("outcome")
                .and_then(|outcome| outcome.get(name))
                .and_then(::serde_json::Value::as_str);
            assert_eq!(
                actual,
                Some("refused"),
                "case posix.{name}: expected the call to raise, got {actual:?} (log {:?})",
                finished.log
            );
        }
        // And the module still answers what a run reads from its own memory, so the refusals
        // above are refusals of the host-reaching calls and not of the module as a whole.
        for (member, expected) in [
            ("cwd", ::serde_json::json!("/var/task")),
            ("pid", ::serde_json::json!(true)),
        ] {
            let actual = answered.get(member).cloned().unwrap_or_default();
            assert_eq!(
                actual, expected,
                "case posix.{member}: expected {expected}, got {actual} (log {:?})",
                finished.log
            );
        }
    }

    /// The context object reports the invocation, out of the environment the run was given.
    #[test]
    fn the_context_reports_the_invocation() {
        let mut environment = BTreeMap::new();
        environment.insert("AWS_LAMBDA_FUNCTION_NAME".to_string(), "probe".to_string());
        environment.insert("AWS_LAMBDA_FUNCTION_VERSION".to_string(), "7".to_string());
        environment.insert(
            "AWS_LAMBDA_FUNCTION_MEMORY_SIZE".to_string(),
            "512".to_string(),
        );
        environment.insert("GREETING".to_string(), "hello".to_string());
        let source = "import os\n\
                      def handler(event, context):\n    \
                          return {\n        \
                              'name': context.function_name,\n        \
                              'version': context.function_version,\n        \
                              'memory': context.memory_limit_in_mb,\n        \
                              'arn': context.invoked_function_arn,\n        \
                              'request': context.aws_request_id,\n        \
                              'left': context.get_remaining_time_in_millis() > 0,\n        \
                              'greeting': os.environ['GREETING'],\n        \
                              'cwd': os.getcwd(),\n    \
                          }\n";
        let finished = ran_with(
            source,
            "lambda_function.handler",
            None,
            &environment,
            budget(),
        );
        let answered = returned(&finished, "the context");
        let cases: &[(&str, ::serde_json::Value)] = &[
            ("name", ::serde_json::json!("probe")),
            ("version", ::serde_json::json!("7")),
            ("memory", ::serde_json::json!("512")),
            (
                "arn",
                ::serde_json::json!("arn:aws:lambda:us-west-2:000000000000:function:probe"),
            ),
            (
                "request",
                ::serde_json::json!("11111111-2222-3333-4444-555555555555"),
            ),
            ("left", ::serde_json::json!(true)),
            ("greeting", ::serde_json::json!("hello")),
            ("cwd", ::serde_json::json!("/var/task")),
        ];
        for (member, expected) in cases {
            let actual = answered.get(*member).cloned().unwrap_or_default();
            assert_eq!(
                actual, *expected,
                "case context.{member}: expected {expected}, got {actual} (log {:?})",
                finished.log
            );
        }
    }

    /// What the handler wrote to its output streams is the run's log, and nothing of the
    /// host's.
    #[test]
    fn the_log_is_what_the_handler_wrote() {
        let source = "import sys\n\
                      def handler(event, context):\n    \
                          print('to stdout')\n    \
                          sys.stderr.write('to stderr\\n')\n    \
                          return 1\n";
        let finished = ran(source, "lambda_function.handler", None);
        for expected in ["to stdout", "to stderr"] {
            assert!(
                finished.log.contains(expected),
                "case {expected:?}: expected it in the log, got {:?}",
                finished.log
            );
        }
    }

    /// An exception the handler raised is reported as the exception it is, with the
    /// handler's own stack.
    #[test]
    fn an_exception_the_handler_raised_is_reported_as_itself() {
        let cases: &[(&str, &str, &str)] = &[
            ("raise ValueError('boom')", "ValueError", "boom"),
            ("raise KeyError('k')", "KeyError", "'k'"),
            ("return 1 / 0", "ZeroDivisionError", "division by zero"),
            (
                "return {'set': set()}",
                "Runtime.MarshalError",
                "Unable to marshal response",
            ),
        ];
        for (statement, error_type, message) in cases {
            let source = format!("def handler(event, context):\n    {statement}\n");
            let finished = ran(&source, "lambda_function.handler", None);
            match &finished.completion {
                Completion::Raised {
                    error_type: actual_type,
                    error_message,
                    stack_trace,
                    ..
                } => {
                    assert_eq!(
                        actual_type, error_type,
                        "case {statement:?}: expected the type {error_type}, got {actual_type}"
                    );
                    assert!(
                        error_message.contains(message),
                        "case {statement:?}: expected {message:?} in the message, got \
                         {error_message:?}"
                    );
                    if !error_type.starts_with("Runtime.") {
                        assert!(
                            stack_trace.iter().any(|frame| frame.contains("/var/task/")),
                            "case {statement:?}: expected the handler's own frame, got \
                             {stack_trace:?}"
                        );
                    }
                }
                other => panic!("case {statement:?}: expected a raise, got {other:?}"),
            }
        }
    }

    /// A handler that cannot be resolved is the runtime failure for it, named as the real
    /// runtime names it — one row per way a handler fails to resolve.
    #[test]
    fn a_handler_that_does_not_resolve_is_reported_as_the_runtime_failure() {
        let source = "def handler(event, context):\n    return 1\n";
        let cases: &[(&str, &str)] = &[
            ("lambda_function.absent", "Runtime.HandlerNotFound"),
            ("absent_module.handler", "Runtime.ImportModuleError"),
            ("handler", "Runtime.MalformedHandlerName"),
        ];
        for (handler, expected) in cases {
            let finished = ran(source, handler, None);
            match &finished.completion {
                Completion::Raised { error_type, .. } => assert_eq!(
                    error_type, expected,
                    "case {handler:?}: expected {expected}, got {error_type}"
                ),
                other => panic!("case {handler:?}: expected a raise, got {other:?}"),
            }
        }
    }

    /// Every bound is the HOST's: one row per bound, each exhausted from guest code, and
    /// each ends the run with that bound rather than with an answer.
    ///
    /// A row names ONE bound, so the row is given a run only that bound can answer: the
    /// CLOCK the run's deadline is read against is the row's own
    /// ([`super::super::run_clock`]), and a row about a bound that is not a span of time is
    /// judged against a clock that does not advance — which is what keeps a host slow enough
    /// to spend the deadline from answering the row with the deadline instead of with the
    /// bound the row is about. The MEMORY ceiling is put out of reach of every row but the
    /// one about it for the same reason ([`CEILING_OUT_OF_REACH`]). The last two rows are the
    /// same guest code under two different runs, so what differs between their answers is the
    /// bound each run left reachable and nothing of the host.
    #[test]
    fn a_run_that_breaches_a_bound_is_stopped_by_it() {
        /// One row: the bound it is about, the handler body that reaches it, and the run
        /// that leaves that bound the only one able to answer.
        struct BoundCase<'a> {
            /// What the row is about.
            case: &'a str,
            /// The handler's body, which reaches the bound below.
            body: &'a str,
            /// The bound that has to end the run.
            expected: Stop,
            /// The clock the run's deadline is read against.
            clock: Clock,
            /// How many traced events the run may spend: a row about the DEADLINE has to
            /// reach it before the step budget answers instead.
            steps: u64,
            /// How long the handler may run for, in that clock's own time.
            wall_clock: Duration,
            /// How much memory the run may add to this process.
            memory_bytes: u64,
        }
        // Enough events for a row about another bound to reach it, and far fewer than a row
        // about the deadline drives its clock across.
        let few_steps = 50_000;
        let many_steps = 2_000_000_000;
        // Small enough that an allocating handler passes it inside the step budget, and far
        // above what building the interpreter itself faults in.
        let reachable_ceiling = 64 * 1024 * 1024;
        let cases = vec![
            BoundCase {
                case: "a loop with no exit",
                body: "while True:\n        pass\n",
                expected: Stop::Steps,
                clock: Clock::still(),
                steps: few_steps,
                wall_clock: budget().wall_clock,
                memory_bytes: CEILING_OUT_OF_REACH,
            },
            BoundCase {
                case: "unbounded recursion",
                body: "def deep(n):\n        return deep(n + 1)\n    deep(0)\n",
                expected: Stop::Steps,
                clock: Clock::still(),
                steps: few_steps,
                wall_clock: budget().wall_clock,
                memory_bytes: CEILING_OUT_OF_REACH,
            },
            BoundCase {
                case: "a response larger than one may be",
                body: "return 'x' * 200000\n",
                expected: Stop::Response,
                clock: Clock::still(),
                steps: few_steps,
                wall_clock: budget().wall_clock,
                memory_bytes: CEILING_OUT_OF_REACH,
            },
            BoundCase {
                case: "an allocating loop",
                body: "kept = []\n    \
                       while True:\n        \
                           kept.append(bytearray(1024 * 1024))\n",
                expected: Stop::Memory,
                clock: Clock::still(),
                steps: few_steps,
                wall_clock: budget().wall_clock,
                memory_bytes: reachable_ceiling,
            },
            BoundCase {
                case: "a loop with no exit, with events enough to drive its clock past the \
                       deadline",
                body: "while True:\n        pass\n",
                expected: Stop::Deadline,
                clock: Clock::driven(DRIVEN_TICK),
                steps: many_steps,
                wall_clock: Duration::from_secs(1),
                memory_bytes: CEILING_OUT_OF_REACH,
            },
        ];
        for case in &cases {
            let source = format!("def handler(event, context):\n    {}", case.body);
            let mut tight = budget();
            tight.steps = case.steps;
            tight.wall_clock = case.wall_clock;
            tight.memory_bytes = case.memory_bytes;
            let finished = ran_against(
                &source,
                "lambda_function.handler",
                None,
                &BTreeMap::new(),
                tight,
                &case.clock,
                case.case,
            );
            let stopped = match &finished.completion {
                Completion::Stopped(stop) => Some(*stop),
                // Deep recursion may end at the interpreter's own clamped limit first,
                // which is also the host's bound doing its job.
                Completion::Raised { error_type, .. } if error_type == "RecursionError" => None,
                other => panic!("case {}: expected a stop, got {other:?}", case.case),
            };
            if let Some(stopped) = stopped {
                assert_eq!(
                    stopped, case.expected,
                    "case {}: expected {:?}, got {stopped:?}",
                    case.case, case.expected
                );
            }
        }
    }

    /// The step budget cannot be escaped by catching what the hook raises: the stop is
    /// LATCHED, so the run's answer is the stop whatever the guest returns afterwards.
    ///
    /// The run is judged against a clock that does not advance, so the stop the guest
    /// catches is the step budget whatever this host's own clock did meanwhile.
    #[test]
    fn a_guest_that_catches_the_stop_is_still_stopped() {
        let source = "def handler(event, context):\n    \
                          try:\n        \
                              while True:\n            \
                                  pass\n    \
                          except BaseException:\n        \
                              return {'escaped': True}\n";
        let mut tight = budget();
        tight.steps = 20_000;
        let case = "a guest that catches the stop it was given";
        let finished = ran_against(
            source,
            "lambda_function.handler",
            None,
            &BTreeMap::new(),
            tight,
            &Clock::still(),
            case,
        );
        assert!(
            matches!(finished.completion, Completion::Stopped(Stop::Steps)),
            "case {case}: expected the latched step stop to win, got {:?}",
            finished.completion
        );
    }

    /// The HOST's own time one run may take is a bound of its own: the bounds read it at
    /// every traced event and from the watch over the run, and latch it — which is what
    /// bounds the host a run judged against a clock the host does not move runs on.
    ///
    /// Read from the bounds themselves rather than through a run, because what is pinned is
    /// WHERE the ceiling is enforced: the two places a run can be stopped on time
    /// ([`Bounds::step`], the trace hook's own call, and [`Bounds::watch`], the host's watch
    /// over a run that has stopped delivering events). The rows differ along which of them
    /// reads it and along whether the clock states a ceiling at all; a clock that stands
    /// still is used throughout, so no deadline can answer a row instead and nothing waits
    /// for a span of this host's time.
    #[test]
    fn the_bounds_stop_a_run_on_the_host_time_it_may_take() {
        // (case, the clock the run is judged against, the stop its first event must answer)
        let cases: &[(&str, Clock, Option<Stop>)] = &[
            (
                "a ceiling this run has already taken",
                Clock::still().within(Duration::ZERO),
                Some(Stop::HostCeiling),
            ),
            (
                "an hour of host time left to take",
                Clock::still().within(Duration::from_secs(3_600)),
                None,
            ),
            ("no ceiling stated at all", Clock::still(), None),
        ];
        for (case, clock, expected) in cases {
            let mut whole = budget();
            // Far more steps than the one event below spends, so the step budget answers no
            // row.
            whole.steps = 2_000_000_000;
            let stepped = Bounds::new(&whole, clock.clone());
            assert_eq!(
                stepped.step(),
                *expected,
                "case {case}: expected one traced event to answer {expected:?}"
            );

            // The same reading, taken by the host's own watch over a run that is delivering
            // no events at all.
            let watched = Bounds::new(&whole, clock.clone());
            watched.watch();
            assert_eq!(
                watched.stopped(),
                *expected,
                "case {case}: expected the watch over the run to latch {expected:?}"
            );
        }
    }

    /// The span this emulator spends STANDING AN ENVIRONMENT UP is charged to neither phase
    /// of the run: the deadline the initialization is judged against is read from the instant
    /// the environment is standing — the same instant the memory baseline is taken at — so a
    /// host slow enough to spend a whole `Timeout` on building an interpreter does not have
    /// the run answered by its deadline.
    ///
    /// Judged against a clock the run's own READINGS drive, so what a row "spends" standing
    /// the environment up is a number of readings rather than a span of this host's time, the
    /// same on an idle host and on a loaded one. The rows differ along how much of that span
    /// went by before the environment stood: none of it, more of it than the function's own
    /// `Timeout`, and more of it than both of the run's spans together.
    #[test]
    fn the_span_spent_standing_an_environment_up_is_charged_to_neither_phase() {
        // How far ONE reading drives the clock a row is judged against, so a row spends a
        // stated span of it by taking a stated number of readings.
        const TICK: Duration = Duration::from_secs(1);
        /// The `Timeout` every row's function carries, in those same seconds.
        const TIMEOUT: Duration = Duration::from_secs(5);
        // (case, readings taken while the environment was being stood up)
        let cases: &[(&str, u32)] = &[
            ("an environment that stood at once", 0),
            ("a build longer than the function's own Timeout", 8),
            ("a build longer than both of the run's spans together", 40),
        ];
        for (case, readings) in cases {
            let mut whole = budget();
            // Far more steps than the readings below spend, so no row is answered by the
            // step budget instead.
            whole.steps = 2_000_000_000;
            whole.wall_clock = TIMEOUT;
            let clock = Clock::driven(TICK);
            let bounds = Bounds::new(&whole, clock.clone());
            bounds.begin_provisioning();
            for _ in 0..*readings {
                let _ = clock.now();
            }
            // The environment is standing: the initialization's own span starts here, and
            // what standing it up spent is owed by nobody.
            bounds.begin_initialization();
            bounds.watch();
            assert_eq!(
                bounds.stopped(),
                None,
                "case {case}: expected the run to be left alone after {readings} readings \
                 spent standing its environment up"
            );
            // And what it has left is the span an initialization is allowed beside the
            // function's `Timeout` — the two spans of the RUN and nothing of the build's,
            // less only the handful of readings the checks above took.
            let left = bounds.remaining();
            let allowed = crate::state::execution::INIT_PHASE_LIMIT.saturating_add(TIMEOUT);
            assert!(
                left > TIMEOUT && left <= allowed,
                "case {case}: expected between the function's own {TIMEOUT:?} and {allowed:?} \
                 left once the environment stood, got {left:?}"
            );
        }
    }

    /// The deadline is the host's too: a sleep the guest asks for never outlasts the run.
    #[test]
    fn a_sleep_past_the_deadline_does_not_outlast_the_run() {
        let source = "import time\n\
                      def handler(event, context):\n    \
                          time.sleep(600)\n    \
                          return {'slept': True}\n";
        let mut tight = budget();
        tight.wall_clock = Duration::from_secs(2);
        let started = Instant::now();
        let finished = ran_with(
            source,
            "lambda_function.handler",
            None,
            &BTreeMap::new(),
            tight,
        );
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "expected the clamped sleep to end inside the run, took {:?}",
            started.elapsed()
        );
        assert!(
            !matches!(finished.completion, Completion::Internal(_)),
            "expected the run to end on its own bound, got {:?}",
            finished.completion
        );
    }

    /// Containment, attempted FROM guest code: one row per host surface a handler must not
    /// reach. Each row is the expression, and each has to fail.
    #[test]
    fn no_host_surface_is_reachable_from_guest_code() {
        let cases: &[(&str, &str)] = &[
            ("a subprocess", "__import__('subprocess')"),
            ("a shell command", "__import__('os').system('echo hi')"),
            ("a process pipe", "__import__('os').popen('echo hi')"),
            ("a host file descriptor", "__import__('os').open('/etc/hostname', 0)"),
            ("a host file removal", "__import__('os').remove('/etc/hostname')"),
            ("a fork", "__import__('os').fork()"),
            // A socket EXISTS — a traced handler writes its segment documents to the X-Ray
            // daemon through one ([`super::xray_daemon`]) — and the containment is that it
            // reaches that daemon and nothing else: a connection to a host, and a datagram
            // addressed at one, each have no route.
            (
                "a connection to a host",
                "__import__('socket').socket().connect(('example.com', 80))",
            ),
            (
                "a datagram to a host",
                "__import__('socket').socket(2, 2).sendto(b'x', ('example.com', 80))",
            ),
            (
                "a listening socket",
                "__import__('socket').socket().bind(('0.0.0.0', 8080)) or \
                 __import__('socket').socket().listen(1)",
            ),
            (
                "a host name lookup",
                "__import__('socket').gethostbyname('example.com')",
            ),
            ("the host's posix", "__import__('posix').open('/etc/hostname', 0)"),
            ("a host file", "open('/etc/hostname').read()"),
            ("a host directory", "__import__('os').listdir('/')"),
            ("a thread", "__import__('_thread').start_new_thread(print, ())"),
            ("a memory map", "__import__('mmap')"),
            ("a host library", "__import__('ctypes')"),
            ("the host's environment", "__import__('os').environ['PATH']"),
            ("a re-import of a dropped module", "__import__('sys').modules.pop('posix', None) or __import__('posix').open('/etc/hostname', 0)"),
            ("the trace hook", "__import__('sys').settrace(None)"),
            ("the recursion limit", "__import__('sys').setrecursionlimit(1000000)"),
            ("the process", "__import__('os')._exit(0)"),
        ];
        for (case, expression) in cases {
            let source = format!("def handler(event, context):\n    return {expression}\n");
            let finished = ran(&source, "lambda_function.handler", None);
            match &finished.completion {
                Completion::Raised { .. } | Completion::Stopped(_) => {}
                other => panic!(
                    "case {case}: expected guest code reaching {case} to fail, got {other:?} \
                     (log {:?})",
                    finished.log
                ),
            }
        }
    }

    /// Containment of the TYPE behind a host file, attempted the way a Python-level denylist
    /// is sidestepped: not by the name `_io.FileIO`, which is replaced, but by walking the
    /// interpreter's type graph from `object` to the type itself. Constructing it opens
    /// nothing, because the refusal is on the type's own construction.
    #[test]
    fn the_type_behind_a_host_file_refuses_however_it_is_reached() {
        let source = "def handler(event, context):\n    \
                          def found(cls):\n        \
                              for sub in cls.__subclasses__():\n            \
                                  if sub.__name__ == 'FileIO':\n                \
                                      return sub\n            \
                                  deeper = found(sub)\n            \
                                  if deeper is not None:\n                \
                                      return deeper\n        \
                              return None\n    \
                          reached = found(object)\n    \
                          if reached is None:\n        \
                              return {'reached': False}\n    \
                          try:\n        \
                              held = reached('/etc/hostname').read()\n    \
                          except BaseException as error:\n        \
                              return {'reached': True, 'refused': type(error).__name__}\n    \
                          return {'reached': True, 'opened': True, 'held': len(held)}\n";
        let finished = ran(source, "lambda_function.handler", None);
        match &finished.completion {
            // Reaching the type at all is allowed; opening a host file with it is not.
            Completion::Raised { .. } | Completion::Stopped(_) => {}
            Completion::Returned(payload) => assert!(
                !payload.contains("\"opened\"")
                    && (payload.contains("\"refused\"") || payload.contains("\"reached\": false")
                        || payload.contains("\"reached\":false")),
                "expected no host file to be opened through the type graph, got {payload}"
            ),
            other => panic!("expected the type behind a host file to refuse, got {other:?}"),
        }
    }

    /// The step, deadline and memory bounds are the HOST's: a handler that rebinds every name
    /// the host module carries — including one it may expect the hook to be behind — and then
    /// spins is still stopped, because the hook the interpreter calls is not reached through
    /// any name the guest can write.
    #[test]
    fn a_guest_that_rebinds_the_host_module_is_still_bounded() {
        let source = "def handler(event, context):\n    \
                          host = __import__('_lambda_host')\n    \
                          for name in dir(host) + ['step']:\n        \
                              try:\n            \
                                  setattr(host, name, lambda *a, **k: None)\n        \
                              except Exception:\n            \
                                  pass\n    \
                          while True:\n        \
                              pass\n";
        let mut tight = budget();
        // A small STEP budget, so the stop the guest cannot reach arrives at once: with the
        // hook switched off nothing would arrive at all. The run is judged against a clock
        // that does not advance, so that budget is the only bound that can answer it and
        // nothing of this host's own speed decides what the row came to.
        tight.steps = 20_000;
        let case = "a guest that rebound every name the host module carries";
        let finished = ran_against(
            source,
            "lambda_function.handler",
            None,
            &BTreeMap::new(),
            tight,
            &Clock::still(),
            case,
        );
        assert!(
            matches!(finished.completion, Completion::Stopped(Stop::Steps)),
            "case {case}: expected the run to be stopped by the step budget the guest cannot \
             reach, got {:?}",
            finished.completion
        );
    }

    /// Where one PHASE of a run ends and the next begins is the HOST's decision: the host
    /// module carries no name for it, so no guest can start its own invoke phase — and a
    /// guest that goes looking for one over every name that module does carry is still
    /// stopped by the bounds the host set.
    ///
    /// The rows differ along WHICH bound such a guest would move if it could: the deadline,
    /// looked for over and over from a handler that never returns, and the memory baseline,
    /// looked for while the handler's module is being imported between two allocations that
    /// TOGETHER pass the run's ceiling — a run that must still be stopped, because the
    /// ceiling is measured from the one baseline the host took.
    ///
    /// EVERY run here is judged against a clock whose span this host's own speed cannot
    /// spend ([`super::super::run_clock`]), the row that reads the surface included: what
    /// each row comes to is then the run's own doing, and a host slow enough to spend a span
    /// of its own time while the interpreter is standing up answers none of them with the
    /// deadline instead.
    ///
    /// The span the row about the DEADLINE needs is passed by the run ASKING to wait for it
    /// rather than by this host running the run long enough to deliver it: so neither which
    /// bound answers that row nor how much of this host it takes to get there is a reading of
    /// how fast this host happens to be running.
    #[test]
    fn no_guest_code_can_begin_its_own_invoke_phase() {
        // The names the host module really carries, read from inside a run: a surface this
        // small is the whole of what a guest can call, and none of these is a phase or a
        // bound. A name added to it has to be added here too, which is the point.
        //
        // The run is judged against a clock that does not advance, like the rows below: this
        // row is about the SURFACE and not about a span of time, so no deadline may answer
        // it — the step budget and the response cap are the only bounds it leaves reachable,
        // and neither is a reading of how fast this host happens to be running.
        let surface = "the host surface a run can call";
        let listed = ran_against(
            "def handler(event, context):\n    \
                 host = __import__('_lambda_host')\n    \
                 return sorted(n for n in dir(host) if not n.startswith('_'))\n",
            "lambda_function.handler",
            None,
            &BTreeMap::new(),
            budget(),
            &Clock::still(),
            surface,
        );
        let reachable = returned(&listed, surface);
        let expected = ::serde_json::json!([
            "aws",
            "child",
            "db_auth_token",
            "durable",
            "durable_callback",
            "ephemeral",
            "error_shapes",
            "files",
            "library_source",
            "log",
            "net",
            "pagination",
            "presign",
            "record",
            "request",
            "sleep",
            "sqlite",
            "zone"
        ]);
        assert_eq!(
            reachable, expected,
            "expected the host module to carry {expected} and nothing that begins a phase \
             or moves a bound, got {reachable}"
        );

        /// How many megabytes each half of the memory row below allocates.
        const ALLOCATIONS: usize = 40;
        /// How long each turn of the handler of the row about the DEADLINE asks to wait for,
        /// in seconds.
        ///
        /// A wait a run asks for is spent on the run's OWN clock ([`Clock::wait`]): against a
        /// clock the host drives the span is ADVANCED on that clock rather than slept on this
        /// host, and the host clamps it to what the run has left. So the span that row's
        /// deadline needs passes because the run asked for it, in a handful of turns, instead
        /// of being earned by delivering the hundred thousand traced events a span of that
        /// clock's time otherwise takes — work whose cost is this host's own, and which on a
        /// loaded host outlasts the backstop this module gives a run to end in
        /// ([`RUN_ANSWER_CEILING`]) rather than being answered by the run's own bound.
        ///
        /// A fraction of the span that row gives the handler, so the hunt below really does
        /// run turn after turn before the deadline falls.
        const ASKED_PER_TURN_SECONDS: f64 = 0.25;
        // Every name the host module carries, called by the guest in the hope that one of
        // them starts its invoke phase over.
        let hunts = "_host = __import__('_lambda_host')\n\
             def _hunt():\n    \
                 for name in dir(_host):\n        \
                     try:\n            \
                         getattr(_host, name)()\n        \
                     except BaseException:\n            \
                         pass\n";
        // The wait is the one time-moving call the guest does have, and it moves the run
        // TOWARDS its deadline rather than past it: the host clamps it to what the run has
        // left ([`bounded_sleep`]), so a handler that never returns and keeps asking is
        // answered by its deadline whatever it asked for.
        let hunts_the_deadline = format!(
            "{hunts}\
             _hunt()\n\
             import time\n\
             def handler(event, context):\n    \
                 while True:\n        \
                     _hunt()\n        \
                     time.sleep({ASKED_PER_TURN_SECONDS})\n"
        );
        let hunts_the_baseline = format!(
            "{hunts}\
             def megabyte():\n    \
                 return bytearray(b'x' * (1024 * 1024))\n\
             KEPT = [megabyte() for _ in range({ALLOCATIONS})]\n\
             _hunt()\n\
             def handler(event, context):\n    \
                 KEPT.extend(megabyte() for _ in range({ALLOCATIONS}))\n    \
                 return {{'held': len(KEPT)}}\n"
        );
        /// One row: what it is about, the package's module source, the bound that has to end
        /// the run, the clock the run is judged against, the span the handler is given in
        /// that clock's own time, and the ceiling the run may add to this process.
        type PhaseCase<'a> = (&'a str, &'a str, Stop, Clock, Duration, u64);
        let cases: &[PhaseCase<'_>] = &[
            (
                "the deadline, hunted for from inside a handler that never returns",
                hunts_the_deadline.as_str(),
                Stop::Deadline,
                Clock::driven(DRIVEN_TICK),
                Duration::from_secs(1),
                CEILING_OUT_OF_REACH,
            ),
            (
                "the memory baseline, hunted for while the module is being imported",
                hunts_the_baseline.as_str(),
                Stop::Memory,
                Clock::still(),
                budget().wall_clock,
                // Under what the two allocations hold together and over what either holds
                // on its own, so this row is stopped only if the baseline stayed where the
                // host took it.
                64 * 1024 * 1024,
            ),
        ];
        for (case, source, expected, clock, wall_clock, memory_bytes) in cases {
            let mut tight = budget();
            // The bound each row names has to be the only one that can answer it, and WHICH
            // clock the run's deadline is read against is what settles that: the row about
            // the DEADLINE is judged against a clock the run's own readings and the run's own
            // waits drive, so its deadline falls where what the RUN did took it rather than
            // after a span of this host's time, and the row about the BASELINE against a
            // clock that does not advance, so no deadline falls for it at all however slowly
            // this host faults its allocations in. Both are given far more steps than either
            // spends, so the step budget answers neither, and the row about the deadline is
            // given a ceiling out of reach, so the memory bound answers it no more than the
            // clock does.
            tight.wall_clock = *wall_clock;
            tight.steps = 2_000_000_000;
            tight.memory_bytes = *memory_bytes;
            let finished = ran_against(
                source,
                "lambda_function.handler",
                None,
                &BTreeMap::new(),
                tight,
                clock,
                case,
            );
            match &finished.completion {
                Completion::Stopped(stopped) => assert_eq!(
                    stopped, expected,
                    "case {case}: expected the run to be stopped by {expected:?}, got \
                     {stopped:?}"
                ),
                other => panic!(
                    "case {case}: expected the run to be stopped by {expected:?}, got {other:?}"
                ),
            }
        }
    }

    /// A guest `atexit` callback runs during the interpreter's finalization, which is after
    /// the handler returned — and it is bounded like every other guest frame: the hook is
    /// still installed and still answers itself as the local tracer, so a callback that spins
    /// is stopped on one of the run's own bounds rather than wedging the thread.
    #[test]
    fn a_guest_atexit_callback_is_bounded_like_the_handler() {
        let source = "import atexit\n\
                      def spin():\n    \
                          while True:\n        \
                              pass\n\
                      def handler(event, context):\n    \
                          atexit.register(spin)\n    \
                          return {'registered': True}\n";
        let mut tight = budget();
        // The callback is stopped on a budget, so this row is about WHETHER it is bounded and
        // not about how much it is given: a small step budget keeps the spin short, and a
        // clock that does not advance leaves that budget the only bound that can answer the
        // run — the stop here is the callback's own spending and not this host's speed.
        tight.steps = 20_000;
        let case = "an atexit callback that spins during the interpreter's finalization";
        let finished = ran_against(
            source,
            "lambda_function.handler",
            None,
            &BTreeMap::new(),
            tight,
            &Clock::still(),
            case,
        );
        assert!(
            matches!(finished.completion, Completion::Stopped(Stop::Steps)),
            "case {case}: expected the spinning callback to spend the run's step budget, got \
             {:?}",
            finished.completion
        );
    }

    /// The ENVIRONMENT a run sees is the invocation's own and nothing of this process's:
    /// read from inside the run, which is where it matters.
    #[test]
    fn the_environment_a_run_sees_holds_nothing_of_the_host() {
        let mut environment = BTreeMap::new();
        environment.insert("SECRET".to_string(), "shhh".to_string());
        let source = "import os\n\
                      def handler(event, context):\n    \
                          return {'own': os.environ.get('SECRET'), \
                                  'names': sorted(os.environ.keys())}\n";
        let finished = ran_with(
            source,
            "lambda_function.handler",
            None,
            &environment,
            budget(),
        );
        let answered = returned(&finished, "the environment");
        assert_eq!(
            answered.get("own").and_then(::serde_json::Value::as_str),
            Some("shhh"),
            "expected the invocation's own variable, got {answered}"
        );
        let names: Vec<&str> = answered
            .get("names")
            .and_then(::serde_json::Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(::serde_json::Value::as_str)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            names,
            vec!["SECRET"],
            "expected only the invocation's own variables, got {names:?}"
        );
    }

    /// What this runtime presents of the STANDARD LIBRARY, read from inside a run: one row
    /// per module a handler imports that a module table missing a name of `os` could not
    /// import at all, plus the `stat_result` type `os` re-exports.
    ///
    /// The modules are the ones the standard library itself cannot be imported without:
    /// `shutil` (and `tempfile`, `zipfile`, `tarfile`, which import it) reads
    /// `os.stat_result`, `glob` (and `pathlib`, which imports it) reads `os.O_RDONLY`,
    /// `random` (and `secrets`, `statistics`) registers an at-fork hook, and `http.client`
    /// and `urllib.request` are built on `socket`.
    #[test]
    fn the_standard_library_a_run_presents_is_a_complete_one() {
        let modules = [
            "shutil",
            "zipfile",
            "tempfile",
            "tarfile",
            "pathlib",
            "glob",
            "random",
            "secrets",
            "statistics",
            "socket",
            "http.client",
            "urllib.request",
            "importlib.metadata",
        ];
        let source = "def handler(event, context):\n    \
                          read = {}\n    \
                          for name in event['modules']:\n        \
                              try:\n            \
                                  __import__(name)\n            \
                                  read[name] = 'ok'\n        \
                              except BaseException as error:\n            \
                                  read[name] = repr(error)\n    \
                          return read\n";
        let event = ::serde_json::json!({ "modules": modules }).to_string();
        // Importing this much of the standard library is more work than the small budget
        // above allows, and is work a real invocation's own budget covers
        // ([`crate::state::execution`]), so this row is given a run's own.
        let mut whole = budget();
        whole.wall_clock = Duration::from_secs(120);
        whole.steps = 20_000_000;
        let finished = ran_with(
            source,
            "lambda_function.handler",
            Some(&event),
            &BTreeMap::new(),
            whole,
        );
        let answered = returned(&finished, "the standard library");
        for module in modules {
            assert_eq!(
                answered.get(module).and_then(::serde_json::Value::as_str),
                Some("ok"),
                "case {module}: expected the module to import, got {answered}"
            );
        }
        // The type `os` re-exports from the module it is built on, read by name, by index
        // and by length.
        let built = "import os\n\
                     def handler(event, context):\n    \
                         read = os.stat_result((33188, 2, 3, 1, 0, 0, 12, 1.0, 2.0, 3.0))\n    \
                         return [read.st_mode, read.st_size, read[6], len(read)]\n";
        let finished = ran(built, "lambda_function.handler", None);
        let answered = returned(&finished, "the stat_result type");
        assert_eq!(
            answered,
            ::serde_json::json!([33188, 12, 12, 10]),
            "case os.stat_result: expected the stat fields, got {answered}"
        );
    }

    /// A standard-library module that WORKS, not only one that imports: the archive writer
    /// builds a package in memory and the archive reader reads it back, which is what a
    /// handler doing ordinary file work does.
    ///
    /// The reader decodes an entry's name with the codec the ZIP format names for a name not
    /// flagged UTF-8, so this row also pins that a text codec is reachable BY NAME from code
    /// that imported nothing of the codec machinery itself
    /// ([`super::install_codec_registry`]).
    #[test]
    fn the_standard_librarys_archive_machinery_round_trips_inside_a_run() {
        let source = "import io\n\
                      import zipfile\n\
                      def handler(event, context):\n    \
                          buffer = io.BytesIO()\n    \
                          with zipfile.ZipFile(buffer, 'w', zipfile.ZIP_DEFLATED) as built:\n        \
                              built.writestr('rows.txt', 'id,total\\n1,42\\n')\n    \
                          package = buffer.getvalue()\n    \
                          with zipfile.ZipFile(io.BytesIO(package)) as read:\n        \
                              names = sorted(read.namelist())\n        \
                              held = read.read('rows.txt').decode('utf-8')\n    \
                          return {'names': names, 'held': held,\n            \
                                  'single_byte': b'\\x82'.decode('cp437')}\n";
        // Importing the archive machinery (`zipfile`, and `shutil` behind it) is more work
        // than the small budget above allows, and is work a real invocation's own budget
        // covers ([`crate::state::execution`]), so this row is given a run's own.
        let mut whole = budget();
        whole.steps = 20_000_000;
        let finished = ran_with(
            source,
            "lambda_function.handler",
            None,
            &BTreeMap::new(),
            whole,
        );
        let answered = returned(&finished, "the archive round trip");
        assert_eq!(
            answered,
            ::serde_json::json!({
                "names": ["rows.txt"],
                "held": "id,total\n1,42\n",
                "single_byte": "é",
            }),
            "case zipfile: expected the package to be read back as it was built, got \
             {answered}"
        );
    }

    /// The native modules a run can reach are WITHIN the allow-list — read from inside a
    /// run, which is where it matters — and no host-reaching one is among them.
    #[test]
    fn the_registered_native_modules_are_within_the_allow_list() {
        let source = "import sys\n\
                      def handler(event, context):\n    \
                          return sorted(sys.builtin_module_names)\n";
        let finished = ran(source, "lambda_function.handler", None);
        let answered = returned(&finished, "the registered modules");
        let actual: Vec<String> = answered
            .as_array()
            .map(|names| {
                names
                    .iter()
                    .filter_map(::serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        for name in &actual {
            assert!(
                ALLOWED_NATIVE_MODULES.contains(&name.as_str())
                    || SUPPLIED_MODULES.contains(&name.as_str()),
                "case {name}: a module outside the allow-list is reachable, got {actual:?}"
            );
        }
        for absent in [
            "_socket",
            "_ssl",
            "select",
            "_posixsubprocess",
            "mmap",
            "_ctypes",
            "_signal",
            "_interpreters",
            "fcntl",
            "resource",
            "pwd",
        ] {
            assert!(
                !actual.iter().any(|name| name == absent),
                "case {absent}: a host-reaching module is registered, got {actual:?}"
            );
        }
    }

    /// A run is stopped for memory on its OWN reading and on no other: a run that allocated
    /// nothing is answered by its handler however much memory the rest of this process holds.
    ///
    /// The rows are the two readings a run can be in: one whose own work stays inside the
    /// ceiling its function configures, and one whose own work passes it. Neither consults
    /// anything but the faulted pages of the run's own thread, which is why a run of one
    /// function is never ended by what another run, a standing environment or this emulator
    /// itself is holding.
    #[test]
    fn a_run_is_stopped_for_memory_only_on_its_own_reading() {
        // (case, the ceiling the run's function configures, how many MB the handler holds,
        //  whether the run is stopped for memory)
        let cases: &[(&str, u64, usize, bool)] = &[
            (
                "a handler that holds nothing, under a ceiling it cannot reach",
                CEILING_OUT_OF_REACH,
                0,
                false,
            ),
            (
                "a handler that holds a little, under a ceiling it cannot reach",
                CEILING_OUT_OF_REACH,
                4,
                false,
            ),
            (
                "a handler that holds more than its function is configured for",
                8 * 1024 * 1024,
                64,
                true,
            ),
        ];
        for (case, ceiling, megabytes, expected) in cases {
            let source = format!(
                "def lambda_handler(event, context):\n    \
                     held = [bytearray(1024 * 1024) for _ in range({megabytes})]\n    \
                     return {{'held': len(held)}}\n"
            );
            let finished = ran_with(
                &source,
                "lambda_function.lambda_handler",
                None,
                &BTreeMap::new(),
                Budget {
                    memory_bytes: *ceiling,
                    ..budget()
                },
            );
            let stopped = matches!(finished.completion, Completion::Stopped(Stop::Memory));
            assert_eq!(
                stopped, *expected,
                "case {case}: expected the run to be stopped for memory: {expected}, got {:?}",
                finished.completion
            );
        }
    }
}
