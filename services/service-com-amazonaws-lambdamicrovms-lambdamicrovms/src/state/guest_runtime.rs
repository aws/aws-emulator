// SPDX-License-Identifier: CC0-1.0

//! RUNNING a MicroVM's guest: the engine the image's own code executes in, inside this
//! process, and the one request a published endpoint hands it.
//!
//! `RunMicrovmResponse$endpoint` is "the HTTPS endpoint URL for communicating with the
//! MicroVM", and what a client reads back from it is the GUEST's own HTTP response —
//! whatever the guest chose to send. None of that can be assembled here, so none of it
//! is: the guest's program ([`super::guest_program`]) is compiled and executed by the
//! QuickJS engine embedded in this process (`rquickjs`), the server it registers is
//! handed the request the client sent, and the status, headers and body it answered are
//! read back off that run. A guest that registers no server, or whose code raises before
//! it answers, produces no response, and that is reported as what it is.
//!
//! # What the guest can reach
//!
//! A FRESH engine per proxied request, so nothing of one request is reachable from the
//! next. Inside it the guest reaches the language, the modules this module implements for
//! it ([`RUNTIME_SOURCE`]: `http`, `net`, `url`, `events`, `querystring`, a console and
//! timers) and the three host functions those are built over — the response sink, the
//! outbound-connection seam and the log writer — each deleted from the global object
//! before any of the guest's own code is compiled, so only this module's closures still
//! hold them. No module loader is installed, so an `import` resolves to nothing; plain
//! QuickJS registers no filesystem, process, environment, socket, host-library or thread
//! API, and nothing here adds one. The guest's `net` is this module's own: it opens no
//! host socket, and where a connection goes is decided by the MicroVM's egress network
//! connectors ([`super::microvm_network`]).
//!
//! # What bounds it
//!
//! Every bound lives in host code the guest cannot reach:
//!
//! * a WALL CLOCK ([`RUN_WALL_CLOCK`]) and a STEP budget ([`RUN_ENGINE_CHECKS`]), both
//!   spent by the engine's own interrupt handler as it works, so a guest that never
//!   returns and one that loops without allocating are each stopped;
//! * a MEMORY ceiling ([`RUN_MEMORY_BYTES`]) and a JavaScript STACK ceiling
//!   ([`RUN_STACK_BYTES`]), both the engine's own;
//! * ceilings on what one request may produce: the response body
//!   ([`MAX_RESPONSE_BYTES`]), the log lines ([`MAX_LOG_ENTRIES`],
//!   [`MAX_LOG_ENTRY_BYTES`]) and the outbound connections one request may open
//!   ([`MAX_CONNECTS`]);
//! * bounds on the work the run is driven through: the engine jobs
//!   ([`MAX_PENDING_JOBS`]) and the timer callbacks ([`MAX_TIMER_TICKS`]) one request may
//!   spend;
//! * a CONCURRENCY permit ([`MAX_CONCURRENT_RUNS`]) and a CALLER WAIT ceiling
//!   ([`CALLER_WAIT_CEILING`]) held outside the engine, so however many clients reach
//!   their MicroVMs at once, what the engines hold is bounded and each request is
//!   answered even where a stretch of engine-internal work outlasts the run's own clock.
//!
//! Which bound was breached is LATCHED in host state rather than read back out of the
//! engine, so a guest that catches the exception the engine raises in it cannot make the
//! run report a response.
//!
//! # The guest's clock
//!
//! A guest's timers run on a VIRTUAL clock: `setTimeout` records when a callback is due
//! in the guest's own time, and the host runs the earliest due callback whenever nothing
//! else can make progress. Nothing sleeps — a guest that waits five seconds for a
//! connection costs the client no wall-clock time — and the ORDER the guest sees is the
//! order it asked for.
//!
//! The run happens on a BLOCKING thread, never on the async executor serving the request,
//! and no store lock is held across it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ::rquickjs::function::Func;
use ::rquickjs::{CatchResultExt, Context, Ctx, Function, Object, Runtime};

use super::guest_program::GuestProgram;
use super::microvm_network::ConnectVerdict;

/// How long one request's run may take before the engine's interrupt handler stops it.
const RUN_WALL_CLOCK: Duration = Duration::from_secs(5);

/// How long the CALLER waits for a run at most, which is longer than the run's own clock
/// by the margin it takes the engine to notice that clock.
const CALLER_WAIT_CEILING: Duration = Duration::from_secs(10);

/// How many engine checks one run may spend — the STEP bound beside the wall-clock one,
/// and the one that does not depend on how fast the host is.
const RUN_ENGINE_CHECKS: u64 = 2_000_000;

/// The heap one run's engine is built with.
const RUN_MEMORY_BYTES: usize = 64 * 1024 * 1024;

/// The JavaScript stack one run's engine is built with, well inside the stack of the
/// blocking thread the run happens on.
const RUN_STACK_BYTES: usize = 512 * 1024;

/// How large a response body one request may carry.
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// How many log lines one request may write.
const MAX_LOG_ENTRIES: usize = 200;

/// How long one log line may be, in bytes.
const MAX_LOG_ENTRY_BYTES: usize = 4_096;

/// How many outbound connections one request may open.
const MAX_CONNECTS: u64 = 16;

/// How many engine jobs one request may spend.
const MAX_PENDING_JOBS: usize = 20_000;

/// How many timer callbacks one request may spend.
const MAX_TIMER_TICKS: usize = 10_000;

/// How many guests this emulator runs AT ONCE: each run holds the engine's own heap on a
/// blocking thread, so this is what bounds what those runs occupy however many clients
/// arrive together.
const MAX_CONCURRENT_RUNS: usize = 4;

/// How long a client waits for one of those run slots before the request is answered as
/// one the guest never saw.
const RUN_PERMIT_WAIT: Duration = Duration::from_secs(5);

/// How long the host waits for one outbound-connection decision before the connection is
/// reported as one nothing carried.
const CONNECT_DECISION_CEILING: Duration = Duration::from_secs(5);

/// The run slots [`MAX_CONCURRENT_RUNS`] declares, over every MicroVM of this process.
fn guest_runs() -> &'static Arc<::tokio::sync::Semaphore> {
    static RUNS: ::std::sync::OnceLock<Arc<::tokio::sync::Semaphore>> =
        ::std::sync::OnceLock::new();
    RUNS.get_or_init(|| Arc::new(::tokio::sync::Semaphore::new(MAX_CONCURRENT_RUNS)))
}

/// One request a client sent through a MicroVM's published endpoint, as the guest is
/// handed it.
#[derive(Debug, Clone)]
pub struct GuestRequest {
    /// The HTTP method.
    pub method: String,
    /// The path and query the client addressed the guest with — `req.url` as a Node
    /// server reads it.
    pub url: String,
    /// The headers the client sent, lowercased as a Node server reports them.
    pub headers: Vec<(String, String)>,
    /// The request body, as the text the guest reads.
    pub body: String,
}

