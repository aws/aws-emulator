// SPDX-License-Identifier: CC0-1.0

//! The THREADS one invocation's own code starts: the callable a guest hands the thread
//! interface, RUN in this process on the invocation's own thread.
//!
//! A managed runtime's execution environment is an ordinary sandbox, and
//! `threading.Thread(target=…).start()` is how a handler registers an internal extension,
//! drains a telemetry pump, keeps a watchdog or fans work out. So a thread a handler starts has
//! to RUN: its body is the handler's own code, called with the handler's own arguments, and what
//! it leaves behind — what it appended, what it sent, what it registered — is what that code
//! really did. Nothing here invents a result, a thread identity or a failure for work nothing
//! ran.
//!
//! What a thread here is NOT is a thread of this PROCESS. An invocation is one interpreter on
//! one thread, every bound it is judged against belongs to that thread — the step budget, the
//! stop latch and the memory sample are all the one run's — and a second thread running the same
//! interpreter would be outside all three. So a thread is COOPERATIVE, the way the node runtime's
//! workers are ([`super::node`]'s `worker_threads`): the body runs on the invocation's own
//! thread, inside the call that started it, and the consequences are stated rather than hidden:
//!
//! * a thread runs its body before the code that started it goes on, so nothing here makes a
//!   handler faster — what it makes is a handler that uses threads RUN;
//! * two threads never run at the same instant, so a body waiting for something only another
//!   thread of the same invocation could produce waits for nothing, and is ended by its own
//!   slice instead of holding the invocation;
//! * a body runs inside a SLICE of what the invocation has left ([`opened`]) — this host's own
//!   time and a share of the traced events of the run — so a body that does not finish leaves
//!   the invocation the rest of its budget, and the invocation's LOG says the body was stopped
//!   rather than reporting work that was never done;
//! * every WAIT a body asks for is held inside what its slice has left
//!   ([`held_inside_the_slice`]), so a long poll inside a thread times out instead of holding
//!   the invocation until its deadline.
//!
//! The thread interface itself stays the interpreter's own and the standard library's
//! `threading` is the frozen one, driving it exactly as it drives a platform's: what is replaced
//! here are the calls that START a thread and the identity reading that says which thread is
//! running ([`install`]), and what is COMPLETED is the reentrant lock's own surface
//! ([`install_reentrant_lock_surface`]) — the calls `threading.Condition` drives its lock
//! through, which this interpreter's reentrant lock does not carry. The names that reach ACROSS
//! the threads of a process — the main thread's interrupt, and the host sentinel a thread state
//! releases — stay refused, because there is no second thread of this process to reach.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ::rustpython_vm::builtins::{PyBaseExceptionRef, PyDictRef, PyTupleRef};
use ::rustpython_vm::function::{FuncArgs, OptionalArg};
use ::rustpython_vm::{AsObject, PyObjectRef, PyResult, VirtualMachine};

/// How many threads ONE invocation may start.
///
/// A bound of this HOST, like the one on how many child processes an invocation may start: a
/// handler chooses how many threads to start and every body runs on the invocation's own
/// thread, so the count is bounded here. Past it a start is REFUSED with the error Python
/// itself raises for a thread the platform would not start ([`CANNOT_START`]), rather than
/// answered with a thread nothing runs.
const MAX_THREADS_PER_RUN: u64 = 64;

/// What a start this host cannot serve raises: the message `_thread` gives for a thread the
/// platform refused to start.
const CANNOT_START: &str = "can't start new thread";

/// How much of THIS HOST's own time the bodies of all the threads ONE invocation starts may
/// take together.
///
/// Host time rather than a span of the run's own clock: what it bounds is how long this process
/// may spend inside the calls that start threads, so an invocation every one of whose threads
/// blocks still gets to run its handler and answer. Everything besides is bounded by the run's
/// own deadline and step budget, exactly as the handler itself is.
const BODIES_HOST_SPAN: Duration = Duration::from_secs(3);

/// How much of that ONE body may take, so a thread that never finishes cannot spend what the
/// threads started after it would have.
const BODY_HOST_SPAN: Duration = Duration::from_secs(1);

/// What share of the traced events the run has left when the FIRST thread of an invocation
/// starts the bodies of that invocation's threads may spend together: one in this many.
const BODIES_STEP_SHARE: u64 = 4;

/// What share of THAT one body may spend, for the reason [`BODY_HOST_SPAN`] states.
const BODY_STEP_SHARE: u64 = 4;

/// What the invocation's log says about a body this host stopped at its slice.
const STOPPED_AT_ITS_SLICE: &str = "[WARNING] a thread of this invocation was stopped after the \
                                    host time one thread body may take, so the rest of its work \
                                    was not done";

