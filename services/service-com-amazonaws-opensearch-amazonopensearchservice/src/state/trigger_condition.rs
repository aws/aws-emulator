// SPDX-License-Identifier: CC0-1.0

//! Whether a monitor's TRIGGER fires: the condition the caller wrote, evaluated over the
//! results of that monitor's own run.
//!
//! "Triggers define the conditions that, if met, generate alerts" — a query-level trigger's
//! condition is a script the CALLER wrote over the run's context (`ctx.results`, the search
//! responses the monitor's inputs produced), and whether it holds is the whole difference
//! between a notification being published and being withheld. So the condition is EXECUTED
//! rather than read: nothing here matches on its text, and nothing decides a trigger by what
//! the request looked like.
//!
//! The engine is the QuickJS interpreter embedded in this process (`rquickjs`), the one
//! several services of this emulator already run caller-supplied code in. A trigger
//! condition is an EXPRESSION over the context — property access, indexing, comparison, the
//! boolean and arithmetic operators, and calls on the values the context carries — and that
//! expression grammar is the one this engine evaluates, so the caller's own source is handed
//! to it unchanged. A source the engine cannot compile, or one that does not produce a
//! boolean, is reported as the TRIGGER's own error, exactly as the plugin reports a condition
//! it could not run, and the trigger does not fire: nothing is published for a condition that
//! was not shown to hold. A declaration spelled in the scripting language's own keywords
//! rather than in that shared expression grammar is such a source — it is reported as a
//! condition this cluster could not compile, and never read as true.
//!
//! # What the condition can reach
//!
//! A FRESH engine per evaluation, so nothing of one trigger is reachable from the next. The
//! context is handed in as a JSON value and the condition is called as a function of it, so
//! the only data it reads is that run's own context. Plain QuickJS registers no filesystem,
//! process, environment, network, host-library or thread API, and this module adds none.
//!
//! # What bounds it
//!
//! Every bound lives in host code the condition cannot reach, and the first two are checked
//! by the engine's own interrupt handler as it works. They are the WHOLE REQUEST's rather
//! than one condition's ([`Budget`]), so a monitor carrying several triggers cannot add its
//! ceilings up:
//!
//! * a WALL CLOCK ([`RUN_WALL_CLOCK`]), so a condition that never returns is stopped — and
//!   once it is spent, no further condition of the same run is evaluated;
//! * a STEP budget ([`RUN_ENGINE_CHECKS`]), so a tight loop making no progress is stopped
//!   even where the clock has not run out;
//! * a MEMORY ceiling ([`RUN_MEMORY_BYTES`]) and a STACK ceiling ([`RUN_STACK_BYTES`]), both
//!   the engine's own and per evaluation, so an allocation or a recursion past them ends the
//!   evaluation inside the engine;
//! * a SOURCE ceiling ([`MAX_CONDITION_BYTES`]), so a condition longer than any the plugin
//!   stores is refused before the engine is handed anything.
//!
//! Which bound was breached is LATCHED in host state ([`Bounds`]) rather than read back out
//! of the engine, so a condition that catches the exception the engine raises in it cannot
//! make the evaluation report a result: once the latch is set, the answer is the stop.
//!
//! WHERE IT RUNS: with no store of this service held, and off the async worker serving the
//! request — the caller of this module is what arranges both
//! (`crate::service::domain_rest::monitor_run`), so the condition occupies neither the store
//! every other caller waits on nor the runtime's ability to serve anything else.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ::rquickjs::{CatchResultExt, Context, Runtime};
use ::serde_json::Value;

/// How long ONE REQUEST's trigger conditions may run, together, before they are stopped.
///
/// A bound of this emulator's own, and ONE for the whole request rather than one per
/// condition: a monitor carries several triggers, so a per-condition clock would let a run
/// add its ceilings up and hold the request open for the product. The budget is created once
/// per run ([`Budget::for_one_run`]) and spent across every condition of it.
const RUN_WALL_CLOCK: Duration = Duration::from_secs(1);

/// How many engine checks ONE REQUEST's conditions may spend, together, for the same reason.
/// The engine asks the interrupt handler as it works, so a loop that allocates nothing and
/// calls nothing still runs the count down and is stopped by it.
const RUN_ENGINE_CHECKS: u64 = 2_000_000;