/// The response a guest answered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestAnswer {
    /// The status the guest set.
    pub status: u16,
    /// The headers it set, in the order it set them.
    pub headers: Vec<(String, String)>,
    /// The body it wrote.
    pub body: Vec<u8>,
}

/// What one proxied request came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuestOutcome {
    /// The guest answered.
    Answered(GuestAnswer),
    /// The MicroVM's image holds no guest program this engine can run, so nothing is
    /// listening inside the MicroVM.
    NoGuest,
    /// A guest ran and produced no response: it registered no server, its code raised
    /// before answering, or a bound of this module stopped it. The clause says which.
    NoResponse(String),
    /// Nothing of the guest ran at all — the engine did not start, or every run slot was
    /// taken. Nothing was learned about the guest.
    Unavailable(String),
}

/// One outbound connection a guest opened, handed to the host to decide.
#[derive(Debug)]
pub struct ConnectAsk {
    /// The address the guest named.
    pub host: String,
    /// The port it named.
    pub port: i32,
    /// Where the decision is sent back to the run that is waiting for it.
    pub reply: ::std::sync::mpsc::Sender<ConnectVerdict>,
}

/// The channel a run's outbound connections are decided over: the async side of the
/// listener holds the receiver and answers each ask from the emulated network.
pub type ConnectSeam = ::tokio::sync::mpsc::UnboundedSender<ConnectAsk>;

/// Hand `request` to the guest `program` and report what it answered.
///
/// The run slot is OWNED by the blocking thread rather than by this future: dropping a
/// join handle does not stop a blocking task, so a run the caller gave up on keeps its
/// slot until its thread really ends, and [`MAX_CONCURRENT_RUNS`] bounds the engines that
/// exist whatever a client does.
pub async fn serve(
    program: Arc<GuestProgram>,
    request: GuestRequest,
    connects: ConnectSeam,
) -> GuestOutcome {
    let waited = ::tokio::time::timeout(
        RUN_PERMIT_WAIT,
        Arc::clone(guest_runs()).acquire_owned(),
    )
    .await;
    let permit = match waited {
        Ok(Ok(permit)) => permit,
        _ => {
            return GuestOutcome::Unavailable(format!(
                "every one of the {MAX_CONCURRENT_RUNS} guest run slots this emulator holds \
                 was taken for {} seconds",
                RUN_PERMIT_WAIT.as_secs()
            ))
        }
    };
    let running = ::tokio::task::spawn_blocking(move || {
        // The slot is released where the RUN ends, on every path out of it.
        let _slot = permit;
        run(&program, &request, &connects)
    });
    match ::tokio::time::timeout(CALLER_WAIT_CEILING, running).await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(error)) => GuestOutcome::Unavailable(format!(
            "the thread the guest ran on did not finish: {error}"
        )),
        Err(_) => GuestOutcome::Unavailable(format!(
            "the guest was given up on after {} seconds",
            CALLER_WAIT_CEILING.as_secs()
        )),
    }
}

/// Run the guest on THIS thread, under every bound this module declares.
fn run(program: &GuestProgram, request: &GuestRequest, connects: &ConnectSeam) -> GuestOutcome {
    let runtime = match Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => return GuestOutcome::Unavailable(format!("the engine did not start: {error}")),
    };
    runtime.set_memory_limit(RUN_MEMORY_BYTES);
    runtime.set_max_stack_size(RUN_STACK_BYTES);
    let bounds = Arc::new(Bounds::new());
    let watched = Arc::clone(&bounds);
    // The only thing that can stop a guest with no exit: a timeout cannot preempt a loop
    // running on this thread, but the engine asks this handler as it works.
    runtime.set_interrupt_handler(Some(Box::new(move || watched.spent())));
    let context = match Context::full(&runtime) {
        Ok(context) => context,
        Err(error) => return GuestOutcome::Unavailable(format!("the engine did not start: {error}")),
    };

    let collected = Arc::new(Mutex::new(Collected::default()));
    let answered: Arc<Mutex<Option<GuestAnswer>>> = Arc::new(Mutex::new(None));
    let started = context.with(|ctx| hand_over(ctx, program, request, connects, &collected, &answered));
    let started = match started {
        Ok(started) => started,
        Err(outcome) => {
            runtime.set_interrupt_handler(None);
            return outcome;
        }
    };
    // A guest that has not answered yet is still working: its promises are settled by the
    // engine's jobs and its callbacks by its own timers, and nothing but this drives them.
    if answered
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_none()
    {
        drain(&runtime, &context, &started.tick, &bounds, &answered);
    }
    // The run is over, and nothing the engine does to tear itself down is the guest's
    // work.
    runtime.set_interrupt_handler(None);
    context.with(|ctx| drop(ctx.catch()));

    for line in collected
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .logs
        .iter()
    {
        // The guest's own output, rendered in its escaped form so a line it wrote cannot
        // forge a log line of this emulator's.
        ::tracing::debug!(line = ?line, "a MicroVM guest wrote a log line");
    }

    // A breached bound is the run's answer, whatever the guest then said: the latch is
    // host state the guest cannot reach.
    if let Some(breach) = bounds.breach() {
        return GuestOutcome::NoResponse(breach);
    }
    let answer = answered
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    match answer {
        Some(answer) => GuestOutcome::Answered(answer),
        None => GuestOutcome::NoResponse(started.unanswered()),
    }
}

/// What handing the request to the guest produced: what the drain loop drives it through,
/// and what a run that never answers is reported as.
struct Started {
    /// The guest clock's own driver, held OUTSIDE the engine's global object so a guest
    /// that rebinds its globals cannot replace what drives it.
    tick: ::rquickjs::Persistent<Function<'static>>,
    /// Whether the guest registered a server listening inside the MicroVM.
    listening: bool,
    /// Whether the guest's code raised, and what it said.
    raised: Option<String>,
}

impl Started {
    /// What a run that produced no response is reported as.
    fn unanswered(&self) -> String {
        if let Some(raised) = &self.raised {
            return format!("the guest raised before it answered: {raised}");
        }
        if !self.listening {
            return NO_LISTENER.to_string();
        }
        NO_RESPONSE.to_string()
    }
}

/// What a guest that registered no server is reported as.
const NO_LISTENER: &str = "the guest registered no HTTP server, so nothing is listening";

/// What a guest that registered a server and never answered is reported as.
const NO_RESPONSE: &str = "the guest did not answer the request";