/// What the invocation being served has left, which a body's slice is cut out of.
///
/// Read through [`RunAccess`] rather than held here, because an execution environment serves one
/// invocation after another and what a thread may spend is the invocation's own.
pub(super) struct Left {
    /// WHICH invocation this is, as an identity nothing else of this process shares: the
    /// NUMBER of its own run. A start that reads a number other than the one the counts below
    /// belong to is the first thread of a new invocation, so no budget of the invocation before
    /// it is carried over.
    pub invocation: u64,
    /// How much of its own deadline it has left.
    pub deadline: Duration,
    /// How many traced events it has left.
    pub steps: u64,
    /// Whether one of its own bounds has already been breached, which is what says a body's
    /// failure is the RUN being stopped rather than anything the thread's code did.
    pub stopped: bool,
}

/// How this module reaches the invocation being served: what it has left, and where a line
/// about one of its threads goes.
///
/// Readings handed in rather than the run's own types, which are the Python runtime's
/// ([`super::python`]): what is here is the thread interface alone.
pub(super) struct RunAccess {
    /// What the invocation being served has left.
    pub left: Box<dyn Fn() -> Left + Send + Sync>,
    /// Where a line about one of its threads goes: that invocation's own log.
    pub log: Box<dyn Fn(&str) + Send + Sync>,
}

/// Install the calls that START a thread on the single `_thread` module `sys.modules` holds,
/// and the identity reading they decide — so the frozen `threading` starts a thread through
/// this module and reads back which thread it is in.
///
/// Called once per execution environment, before any guest code runs, and read BACK like every
/// other patch of that module ([`installed`]): a call that did not take its place refuses the
/// run rather than leaving the interpreter's own — which would start a thread of this process —
/// reachable.
pub(super) fn install(vm: &VirtualMachine, run: RunAccess) -> PyResult<()> {
    let threads = vm.import("_thread", 0)?;
    // The identity the interpreter answers for the thread an invocation runs on, read once
    // while that reading is still the interpreter's own: every thread this module starts is
    // identified apart from it ([`opened`]), so the standard library's own per-thread tables
    // keep the main thread's entry while a body runs.
    let main_ident: i64 = threads
        .get_attr("get_ident", vm)?
        .call((), vm)?
        .try_into_value(vm)?;
    let run = Arc::new(run);
    // `start_new_thread(function, args[, kwargs])` and the deprecated spelling of the same
    // call: both answer the identity of the thread that ran.
    for name in ["start_new_thread", "start_new"] {
        let started = Arc::clone(&run);
        let starting = vm.new_function(name, move |args: FuncArgs, vm: &VirtualMachine| {
            let (body, call) = body_and_arguments(vm, args, name)?;
            let ident = ran(vm, &started, &body, call, main_ident)?;
            Ok::<PyObjectRef, PyBaseExceptionRef>(vm.ctx.new_int(ident).into())
        });
        installed(vm, &threads, name, starting.into())?;
    }
    // `start_joinable_thread(function, handle=None, daemon=True)`, which is what
    // `threading.Thread.start` calls. The body takes no arguments, and the HANDLE the caller
    // was given is what a later `join` and `is_alive` read the thread's state off — so it is
    // marked done once the body has run, which is what the thread being over means.
    let started = Arc::clone(&run);
    let joinable = vm.new_function(
        "start_joinable_thread",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let Some(body) = args.args.first().cloned() else {
                return Err(vm.new_type_error(
                    "start_joinable_thread() takes at least one argument".to_owned(),
                ));
            };
            let handle = args
                .kwargs
                .get("handle")
                .cloned()
                .or_else(|| args.args.get(1).cloned())
                .filter(|handle| !vm.is_none(handle));
            let ident = ran(vm, &started, &body, FuncArgs::default(), main_ident)?;
            let handle = match handle {
                Some(handle) => handle,
                None => vm
                    .import("_thread", 0)?
                    .get_attr("_make_thread_handle", vm)?
                    .call((ident,), vm)?,
            };
            if let Ok(done) = handle.get_attr("_set_done", vm) {
                let _ = done.call((), vm);
            }
            Ok(handle)
        },
    );
    installed(vm, &threads, "start_joinable_thread", joinable.into())?;
    // WHICH thread the code asking is running in: the body being run, when one is, and the
    // interpreter's own answer otherwise. The standard library keeps its per-thread tables
    // under this value, so a body that ran would otherwise take the main thread's own entry
    // out of them.
    let identifying = vm.new_function("get_ident", move |_args: FuncArgs, vm: &VirtualMachine| {
        Ok::<PyObjectRef, PyBaseExceptionRef>(vm.ctx.new_int(current_ident(main_ident)).into())
    });
    installed(vm, &threads, "get_ident", identifying.into())?;
    install_reentrant_lock_surface(vm, &threads)
}