/// The memory ceiling one condition's engine is built with.
const RUN_MEMORY_BYTES: usize = 8 * 1024 * 1024;

/// The stack ceiling one condition's engine is built with, which is what ends a runaway
/// recursion. Well below the stack of the thread the request is served on, so a recursion
/// past this raises INSIDE the engine rather than overflowing the host thread.
const RUN_STACK_BYTES: usize = 256 * 1024;

/// The longest condition source this module evaluates, in bytes.
const MAX_CONDITION_BYTES: usize = 64 * 1024;

/// What evaluating one trigger condition came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evaluated {
    /// The condition produced a boolean: whether the trigger fires.
    Decided(bool),
    /// The condition could not be evaluated, or produced something that is not a boolean.
    /// The text is what the trigger's own error reports.
    Failed(String),
}

/// What ONE RUN's conditions may spend, together: the clock they share and the engine checks
/// they share, with the latch that says which of the two ended an evaluation.
///
/// Created once per run and handed to every condition of it, so the work one REQUEST can
/// cause is bounded by these two constants however many triggers its monitor carries.
#[derive(Debug)]
pub struct Budget {
    bounds: Arc<Bounds>,
}

impl Budget {
    /// The budget one run of a monitor has.
    #[must_use]
    pub fn for_one_run() -> Self {
        Self {
            bounds: Arc::new(Bounds::new()),
        }
    }
}

/// Evaluate `source` over `context` — the `ctx` of the run — and say whether the trigger
/// fires.
///
/// `context` is this emulator's own run context, built from the monitor and the results its
/// inputs produced, so the condition reads the same `ctx` the message template renders.
/// `budget` is the whole REQUEST's ([`Budget`]), so what this evaluation spends is taken off
/// what the rest of the run has left.
#[must_use]
pub fn evaluate(budget: &Budget, source: &str, context: &Value) -> Evaluated {
    if let Some(breach) = budget.bounds.breach() {
        // A run whose budget an earlier condition already spent evaluates no further
        // condition: the breach is this one's answer too.
        return Evaluated::Failed(breach);
    }
    if source.trim().is_empty() {
        return Evaluated::Failed("the trigger condition carries no script source".to_string());
    }
    if source.len() > MAX_CONDITION_BYTES {
        return Evaluated::Failed(format!(
            "the trigger condition is longer than the {MAX_CONDITION_BYTES} bytes this \
             cluster evaluates"
        ));
    }
    let Ok(context_json) = ::serde_json::to_string(context) else {
        return Evaluated::Failed("the run context could not be rendered".to_string());
    };
    let Ok(runtime) = Runtime::new() else {
        return Evaluated::Failed("the condition engine did not start".to_string());
    };
    runtime.set_memory_limit(RUN_MEMORY_BYTES);
    runtime.set_max_stack_size(RUN_STACK_BYTES);
    let bounds = Arc::clone(&budget.bounds);
    let watched = Arc::clone(&bounds);
    // The only thing that can stop a condition with no exit: the clock cannot preempt a
    // loop, but the engine asks this handler as it works.
    runtime.set_interrupt_handler(Some(Box::new(move || watched.spent())));
    let Ok(engine_context) = Context::full(&runtime) else {
        return Evaluated::Failed("the condition engine did not start".to_string());
    };
    let answered = engine_context.with(|ctx| {
        // Two forms, because a condition is written either way: an EXPRESSION over the
        // context, which is what the plugin's own trigger editor writes, or a block of
        // statements ending in a `return`. The expression form is compiled first and the
        // block form only where the engine could not read the source as an expression, so
        // the caller's source is handed to the engine unchanged either way.
        let compiled = ctx
            .eval::<::rquickjs::Function<'_>, _>(expression_form(source))
            .catch(&ctx)
            .or_else(|_| {
                ctx.eval::<::rquickjs::Function<'_>, _>(statement_form(source))
                    .catch(&ctx)
            });
        let function = match compiled {
            Ok(function) => function,
            Err(caught) => {
                return Evaluated::Failed(format!(
                    "the trigger condition could not be compiled: {}",
                    raised(&caught)
                ))
            }
        };
        let argument = match ctx.json_parse(context_json) {
            Ok(argument) => argument,
            Err(error) => {
                return Evaluated::Failed(format!("the run context could not be read: {error}"))
            }
        };
        match function
            .call::<_, ::rquickjs::Value<'_>>((argument,))
            .catch(&ctx)
        {
            // A trigger condition decides whether the trigger fires, so it has to produce a
            // boolean: anything else is a condition that did not decide, and is reported as
            // the trigger's own error rather than read as true or false.
            Ok(value) => match value.as_bool() {
                Some(decided) => Evaluated::Decided(decided),
                None => Evaluated::Failed(
                    "the trigger condition did not produce a boolean".to_string(),
                ),
            },
            Err(caught) => Evaluated::Failed(format!(
                "the trigger condition raised: {}",
                raised(&caught)
            )),
        }
    });
    // The evaluation is over, so the interrupt handler is taken off before the engine tears
    // itself down: nothing of a stopped condition is held at teardown.
    runtime.set_interrupt_handler(None);
    // A breached bound is the answer, whatever the condition then said or returned: the
    // latch is host state the condition cannot reach.
    match bounds.breach() {
        Some(breach) => Evaluated::Failed(breach),
        None => answered,
    }
}