/// Compile the guest's program, hand it the request and report what that produced, inside
/// one engine context.
fn hand_over(
    ctx: Ctx<'_>,
    program: &GuestProgram,
    request: &GuestRequest,
    connects: &ConnectSeam,
    collected: &Arc<Mutex<Collected>>,
    answered: &Arc<Mutex<Option<GuestAnswer>>>,
) -> Result<Started, GuestOutcome> {
    install_host_seams(&ctx, connects, collected, answered)
        .map_err(|reason| GuestOutcome::Unavailable(format!("the engine did not start: {reason}")))?;
    // Held BEFORE any of the guest's code is compiled, so the runtime this request is
    // driven through is this module's own however the guest rebinds the global object.
    let runtime: Object<'_> = ctx
        .eval::<Object<'_>, _>(RUNTIME_SOURCE)
        .catch(&ctx)
        .map_err(|caught| {
            GuestOutcome::Unavailable(format!("the guest runtime could not be installed: {caught}"))
        })?;

    let modules = ::serde_json::to_string(&program.modules).unwrap_or_else(|_| "{}".to_string());
    let modules = ctx.json_parse(modules).map_err(|error| {
        GuestOutcome::Unavailable(format!("the guest's modules could not be read: {error}"))
    })?;
    let start: Function<'_> = runtime.get("start").map_err(|error| {
        GuestOutcome::Unavailable(format!("the guest runtime has no entry point: {error}"))
    })?;
    let dispatch: Function<'_> = runtime.get("dispatch").map_err(|error| {
        GuestOutcome::Unavailable(format!("the guest runtime has no request sink: {error}"))
    })?;
    let tick: Function<'_> = runtime.get("tick").map_err(|error| {
        GuestOutcome::Unavailable(format!("the guest runtime has no clock: {error}"))
    })?;
    let tick = ::rquickjs::Persistent::save(&ctx, tick);

    // From here on what runs is the GUEST's: its own modules, and the server it
    // registered, called over the request the client sent.
    let listening = match start
        .call::<_, bool>((modules, program.entry.clone()))
        .catch(&ctx)
    {
        Ok(listening) => listening,
        Err(caught) => {
            return Ok(Started {
                tick,
                listening: false,
                raised: Some(raised_message(&caught)),
            })
        }
    };
    if !listening {
        return Ok(Started {
            tick,
            listening: false,
            raised: None,
        });
    }
    let headers = ::serde_json::to_string(&request.headers).unwrap_or_else(|_| "[]".to_string());
    match dispatch
        .call::<_, ()>((
            request.method.clone(),
            request.url.clone(),
            headers,
            request.body.clone(),
        ))
        .catch(&ctx)
    {
        Ok(()) => Ok(Started {
            tick,
            listening,
            raised: None,
        }),
        Err(caught) => Ok(Started {
            tick,
            listening,
            raised: Some(raised_message(&caught)),
        }),
    }
}

/// Drive the guest until it answers, until it has nothing left to do, or until a bound
/// ends the run: the engine's jobs first, then the earliest timer callback the guest's
/// own clock has due.
fn drain(
    runtime: &Runtime,
    context: &Context,
    tick: &::rquickjs::Persistent<Function<'static>>,
    bounds: &Arc<Bounds>,
    answered: &Arc<Mutex<Option<GuestAnswer>>>,
) {
    let mut jobs = 0usize;
    let mut ticks = 0usize;
    loop {
        if bounds.stop_if_due() {
            return;
        }
        while jobs < MAX_PENDING_JOBS {
            match runtime.execute_pending_job() {
                Ok(true) => jobs += 1,
                Ok(false) | Err(_) => break,
            }
            if bounds.stop_if_due() {
                return;
            }
        }
        if answered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
        {
            return;
        }
        if ticks >= MAX_TIMER_TICKS || jobs >= MAX_PENDING_JOBS {
            return;
        }
        ticks += 1;
        let more = context.with(|ctx| {
            tick.clone()
                .restore(&ctx)
                .and_then(|tick| tick.call::<_, bool>(()))
                .unwrap_or(false)
        });
        if !more {
            return;
        }
    }
}

/// The global the response sink is handed to the engine as.
const HOST_RESPOND: &str = "__microvmHostRespond";

/// The global the outbound-connection seam is handed to the engine as.
const HOST_CONNECT: &str = "__microvmHostConnect";

/// The global the log writer is handed to the engine as.
const HOST_LOG: &str = "__microvmHostLog";

/// Install the three host functions the guest's runtime is built over.
///
/// They are the only host functions reachable from inside the engine, and
/// [`RUNTIME_SOURCE`] deletes their global bindings as soon as it holds them, so after it
/// has run they are reachable from nothing but its own closures.
fn install_host_seams(
    ctx: &Ctx<'_>,
    connects: &ConnectSeam,
    collected: &Arc<Mutex<Collected>>,
    answered: &Arc<Mutex<Option<GuestAnswer>>>,
) -> Result<(), String> {
    let latch = Arc::clone(answered);
    let respond = Func::from(move |status: i32, headers: String, body: String| {
        let mut held = latch.lock().unwrap_or_else(PoisonError::into_inner);
        if held.is_some() {
            // A guest that answers twice has answered once: the first response is the
            // one the client reads, exactly as a server that has already flushed its
            // headers cannot take them back.
            return;
        }
        let mut body = body.into_bytes();
        body.truncate(MAX_RESPONSE_BYTES);
        *held = Some(GuestAnswer {
            status: u16::try_from(status).unwrap_or(200),
            headers: ::serde_json::from_str::<Vec<(String, String)>>(&headers).unwrap_or_default(),
            body,
        });
    });

    let seam = connects.clone();
    let opened = Arc::new(AtomicU64::new(0));
    let connect = Func::from(move |host: String, port: i32| -> String {
        let taken = opened.fetch_add(1, Ordering::Relaxed);
        if taken >= MAX_CONNECTS {
            return verdict_json(&ConnectVerdict::unreached());
        }
        let (reply, answer) = ::std::sync::mpsc::channel();
        if seam
            .send(ConnectAsk {
                host,
                port,
                reply,
            })
            .is_err()
        {
            return verdict_json(&ConnectVerdict::unreached());
        }
        let verdict = answer
            .recv_timeout(CONNECT_DECISION_CEILING)
            .unwrap_or_else(|_| ConnectVerdict::unreached());
        verdict_json(&verdict)
    });

    let logged = Arc::clone(collected);
    let log = Func::from(move |message: String| {
        logged
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .log(&message);
    });

    let globals = ctx.globals();
    globals
        .set(HOST_RESPOND, respond)
        .and_then(|()| globals.set(HOST_CONNECT, connect))
        .and_then(|()| globals.set(HOST_LOG, log))
        .map_err(|error| error.to_string())
}