/// Complete the REENTRANT LOCK of that same `_thread` module with the calls the standard
/// library drives one through, which this interpreter's own carries none of.
///
/// `threading.Condition` reads three of them off the lock it was built on — whether the running
/// thread owns it, how to give up every level of it while waiting, and how to take those levels
/// back afterwards — and a lock that carries none of them is driven through the generic readings
/// `Condition` keeps for a lock that is not reentrant. One of those readings cannot judge a
/// REENTRANT lock at all: it asks whether the lock can be taken, which the thread already
/// holding it is always granted, so a `Condition` on a reentrant lock answers that its lock is
/// not held — and `wait` and `notify` then raise instead of waiting and waking. A `Condition`
/// built with no lock of its own builds exactly such a reentrant one, which is what
/// `concurrent.futures.Future`, `queue`'s own conditions and every handler that writes
/// `threading.Condition()` stand on.
///
/// The four calls are the ones `_thread`'s reentrant lock has where a platform provides it, and
/// they are answered from the lock's OWN state — whether it is held, and how many times it has
/// been taken. The threads of an invocation run one at a time on the invocation's own thread
/// ([`super::guest_thread`]), so a lock held at all is held by the thread that is running, which
/// is what ownership is here.
fn install_reentrant_lock_surface(vm: &VirtualMachine, threads: &PyObjectRef) -> PyResult<()> {
    let reentrant = threads.get_attr("RLock", vm)?;
    let surface = super::host_posix::built_type(vm, SURFACE_TYPE, REENTRANT_LOCK_SURFACE)?;
    for name in ["_is_owned", "_recursion_count", "_release_save", "_acquire_restore"] {
        // A lock that already carries one of them keeps its own: what is completed here is
        // what the interpreter's lock does not have.
        if reentrant.get_attr(name, vm).is_ok() {
            continue;
        }
        let call = surface.get_attr(name, vm)?;
        reentrant.set_attr(name, call, vm)?;
        if reentrant.get_attr(name, vm).is_err() {
            return Err(vm.new_runtime_error(format!(
                "the reentrant-lock call this emulator installs on _thread.RLock.{name} did not \
                 take effect, so no function is run in this interpreter"
            )));
        }
    }
    Ok(())
}

/// The name the source below defines the surface under.
const SURFACE_TYPE: &str = "_ReentrantLockSurface";

/// The calls [`install_reentrant_lock_surface`] completes `_thread`'s reentrant lock with,
/// written over the lock's own state: whether it is held (`locked()`) and how many times it has
/// been taken (`locked_count`).
const REENTRANT_LOCK_SURFACE: &str = r#"
class _ReentrantLockSurface:
    """The calls the standard library drives a reentrant lock through."""

    def _is_owned(self):
        """Whether the thread that is running holds this lock.

        An invocation's threads run one at a time on the invocation's own thread, so a lock
        that is held is held by the thread asking.
        """
        return self.locked()

    def _recursion_count(self):
        """How many times the holder has taken this lock, and zero when it is free."""
        return self.locked_count if self.locked() else 0

    def _release_save(self):
        """Give up EVERY level of this lock, answering how many were given up."""
        count = self.locked_count
        for _ in range(count):
            self.release()
        return count

    def _acquire_restore(self, state):
        """Take back the levels `_release_save` gave up."""
        for _ in range(state):
            self.acquire()
"#;

/// Put `call` on `_thread` under `name` and read it BACK, so a call that could not be replaced
/// refuses the run rather than leaving the interpreter's own reachable.
fn installed(
    vm: &VirtualMachine,
    threads: &PyObjectRef,
    name: &'static str,
    call: PyObjectRef,
) -> PyResult<()> {
    threads.set_attr(name, call.clone(), vm)?;
    if threads.get_attr(name, vm)?.is(&call) {
        return Ok(());
    }
    Err(vm.new_runtime_error(format!(
        "the thread call this emulator installs on _thread.{name} did not take effect, so no \
         function is run in this interpreter"
    )))
}