/// The name the run context is bound to inside the condition — the one the plugin's own
/// trigger conditions read (`ctx.results[0]…`).
const CONTEXT_PARAMETER: &str = "ctx";

/// The condition as a function of the run context whose body RETURNS the expression the
/// caller wrote — the form a trigger condition is written in.
fn expression_form(source: &str) -> String {
    format!("(function ({CONTEXT_PARAMETER}) {{ \"use strict\"; return ({source}); }})")
}

/// The condition as a function of the run context whose body IS what the caller wrote — the
/// form a condition written as statements ending in a `return` is read in, compiled only
/// where the engine could not read the source as an expression.
fn statement_form(source: &str) -> String {
    format!("(function ({CONTEXT_PARAMETER}) {{ \"use strict\"; {source} }})")
}

/// The message a raise carries: the error's own, as the engine worded it.
fn raised(caught: &::rquickjs::CaughtError<'_>) -> String {
    match caught {
        ::rquickjs::CaughtError::Exception(exception) => exception
            .message()
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| exception.to_string()),
        other => other.to_string(),
    }
}

/// What the run's conditions have left to spend, as the engine's interrupt handler spends it,
/// and the latch that says which bound ended the run.
#[derive(Debug)]
struct Bounds {
    /// When the run's conditions must be over.
    deadline: Instant,
    /// How many engine checks are left to the run.
    checks: AtomicU64,
    /// Set once the clock ran out.
    timed_out: AtomicBool,
    /// Set once the checks ran out.
    steps_spent: AtomicBool,
}

impl Bounds {
    /// A budget starting now. The run's conditions share it, so the clock and the checks are
    /// the WHOLE run's.
    fn new() -> Self {
        Self {
            deadline: Instant::now() + RUN_WALL_CLOCK,
            checks: AtomicU64::new(RUN_ENGINE_CHECKS),
            timed_out: AtomicBool::new(false),
            steps_spent: AtomicBool::new(false),
        }
    }

    /// Whether the run's budget is spent, asked by the engine as it works: the clock, then
    /// the step budget. Each answer latches, so the breach survives a condition that catches
    /// the exception the engine raises, and the next condition of the same run reads it too.
    fn spent(&self) -> bool {
        if Instant::now() >= self.deadline {
            self.timed_out.store(true, Ordering::Relaxed);
            return true;
        }
        if self.checks.fetch_sub(1, Ordering::Relaxed) <= 1 {
            self.steps_spent.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// What the breached bound is reported as, or `None` when none was breached.
    fn breach(&self) -> Option<String> {
        if self.timed_out.load(Ordering::Relaxed) {
            return Some(format!(
                "the trigger conditions of this run were stopped after {} second(s)",
                RUN_WALL_CLOCK.as_secs()
            ));
        }
        if self.steps_spent.load(Ordering::Relaxed) {
            return Some(format!(
                "the trigger conditions of this run were stopped after \
                 {RUN_ENGINE_CHECKS} engine steps"
            ));
        }
        None
    }
}