/// One connection decision, as the guest's `net` reads it back.
fn verdict_json(verdict: &ConnectVerdict) -> String {
    ::serde_json::json!({
        "connected": verdict.connected,
        "localAddress": verdict.local_address,
        "localPort": verdict.local_port,
    })
    .to_string()
}

/// The message a raise carries: the error's own, as the guest's code worded it.
fn raised_message(caught: &::rquickjs::CaughtError<'_>) -> String {
    match caught {
        ::rquickjs::CaughtError::Exception(exception) => exception
            .message()
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| exception.to_string()),
        other => other.to_string(),
    }
}

/// The log lines one run wrote, under this module's ceilings.
#[derive(Debug, Default)]
struct Collected {
    logs: Vec<String>,
}

impl Collected {
    /// Record one line, under the count and length ceilings, cut on a character boundary
    /// so a capped line is still a string.
    fn log(&mut self, message: &str) {
        if self.logs.len() >= MAX_LOG_ENTRIES {
            return;
        }
        let mut line = message.to_string();
        if line.len() > MAX_LOG_ENTRY_BYTES {
            let boundary = line
                .char_indices()
                .map(|(index, _)| index)
                .take_while(|index| *index <= MAX_LOG_ENTRY_BYTES)
                .last()
                .unwrap_or_default();
            line.truncate(boundary);
        }
        self.logs.push(line);
    }
}

/// The run's budget, as the engine's interrupt handler spends it, and the latch that says
/// which bound ended the run.
#[derive(Debug)]
struct Bounds {
    /// When the run must be over.
    deadline: Instant,
    /// How many engine checks this budget started with, which a breach reports.
    budget: u64,
    /// How many engine checks are left.
    checks: AtomicU64,
    /// Set once the clock ran out.
    timed_out: AtomicBool,
    /// Set once the checks ran out.
    steps_spent: AtomicBool,
}

impl Bounds {
    /// A budget of the checks and the clock one run is held to.
    fn new() -> Self {
        Self::with_budget(RUN_ENGINE_CHECKS)
    }

    /// A budget of `budget` checks starting now — the one constructor, so a budget small
    /// enough to spend in a test is spent through exactly the path a run's is.
    fn with_budget(budget: u64) -> Self {
        Self {
            deadline: Instant::now() + RUN_WALL_CLOCK,
            budget,
            checks: AtomicU64::new(budget),
            timed_out: AtomicBool::new(false),
            steps_spent: AtomicBool::new(false),
        }
    }