/// The body a start names and the arguments it is to be called with: the callable, then the
/// positional arguments and the keyword arguments, exactly as `_thread.start_new_thread` takes
/// them.
fn body_and_arguments(
    vm: &VirtualMachine,
    args: FuncArgs,
    name: &'static str,
) -> PyResult<(PyObjectRef, FuncArgs)> {
    let (body, positional, keyword): (
        PyObjectRef,
        OptionalArg<PyTupleRef>,
        OptionalArg<PyDictRef>,
    ) = args.bind(vm).map_err(|_refused| {
        vm.new_type_error(format!(
            "{name}() takes a callable, its arguments and its keyword arguments"
        ))
    })?;
    let mut call = FuncArgs::default();
    if let OptionalArg::Present(positional) = positional {
        call.args = positional.as_slice().to_vec();
    }
    if let OptionalArg::Present(keyword) = keyword {
        for (key, value) in keyword {
            call.kwargs
                .insert(key.try_into_value::<String>(vm)?.into(), value);
        }
    }
    Ok((body, call))
}

/// RUN one thread's body, inside a slice of what the invocation has left, and answer the
/// identity of the thread it ran as.
///
/// What the body came to is the THREAD's and not the invocation's, exactly as it is on a
/// platform that really starts one: a body that raised is reported in the invocation's log and
/// the code that started it goes on, which is what `_thread` does with an exception a thread
/// raised. The one failure that is not the thread's is the RUN being stopped by one of its own
/// bounds — its deadline, its step budget, its memory ceiling — which is latched for the whole
/// invocation and so is handed back to the caller.
fn ran(
    vm: &VirtualMachine,
    run: &RunAccess,
    body: &PyObjectRef,
    call: FuncArgs,
    main_ident: i64,
) -> PyResult<i64> {
    let left = (run.left)();
    let Some(ident) = opened(&left, main_ident) else {
        return Err(vm.new_runtime_error(CANNOT_START.to_owned()));
    };
    let outcome = body.call(call, vm);
    let ended = closed();
    if ended {
        (run.log)(STOPPED_AT_ITS_SLICE);
    }
    match outcome {
        Ok(_) => Ok(ident),
        Err(error) if (run.left)().stopped => Err(error),
        Err(error) => {
            if !ended {
                (run.log)(&raised(vm, &error));
            }
            Ok(ident)
        }
    }
}

/// What the invocation's log says about a body that RAISED: the exception as the interpreter
/// renders it, which is what a platform writes for a thread whose body raised.
fn raised(vm: &VirtualMachine, error: &PyBaseExceptionRef) -> String {
    let mut rendered = String::from("[ERROR] an exception in a thread of this invocation\n");
    if vm.write_exception(&mut rendered, error).is_err() {
        rendered.push_str("an exception that could not be rendered");
    }
    rendered
}

// What the threads of ONE invocation have left, and the bodies running right now.
//
// Held per HOST THREAD, which is what an execution environment is: the interpreter, the body
// being run and the trace hook that spends its slice are all on the one thread, so nothing
// here is shared with another environment and no lock is taken on the way to the reading the
// hook makes at every traced event.
thread_local! {
    /// The thread budget and the running bodies of the invocation this host thread is serving.
    static THREADS: RefCell<Threads> = const { RefCell::new(Threads::none()) };
}

/// The thread budget of the invocation being served, and the bodies of its threads that are
/// running.
#[derive(Debug)]
struct Threads {
    /// Which invocation the counts below belong to ([`Left::invocation`]), and `0` before the
    /// first thread of any invocation has been started — which is the number of no run at all.
    invocation: u64,
    /// How many threads it has started.
    started: u64,
    /// How much of this host's own time the bodies of its threads may still take together.
    span_left: Duration,
    /// How many traced events they may still spend together.
    steps_left: u64,
    /// The bodies running right now, the innermost last: a body may itself start a thread.
    bodies: Vec<Body>,
}

impl Threads {
    /// The state of a host thread no invocation has started a thread on yet.
    const fn none() -> Self {
        Self {
            invocation: 0,
            started: 0,
            span_left: Duration::ZERO,
            steps_left: 0,
            bodies: Vec::new(),
        }
    }
}

/// One body that is running: which thread it runs as, what its own slice allowed and has left,
/// and whether that slice is over.
#[derive(Debug)]
struct Body {
    /// The identity of the thread it runs as.
    ident: i64,
    /// How much of this host's time its slice allowed it.
    span: Duration,
    /// When that time is up.
    until: Instant,
    /// How many traced events its slice allowed it.
    allowed: u64,
    /// How many of those it may still spend.
    steps: u64,
    /// Whether its slice is over, which is LATCHED: a body that caught the stop is stopped
    /// again at its next traced event, so it cannot carry on with its slice spent.
    ended: bool,
}

impl Body {
    /// How much of this host's time it has really taken.
    fn spent_span(&self) -> Duration {
        self.span
            .saturating_sub(self.until.saturating_duration_since(Instant::now()))
    }

    /// How many of the traced events its slice allowed it it has spent.
    fn spent_steps(&self) -> u64 {
        self.allowed.saturating_sub(self.steps)
    }
}

/// OPEN a slice for one body and answer the identity of the thread it runs as, or `None` when
/// this invocation may start no further thread: it has started as many as one may
/// ([`MAX_THREADS_PER_RUN`]), or the host time and the traced events its threads may spend
/// together are gone.
///
/// The slice is cut out of what the invocation has left — this host's own time
/// ([`BODY_HOST_SPAN`]) and a share of the traced events the run had left when the first thread
/// of this invocation started ([`BODIES_STEP_SHARE`]) — each held inside what the invocation's
/// threads have left together, and inside what the run has left of its own deadline.
fn opened(left: &Left, main_ident: i64) -> Option<i64> {
    THREADS.with(|held| {
        let mut held = held.borrow_mut();
        // The FIRST thread of an invocation opens that invocation's own budget, so nothing of
        // the invocation served before it is carried over.
        if held.invocation != left.invocation {
            *held = Threads {
                invocation: left.invocation,
                started: 0,
                span_left: BODIES_HOST_SPAN,
                steps_left: left.steps / BODIES_STEP_SHARE,
                bodies: Vec::new(),
            };
        }
        if held.started >= MAX_THREADS_PER_RUN {
            return None;
        }
        let span = BODY_HOST_SPAN.min(held.span_left).min(left.deadline);
        let steps = held.steps_left / BODY_STEP_SHARE;
        if span.is_zero() || steps == 0 {
            return None;
        }
        held.started += 1;
        let ident = main_ident.saturating_add(i64::try_from(held.started).unwrap_or(i64::MAX));
        held.bodies.push(Body {
            ident,
            span,
            until: Instant::now() + span,
            allowed: steps,
            steps,
            ended: false,
        });
        Some(ident)
    })
}

/// CLOSE the slice of the body that has just finished, charging what it spent against what this
/// invocation's threads have left, and answer whether that body was ENDED by its slice rather
/// than by finishing.
fn closed() -> bool {
    THREADS.with(|held| {
        let mut held = held.borrow_mut();
        let Some(body) = held.bodies.pop() else {
            return false;
        };
        held.span_left = held.span_left.saturating_sub(body.spent_span());
        held.steps_left = held.steps_left.saturating_sub(body.spent_steps());
        body.ended
    })
}

/// SPEND one traced event of the body running right now: `true` when its slice is over, which
/// is what ends that body and nothing else.
///
/// Called from the run's own trace hook at every traced event, which is the one place an
/// interpreter can be interrupted.
pub(super) fn slice_spent() -> bool {
    THREADS.with(|held| {
        let mut held = held.borrow_mut();
        let Some(body) = held.bodies.last_mut() else {
            return false;
        };
        if body.ended {
            return true;
        }
        if body.steps == 0 || Instant::now() >= body.until {
            body.ended = true;
            return true;
        }
        body.steps -= 1;
        false
    })
}

/// The STOP a body whose slice is over is ended with: `SystemExit`, which is the one exception
/// the standard library's own thread report passes over in silence — a thread the platform ended
/// is not a failure of the code that started it — and which [`ran`] reports in the invocation's
/// log instead.
pub(super) fn stopping(vm: &VirtualMachine) -> PyBaseExceptionRef {
    vm.new_exception_msg(
        vm.ctx.exceptions.system_exit.to_owned(),
        STOPPED_AT_ITS_SLICE.to_owned().into(),
    )
}

/// Hold `asked` inside what the body running right now has left of its slice, and answer
/// `asked` itself when no body is running.
///
/// Every WAIT a guest asks for goes through this: a body waiting for something only another
/// thread of the same invocation could produce waits for nothing at all, so a wait taken inside
/// one is held to what that body has left rather than to the invocation's whole deadline.
pub(super) fn held_inside_the_slice(asked: Duration) -> Duration {
    THREADS.with(|held| {
        let held = held.borrow();
        match held.bodies.last() {
            Some(body) => asked.min(body.until.saturating_duration_since(Instant::now())),
            None => asked,
        }
    })
}

/// WHICH thread the code asking is running in: the body running right now, or `main_ident` —
/// the interpreter's own answer for the thread an invocation runs on — when none is.
fn current_ident(main_ident: i64) -> i64 {
    THREADS.with(|held| {
        let held = held.borrow();
        held.bodies.last().map_or(main_ident, |body| body.ident)
    })
}