    /// Whether the run is over, asked by the engine as it works: the latch first, then
    /// the clock, then the step budget. Each answer latches, so a breach survives a guest
    /// that catches the exception the engine raises in it.
    fn spent(&self) -> bool {
        if self.steps_spent.load(Ordering::Relaxed) {
            return true;
        }
        if self.stop_if_due() {
            return true;
        }
        let before = self
            .checks
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                Some(left.saturating_sub(1))
            })
            .unwrap_or_default();
        if before <= 1 {
            self.steps_spent.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// Whether the run's clock has run out, latching it when it has. Read by the host
    /// where the engine is not running code of its own, so a run is stopped between jobs
    /// as well as inside one.
    fn stop_if_due(&self) -> bool {
        if self.steps_spent.load(Ordering::Relaxed) {
            return true;
        }
        if Instant::now() >= self.deadline {
            self.timed_out.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// What the breached bound is reported as, or `None` when none was breached.
    fn breach(&self) -> Option<String> {
        if self.timed_out.load(Ordering::Relaxed) {
            return Some(format!(
                "the guest was stopped after {} seconds without answering",
                RUN_WALL_CLOCK.as_secs()
            ));
        }
        if self.steps_spent.load(Ordering::Relaxed) {
            return Some(format!(
                "the guest was stopped after {} engine steps without answering",
                self.budget
            ));
        }
        None
    }
}

/// The runtime a MicroVM guest runs inside: the modules it can require, the timers it can
/// set, the server it registers and the sockets it opens.
///
/// It is an expression evaluating to the driver the HOST calls, and it captures the three
/// host functions and the engine builtins it needs before any of the guest's code is
/// compiled, so a guest that rebinds `JSON`, `String` or the global object cannot change
/// how its own response is rendered or reach the host functions again.
///
/// Every module here is this emulator's own: `net` opens no socket (where a connection
/// goes is the host's decision), `http` serves the one proxied request, and there is no
/// filesystem, process, environment or thread module at all — a guest requiring one gets
/// the refusal a missing module gets.
const RUNTIME_SOURCE: &str = r##"
(function () {
    var hostRespond = globalThis.__microvmHostRespond;
    var hostConnect = globalThis.__microvmHostConnect;
    var hostLog = globalThis.__microvmHostLog;
    delete globalThis.__microvmHostRespond;
    delete globalThis.__microvmHostConnect;
    delete globalThis.__microvmHostLog;
    var stringify = JSON.stringify;
    var parse = JSON.parse;
    var toText = String;

    /* ---- the guest's own clock: due instants, never a sleep ---- */
    var clock = 0;
    var timers = [];
    var order = 0;
    function schedule(fn, delay, args, every) {
        if (typeof fn !== "function") { return { live: false }; }
        var due = Number(delay);
        if (!isFinite(due) || due < 0) { due = 0; }
        var timer = { at: clock + due, seq: order++, fn: fn, args: args || [],
                      live: true, every: every ? due : null };
        timers.push(timer);
        return timer;
    }
    function clearTimer(timer) { if (timer && typeof timer === "object") { timer.live = false; } }
    function tick() {
        var next = null;
        for (var i = 0; i < timers.length; i++) {
            var timer = timers[i];
            if (!timer.live) { continue; }
            if (next === null || timer.at < next.at || (timer.at === next.at && timer.seq < next.seq)) {
                next = timer;
            }
        }
        if (next === null) { return false; }
        if (next.at > clock) { clock = next.at; }
        if (next.every === null) { next.live = false; } else { next.at = clock + next.every; }
        next.fn.apply(null, next.args);
        return true;
    }

    /* ---- the console the guest writes through ---- */
    function rendered(value) {
        if (typeof value === "string") { return value; }
        if (value instanceof Error) { return toText(value); }
        try {
            var text = stringify(value);
            return text === undefined ? toText(value) : text;
        } catch (error) { return toText(value); }
    }
    function write() {
        hostLog(Array.prototype.map.call(arguments, rendered).join(" "));
    }

    /* ---- events ---- */
    function Emitter() { this._listeners = {}; }
    Emitter.prototype.on = function (name, fn) {
        var held = this._listeners[name] || (this._listeners[name] = []);
        held.push(fn);
        return this;
    };
    Emitter.prototype.addListener = Emitter.prototype.on;
    Emitter.prototype.once = function (name, fn) {
        var self = this;
        var wrapper = function () { self.removeListener(name, wrapper); fn.apply(self, arguments); };
        return this.on(name, wrapper);
    };
    Emitter.prototype.removeListener = function (name, fn) {
        var held = this._listeners[name];
        if (held) { this._listeners[name] = held.filter(function (one) { return one !== fn; }); }
        return this;
    };
    Emitter.prototype.removeAllListeners = function (name) {
        if (name === undefined) { this._listeners = {}; } else { delete this._listeners[name]; }
        return this;
    };
    Emitter.prototype.setMaxListeners = function () { return this; };
    Emitter.prototype.listenerCount = function (name) {
        return (this._listeners[name] || []).length;
    };
    Emitter.prototype.emit = function (name) {
        var held = (this._listeners[name] || []).slice();
        var args = Array.prototype.slice.call(arguments, 1);
        for (var i = 0; i < held.length; i++) { held[i].apply(this, args); }
        return held.length > 0;
    };

    /* ---- query strings and URLs ---- */
    function parseQuery(text) {
        var query = {};
        if (!text) { return query; }
        var pairs = toText(text).replace(/^\?/, "").split("&");
        for (var i = 0; i < pairs.length; i++) {
            if (pairs[i].length === 0) { continue; }
            var at = pairs[i].indexOf("=");
            var key = at < 0 ? pairs[i] : pairs[i].slice(0, at);
            var value = at < 0 ? "" : pairs[i].slice(at + 1);
            key = decodeURIComponent(key.replace(/\+/g, " "));
            value = decodeURIComponent(value.replace(/\+/g, " "));
            if (query[key] === undefined) { query[key] = value; }
            else if (Array.isArray(query[key])) { query[key].push(value); }
            else { query[key] = [query[key], value]; }
        }
        return query;
    }
    function stringifyQuery(query) {
        var parts = [];
        Object.keys(query || {}).forEach(function (key) {
            var value = query[key];
            var values = Array.isArray(value) ? value : [value];
            values.forEach(function (one) {
                parts.push(encodeURIComponent(key) + "=" + encodeURIComponent(one));
            });
        });
        return parts.join("&");
    }
    function parseUrl(target, withQuery) {
        var text = toText(target === undefined || target === null ? "" : target);
        var hash = "";
        var at = text.indexOf("#");
        if (at >= 0) { hash = text.slice(at); text = text.slice(0, at); }
        var search = "";
        at = text.indexOf("?");
        if (at >= 0) { search = text.slice(at); text = text.slice(0, at); }
        return {
            pathname: text,
            path: text + search,
            search: search.length > 0 ? search : null,
            query: withQuery ? parseQuery(search) : (search.length > 0 ? search.slice(1) : null),
            hash: hash.length > 0 ? hash : null,
            href: text + search + hash
        };
    }
    function SearchParams(init) {
        this._pairs = [];
        var query = typeof init === "string" ? parseQuery(init) : (init || {});
        var self = this;
        Object.keys(query).forEach(function (key) {
            var value = query[key];
            var values = Array.isArray(value) ? value : [value];
            values.forEach(function (one) { self._pairs.push([key, toText(one)]); });
        });
    }
    SearchParams.prototype.get = function (key) {
        for (var i = 0; i < this._pairs.length; i++) {
            if (this._pairs[i][0] === key) { return this._pairs[i][1]; }
        }
        return null;
    };
    SearchParams.prototype.getAll = function (key) {
        return this._pairs.filter(function (pair) { return pair[0] === key; })
                          .map(function (pair) { return pair[1]; });
    };
    SearchParams.prototype.has = function (key) { return this.get(key) !== null; };
    SearchParams.prototype.toString = function () {
        return this._pairs.map(function (pair) {
            return encodeURIComponent(pair[0]) + "=" + encodeURIComponent(pair[1]);
        }).join("&");
    };
    function Url(target, base) {
        var text = toText(target);
        if (base !== undefined && text.indexOf("://") < 0) {
            var root = toText(base).replace(/\/+$/, "");
            text = root + (text.charAt(0) === "/" ? text : "/" + text);
        }
        var scheme = "";
        var at = text.indexOf("://");
        if (at >= 0) { scheme = text.slice(0, at + 1); text = text.slice(at + 3); }
        var authority = "";
        at = text.indexOf("/");
        if (scheme.length > 0) {
            authority = at < 0 ? text : text.slice(0, at);
            text = at < 0 ? "/" : text.slice(at);
        }
        var parsed = parseUrl(text, false);
        this.protocol = scheme;
        this.host = authority;
        this.hostname = authority.split(":")[0];
        this.port = authority.indexOf(":") > 0 ? authority.split(":")[1] : "";
        this.pathname = parsed.pathname;
        this.search = parsed.search === null ? "" : parsed.search;
        this.hash = parsed.hash === null ? "" : parsed.hash;
        this.searchParams = new SearchParams(this.search);
        this.href = scheme + (scheme.length > 0 ? "//" : "") + authority + parsed.pathname +
                    this.search + this.hash;
    }
    Url.prototype.toString = function () { return this.href; };

    /* ---- net: the sockets a guest opens, decided by the MicroVM's connectors ---- */
    var UNANSWERED_CONNECT_MS = 130000;
    function Socket() {
        Emitter.call(this);
        this.connecting = false;
        this.destroyed = false;
        this.remoteAddress = null;
        this.remotePort = null;
        this.localAddress = null;
        this.localPort = null;
        this._timeout = null;
        this._pending = null;
    }
    Socket.prototype = Object.create(Emitter.prototype);
    Socket.prototype.constructor = Socket;
    Socket.prototype.setTimeout = function (ms, fn) {
        var self = this;
        clearTimer(this._timeout);
        if (fn) { this.once("timeout", fn); }
        if (Number(ms) > 0) {
            this._timeout = schedule(function () { self.emit("timeout"); }, ms, [], false);
        }
        return this;
    };
    Socket.prototype.setNoDelay = function () { return this; };
    Socket.prototype.setKeepAlive = function () { return this; };
    Socket.prototype.unref = function () { return this; };
    Socket.prototype.ref = function () { return this; };
    /* The destination is a resource of the emulated network rather than a process on
       this machine, so nothing is delivered either way: what a connection establishes
       is reachability. */
    Socket.prototype.write = function () { return true; };
    Socket.prototype.end = function () { this.destroy(); return this; };
    Socket.prototype.destroy = function () {
        if (this.destroyed) { return this; }
        this.destroyed = true;
        this.connecting = false;
        clearTimer(this._timeout);
        clearTimer(this._pending);
        this.emit("close", false);
        return this;
    };
    function connect() {
        var options = arguments[0];
        var callback = null;
        var index = 1;
        if (typeof options === "number" || typeof options === "string") {
            options = { port: Number(options) };
            if (typeof arguments[1] === "string") { options.host = arguments[1]; index = 2; }
        }
        for (; index < arguments.length; index++) {
            if (typeof arguments[index] === "function") { callback = arguments[index]; }
        }
        var socket = new Socket();
        var host = toText((options && options.host) || "127.0.0.1");
        var port = Number((options && options.port) || 0);
        socket.connecting = true;
        var verdict = parse(hostConnect(host, port));
        if (verdict.connected) {
            socket.remoteAddress = host;
            socket.remotePort = port;
            socket.localAddress = verdict.localAddress;
            socket.localPort = verdict.localPort;
            if (callback) { socket.once("connect", callback); }
            socket._pending = schedule(function () {
                if (socket.destroyed) { return; }
                socket.connecting = false;
                socket.emit("connect");
                socket.emit("ready");
            }, 0, [], false);
            return socket;
        }
        /* Nothing carried the connection, so it never completes: the guest's own
           timeout is what ends it, and where it set none this ceiling does. */
        if (callback) { socket.once("connect", callback); }
        socket._pending = schedule(function () {
            if (socket.destroyed) { return; }
            socket.connecting = false;
            var error = new Error("connect ETIMEDOUT " + host + ":" + port);
            error.code = "ETIMEDOUT";
            error.errno = "ETIMEDOUT";
            error.syscall = "connect";
            error.address = host;
            error.port = port;
            if (socket.listenerCount("error") > 0) { socket.emit("error", error); }
            socket.destroy();
        }, UNANSWERED_CONNECT_MS, [], false);
        return socket;
    }

    /* ---- http: the server the guest registers, and the one request it is handed ---- */
    var servers = [];
    function ServerResponse() {
        Emitter.call(this);
        this.statusCode = 200;
        this.statusMessage = null;
        this.headersSent = false;
        this.finished = false;
        this._headers = [];
        this._chunks = [];
    }
    ServerResponse.prototype = Object.create(Emitter.prototype);
    ServerResponse.prototype.constructor = ServerResponse;
    ServerResponse.prototype.setHeader = function (name, value) {
        var held = this._headers.filter(function (pair) {
            return pair[0].toLowerCase() !== toText(name).toLowerCase();
        });
        held.push([toText(name), toText(value)]);
        this._headers = held;
        return this;
    };
    ServerResponse.prototype.getHeader = function (name) {
        var wanted = toText(name).toLowerCase();
        for (var i = 0; i < this._headers.length; i++) {
            if (this._headers[i][0].toLowerCase() === wanted) { return this._headers[i][1]; }
        }
        return undefined;
    };
    ServerResponse.prototype.removeHeader = function (name) {
        var wanted = toText(name).toLowerCase();
        this._headers = this._headers.filter(function (pair) {
            return pair[0].toLowerCase() !== wanted;
        });
        return this;
    };
    ServerResponse.prototype.writeHead = function (status, reason, headers) {
        this.statusCode = Number(status) || 200;
        var given = headers;
        if (reason && typeof reason === "object") { given = reason; }
        else if (typeof reason === "string") { this.statusMessage = reason; }
        if (given) {
            var self = this;
            Object.keys(given).forEach(function (name) { self.setHeader(name, given[name]); });
        }
        this.headersSent = true;
        return this;
    };
    ServerResponse.prototype.write = function (chunk) {
        if (chunk !== undefined && chunk !== null) { this._chunks.push(toText(chunk)); }
        return true;
    };
    ServerResponse.prototype.end = function (chunk) {
        if (this.finished) { return this; }
        this.write(chunk);
        this.finished = true;
        this.headersSent = true;
        hostRespond(this.statusCode, stringify(this._headers), this._chunks.join(""));
        this.emit("finish");
        return this;
    };
    function IncomingMessage(method, url, headers, body) {
        Emitter.call(this);
        this.method = method;
        this.url = url;
        this.httpVersion = "1.1";
        this.headers = headers;
        this.rawHeaders = [];
        var self = this;
        Object.keys(headers).forEach(function (name) {
            self.rawHeaders.push(name);
            self.rawHeaders.push(headers[name]);
        });
        this.socket = { remoteAddress: null, localAddress: null };
        this._body = body;
    }
    IncomingMessage.prototype = Object.create(Emitter.prototype);
    IncomingMessage.prototype.constructor = IncomingMessage;
    IncomingMessage.prototype.setEncoding = function () { return this; };
    IncomingMessage.prototype.resume = function () { return this; };
    IncomingMessage.prototype.pause = function () { return this; };

    function Server(handler) {
        Emitter.call(this);
        this.listening = false;
        this._port = null;
        if (typeof handler === "function") { this.on("request", handler); }
        servers.push(this);
    }
    Server.prototype = Object.create(Emitter.prototype);
    Server.prototype.constructor = Server;
    Server.prototype.listen = function () {
        var self = this;
        var callback = null;
        for (var i = 0; i < arguments.length; i++) {
            if (typeof arguments[i] === "function") { callback = arguments[i]; }
            else if (typeof arguments[i] === "number") { this._port = arguments[i]; }
            else if (typeof arguments[i] === "object" && arguments[i] &&
                     typeof arguments[i].port === "number") { this._port = arguments[i].port; }
        }
        this.listening = true;
        schedule(function () { self.emit("listening"); if (callback) { callback(); } }, 0, [], false);
        return this;
    };
    Server.prototype.address = function () {
        return this.listening ? { address: "0.0.0.0", family: "IPv4", port: this._port } : null;
    };
    Server.prototype.close = function (callback) {
        this.listening = false;
        if (callback) { schedule(callback, 0, [], false); }
        return this;
    };
    Server.prototype.setTimeout = function () { return this; };
    Server.prototype.unref = function () { return this; };
    Server.prototype.ref = function () { return this; };

    /* ---- the modules a guest can require ---- */
    var core = {
        events: Emitter,
        http: { createServer: function (handler) { return new Server(handler); },
                Server: Server, ServerResponse: ServerResponse,
                IncomingMessage: IncomingMessage, STATUS_CODES: {} },
        net: { connect: connect, createConnection: connect, Socket: Socket },
        url: { parse: parseUrl, URL: Url, URLSearchParams: SearchParams,
               format: function (parts) { return toText(parts && parts.href || ""); } },
        querystring: { parse: parseQuery, stringify: stringifyQuery,
                       escape: encodeURIComponent, unescape: decodeURIComponent },
        os: { platform: function () { return "linux"; }, EOL: "\n" },
        util: { inspect: rendered, format: function () {
            return Array.prototype.map.call(arguments, rendered).join(" "); } }
    };
    core.events.EventEmitter = Emitter;
    core["node:events"] = core.events;

    var modules = {};
    var loaded = {};
    function resolve(from, request) {
        var name = toText(request).replace(/^node:/, "");
        if (name.charAt(0) !== ".") { return { core: name }; }
        var parts = (from.indexOf("/") < 0 ? "" : from.slice(0, from.lastIndexOf("/"))).split("/");
        if (parts.length === 1 && parts[0] === "") { parts = []; }
        name.split("/").forEach(function (segment) {
            if (segment === "." || segment === "") { return; }
            if (segment === "..") { parts.pop(); return; }
            parts.push(segment);
        });
        var path = parts.join("/");
        var candidates = [path, path + ".js", path + ".json", path + "/index.js"];
        for (var i = 0; i < candidates.length; i++) {
            if (Object.prototype.hasOwnProperty.call(modules, candidates[i])) {
                return { path: candidates[i] };
            }
        }
        return { path: path };
    }
    function requireFrom(from) {
        return function (request) {
            var found = resolve(from, request);
            if (found.core !== undefined) {
                if (Object.prototype.hasOwnProperty.call(core, found.core)) {
                    return core[found.core];
                }
                var missing = new Error("Cannot find module '" + toText(request) + "'");
                missing.code = "MODULE_NOT_FOUND";
                throw missing;
            }
            if (Object.prototype.hasOwnProperty.call(loaded, found.path)) {
                return loaded[found.path].exports;
            }
            if (!Object.prototype.hasOwnProperty.call(modules, found.path)) {
                var absent = new Error("Cannot find module '" + toText(request) + "'");
                absent.code = "MODULE_NOT_FOUND";
                throw absent;
            }
            var source = modules[found.path];
            if (/\.json$/.test(found.path)) {
                loaded[found.path] = { exports: parse(source) };
                return loaded[found.path].exports;
            }
            var record = { exports: {}, id: found.path, filename: found.path };
            loaded[found.path] = record;
            var directory = found.path.indexOf("/") < 0 ? "" : found.path.slice(0, found.path.lastIndexOf("/"));
            var compiled = new Function(
                "exports", "require", "module", "__filename", "__dirname", source
            );
            compiled.call(record.exports, record.exports, requireFrom(found.path), record,
                          found.path, directory);
            return record.exports;
        };
    }

    globalThis.setTimeout = function (fn, delay) {
        return schedule(fn, delay, Array.prototype.slice.call(arguments, 2), false);
    };
    globalThis.setInterval = function (fn, delay) {
        return schedule(fn, delay, Array.prototype.slice.call(arguments, 2), true);
    };
    globalThis.setImmediate = function (fn) {
        return schedule(fn, 0, Array.prototype.slice.call(arguments, 1), false);
    };
    globalThis.clearTimeout = clearTimer;
    globalThis.clearInterval = clearTimer;
    globalThis.clearImmediate = clearTimer;
    globalThis.console = { log: write, info: write, debug: write, warn: write,
                           error: write, trace: write };
    globalThis.URL = Url;
    globalThis.URLSearchParams = SearchParams;
    globalThis.require = requireFrom("");

    var driver = {
        start: function (given, entry) {
            modules = given || {};
            globalThis.require = requireFrom(entry);
            requireFrom("")("./" + entry);
            return servers.some(function (server) { return server.listening; });
        },
        dispatch: function (method, url, headers, body) {
            var sent = {};
            parse(headers).forEach(function (pair) { sent[pair[0]] = pair[1]; });
            var request = new IncomingMessage(method, url, sent, body);
            var response = new ServerResponse();
            var serving = null;
            for (var i = servers.length - 1; i >= 0; i--) {
                if (servers[i].listening) { serving = servers[i]; break; }
            }
            if (serving === null) { return; }
            schedule(function () {
                if (body !== undefined && body !== null && body.length > 0) {
                    request.emit("data", body);
                }
                request.emit("end");
            }, 0, [], false);
            serving.emit("request", request, response);
        },
        tick: tick
    };
    return driver;
})()
"##;

#[cfg(test)]
mod tests {
    use super::*;

    /// The seam a run that opens no connection is given: a channel nothing answers, which
    /// is how a guest that never connects is run.
    fn no_connects() -> (
        ConnectSeam,
        ::tokio::sync::mpsc::UnboundedReceiver<ConnectAsk>,
    ) {
        ::tokio::sync::mpsc::unbounded_channel()
    }

    /// One guest program of one module, which is the form a one-file guest is staged in.
    fn program(source: &str) -> Arc<GuestProgram> {
        let mut modules = ::std::collections::BTreeMap::new();
        modules.insert("index.js".to_string(), source.to_string());
        Arc::new(GuestProgram {
            modules,
            entry: "index.js".to_string(),
        })
    }

    /// A GET of `path` through the endpoint.
    fn get(path: &str) -> GuestRequest {
        GuestRequest {
            method: "GET".to_string(),
            url: path.to_string(),
            headers: vec![("host".to_string(), "microvm".to_string())],
            body: String::new(),
        }
    }

    /// RULE: what the endpoint answers is what the GUEST's own program produced — its
    /// status, its headers and its body — and a program that produces no response is
    /// reported as producing none.
    #[test]
    fn the_answer_is_the_one_the_guests_program_produced() {
        // (case, the guest's program, the outcome it must produce)
        let rows: Vec<(&str, &str, GuestOutcome)> = vec![
            (
                "a server answering JSON",
                r#"const http = require('http');
                   http.createServer((req, res) => {
                     res.writeHead(200, {'content-type': 'application/json'});
                     res.end(JSON.stringify({healthy: true, path: req.url}));
                   }).listen(8080);"#,
                GuestOutcome::Answered(GuestAnswer {
                    status: 200,
                    headers: vec![("content-type".to_string(), "application/json".to_string())],
                    body: br#"{"healthy":true,"path":"/health"}"#.to_vec(),
                }),
            ),
            (
                "a server answering its own status",
                r#"require('http').createServer((req, res) => {
                     res.statusCode = 404;
                     res.setHeader('content-type', 'text/plain');
                     res.end('nothing here');
                   }).listen(8080);"#,
                GuestOutcome::Answered(GuestAnswer {
                    status: 404,
                    headers: vec![("content-type".to_string(), "text/plain".to_string())],
                    body: b"nothing here".to_vec(),
                }),
            ),
            (
                "a server answering from a timer",
                r#"require('http').createServer((req, res) => {
                     setTimeout(() => res.end('later'), 2500);
                   }).listen(8080);"#,
                GuestOutcome::Answered(GuestAnswer {
                    status: 200,
                    headers: Vec::new(),
                    body: b"later".to_vec(),
                }),
            ),
            (
                "a server answering from a promise",
                r#"require('http').createServer(async (req, res) => {
                     const value = await Promise.resolve('awaited');
                     res.end(value);
                   }).listen(8080);"#,
                GuestOutcome::Answered(GuestAnswer {
                    status: 200,
                    headers: Vec::new(),
                    body: b"awaited".to_vec(),
                }),
            ),
            (
                "a program that registers no server",
                "const x = 1 + 1;",
                GuestOutcome::NoResponse(NO_LISTENER.to_string()),
            ),
            (
                "a program that raises while starting",
                "throw new Error('the guest failed to start');",
                GuestOutcome::NoResponse(
                    "the guest raised before it answered: the guest failed to start".to_string(),
                ),
            ),
            (
                "a server that never answers",
                "require('http').createServer((req, res) => {}).listen(8080);",
                GuestOutcome::NoResponse(NO_RESPONSE.to_string()),
            ),
        ];

        for (case, source, expected) in rows {
            let (connects, _answers) = no_connects();
            let actual = run(&program(source), &get("/health"), &connects);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a guest reaches no host of this machine — its runtime holds no filesystem,
    /// process, child-process or host-library module, and requiring one raises in the
    /// guest exactly as a missing module does.
    #[test]
    fn no_host_reaching_module_is_reachable_from_a_guest() {
        for name in [
            "fs", "node:fs", "child_process", "process", "worker_threads", "vm", "dns",
            "tls", "http2", "cluster", "module", "v8", "inspector", "dgram", "repl",
        ] {
            let source = format!(
                "require('http').createServer((req, res) => {{
                   try {{ require('{name}'); res.end('reached'); }}
                   catch (error) {{ res.end('refused:' + error.code); }}
                 }}).listen(8080);"
            );
            let (connects, _answers) = no_connects();
            let actual = run(&program(&source), &get("/"), &connects);
            let expected = GuestOutcome::Answered(GuestAnswer {
                status: 200,
                headers: Vec::new(),
                body: b"refused:MODULE_NOT_FOUND".to_vec(),
            });
            assert_eq!(
                actual, expected,
                "case {name:?}: expected the module to be unreachable, got {actual:?}"
            );
        }
    }

    /// RULE: every bound on a run is the host's, and a guest that does not return under
    /// one produces NO response: the step budget stops a loop with no exit (however it
    /// catches what the engine raises in it) and the engine's own heap ceiling ends a run
    /// that allocates without end.
    #[test]
    fn a_guest_that_never_returns_is_stopped_by_a_bound() {
        let rows: Vec<(&str, &str)> = vec![
            (
                "a loop with no exit",
                "require('http').createServer((req, res) => { while (true) {} })
                   .listen(8080);",
            ),
            (
                "a loop that swallows the engine's stop",
                "require('http').createServer((req, res) => {
                   while (true) { try { JSON.parse('{}'); } catch (error) {} } })
                   .listen(8080);",
            ),
            (
                "a loop that allocates",
                "require('http').createServer((req, res) => {
                   let held = []; while (true) { held.push(new Array(1000).fill('x')); } })
                   .listen(8080);",
            ),
        ];
        for (case, source) in rows {
            let (connects, _answers) = no_connects();
            let actual = run(&program(source), &get("/"), &connects);
            let stopped = matches!(&actual, GuestOutcome::NoResponse(_));
            assert!(
                stopped,
                "case {case:?}: expected the run to produce no response, got {actual:?}"
            );
        }
    }

    /// RULE: the host seams the guest's runtime is built over are reachable from NOTHING
    /// in the guest: they are deleted from the global object before any of its code is
    /// compiled, so a guest that looks for them finds nothing and cannot answer, log or
    /// open a connection except through the runtime's own modules.
    #[test]
    fn no_host_seam_is_reachable_from_a_guest() {
        // Every name a host function is installed under, plus the shapes a guest would
        // reach one through if one were left behind.
        let probes: &[(&str, &str)] = &[
            ("the response sink", "globalThis.__microvmHostRespond"),
            ("the connection seam", "globalThis.__microvmHostConnect"),
            ("the log writer", "globalThis.__microvmHostLog"),
            ("the runtime's own driver", "globalThis.__microvmRuntime"),
            (
                "the response sink by indexed lookup",
                "globalThis['__microvmHostRespond']",
            ),
        ];
        for (case, probe) in probes {
            let source = format!(
                "require('http').createServer((req, res) => {{
                   res.end(String(typeof ({probe})));
                 }}).listen(8080);"
            );
            let (connects, _answers) = no_connects();
            let actual = run(&program(&source), &get("/"), &connects);
            let expected = GuestOutcome::Answered(GuestAnswer {
                status: 200,
                headers: Vec::new(),
                body: b"undefined".to_vec(),
            });
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {probe} to be unreachable, got {actual:?}"
            );
        }
    }

    /// RULE: the engine's own stack ceiling bounds a guest's recursion, so a guest that
    /// recurses without end ends inside the engine — as the error its code sees, or as a
    /// run that produced no response — and never on the host's stack.
    #[test]
    fn a_guest_that_recurses_without_end_is_bounded_by_the_engine() {
        let source = "require('http').createServer((req, res) => {
             const down = (depth) => down(depth + 1);
             try { down(0); res.end('returned'); }
             catch (error) { res.end('bounded'); }
           }).listen(8080);";
        let (connects, _answers) = no_connects();
        let actual = run(&program(source), &get("/"), &connects);
        let bounded = match &actual {
            GuestOutcome::Answered(answer) => answer.body == b"bounded",
            GuestOutcome::NoResponse(_) => true,
            _ => false,
        };
        assert!(
            bounded,
            "expected the recursion to be bounded inside the engine, got {actual:?}"
        );
    }

    /// The step budget is what stops a guest the clock has not caught, so it has to STAY
    /// spent: the engine keeps asking after a breach, and an answer of "keep going" there
    /// would hand the run its budget again.
    #[test]
    fn a_spent_step_budget_stays_spent() {
        const BUDGET: u64 = 8;

        let bounds = Bounds::with_budget(BUDGET);
        for step in 1..BUDGET {
            assert!(
                !bounds.spent(),
                "step {step}: a budget of {BUDGET} must not be spent yet"
            );
        }
        for step in BUDGET..(BUDGET + 1_000) {
            assert!(
                bounds.spent(),
                "step {step}: expected a spent budget to leave the run over"
            );
        }
        assert_eq!(
            bounds.breach(),
            Some(format!(
                "the guest was stopped after {BUDGET} engine steps without answering"
            )),
            "a spent step budget is what the run reports"
        );
    }
}
