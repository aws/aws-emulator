// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the HOST SURFACE a run of the `python3.x` sandbox can reach
//! (`state::execution::python`), driven at the engine the way `run_invocation` drives it: one
//! run per row, on a thread with the stack a run is given.
//!
//! The rule under test: what a run of caller-supplied code can reach of the host is the set of
//! names the host installs for that run and nothing besides — no name that begins a PHASE of
//! the run, or raises one of the BOUNDS the host reads it against, is among them — and that
//! surface is the RUN's own answer, so it is the same whichever way the guest reaches it and
//! whichever clock the host hands the run.
//!
//! Which means a row that reads the surface, or that asks which bound ended a run, is judged
//! against a clock whose span this host's own speed cannot spend
//! (`state::execution::run_clock`): a run judged against this host's own clock is answered by
//! its DEADLINE once the host is slow enough to spend that span while the interpreter is
//! standing up, and a deadline is no answer to what the surface carries — it would make the
//! row's answer a reading of how fast this host happens to be running.
//!
//! The rows differ along what the rule generalises over: HOW the guest reaches the surface
//! (the module's own directory, its namespace mapping, the loaded-module table, a second
//! import of it), which clock the host hands the run (one that stands still, and ones whose
//! own readings drive them, by spans that differ), the step budget the run is given, and —
//! for the names that are NOT there — which part of the host's own phase and bound API the
//! name would belong to.

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::package::PackageModule;
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::python::{run_against, Run};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    Budget, Clock, Completion, Finished, Stop, RUN_STACK_BYTES,
};

/// The module one run's package holds, and the handler it is entered through.
const MODULE: &str = "lambda_function.py";

/// The `Handler` every run here is configured with.
const HANDLER: &str = "lambda_function.handler";

/// How long a row waits for its run's own thread to end before it reports the run as one that
/// did not end at all.
///
/// NOT a bound on the run: every bound a run has is the host's own, held inside the run. This
/// is the backstop that keeps a run which will not end from wedging the suite, the way the
/// service gives such a run up rather than waiting on it for ever. It is far above any span a
/// row's own work takes, so no row is ever answered by it.
const RUN_ANSWER_CEILING: Duration = Duration::from_secs(240);

/// A memory ceiling no row here can reach: the largest `MemorySize` a function may have
/// (`smithy_query(query="shape", shape="MemorySize")`: `@range(min: 128, max: 32768)`), in
/// bytes.
///
/// The ceiling is read from the whole PROCESS as well as from the run
/// (`state::execution::python`), and in a test binary running many suites at once that growth
/// is other suites' and no row's own. The handlers here allocate nothing, so the largest
/// ceiling a function may have keeps the memory bound out of every row's way.
const CEILING_OUT_OF_REACH: u64 = 32_768 * 1024 * 1024;

/// The names the host installs on the module a run can reach (`state::execution::python`
/// installs `log`, `sleep`, `aws`, `db_auth_token`, `request`, `presign` and `pagination`),
/// in the order a sorted reading of them answers.
///
/// None of them begins a phase of the run or raises a bound of it. A name added to the host
/// module has to be added here too, which is the point of reading the surface rather than a
/// list of what is forbidden.
const SURFACE: &[&str] = &[
    "aws",
    "db_auth_token",
    "log",
    "pagination",
    "presign",
    "request",
    "sleep",
];

/// The names a PHASE of a run, or a BOUND the host reads the run against, would be carried
/// by if the guest could reach one: the host's own phase and bound API
/// (`state::execution::python`'s `Bounds::start`, `Bounds::step`, `Bounds::watch`,
/// `Bounds::measure_memory_from_here` and `Bounds::remaining`, and the members of
/// `state::execution::Budget`: `wall_clock`, `steps`, `memory_bytes`, `log_bytes`,
/// `response_bytes`, `recursion_limit`), plus the words a guest would try for the invoke
/// phase itself.
const PHASE_AND_BOUND_NAMES: &[&str] = &[
    "start",
    "step",
    "watch",
    "measure_memory_from_here",
    "remaining",
    "bounds",
    "budget",
    "clock",
    "deadline",
    "invoke",
    "phase",
    "wall_clock",
    "steps",
    "memory_bytes",
    "log_bytes",
    "response_bytes",
    "recursion_limit",
];

/// A budget large enough for any handler here, with every bound a row is not about out of its
/// reach: the handlers read a module's names and return, so a row's answer is the surface and
/// never a bound.
fn budget() -> Budget {
    Budget {
        wall_clock: Duration::from_secs(5),
        steps: 2_000_000,
        memory_bytes: CEILING_OUT_OF_REACH,
        log_bytes: 16 * 1024,
        response_bytes: 64 * 1024,
        recursion_limit: 2_000,
    }
}

/// Run `source` as the one module of a package, with `budget` and `event`, judged against
/// `clock`.
///
/// The answer is waited for rather than joined on, so a run that will not end fails the row
/// that asked for it instead of holding the suite.
fn ran(source: &str, event: Option<&str>, budget: Budget, clock: &Clock, case: &str) -> Finished {
    let modules = vec![PackageModule {
        path: MODULE.to_string(),
        source: source.to_string(),
    }];
    let clock = clock.clone();
    let event = event.map(str::to_string);
    let (answered, answer) = mpsc::channel();
    thread::Builder::new()
        .stack_size(RUN_STACK_BYTES)
        .spawn(move || {
            let environment = BTreeMap::new();
            let finished = run_against(
                &Run {
                    modules: &modules,
                    handler: HANDLER,
                    environment: &environment,
                    event: event.as_deref(),
                    request_id: "11111111-2222-3333-4444-555555555555",
                    invoked_function_arn: "arn:aws:lambda:us-west-2:000000000000:function:probe",
                    client_context: None,
                    tenant_id: None,
                    budget,
                    // The run's own containment is what these rows are about, and no emulator
                    // stands behind them: there is no SDK seam either.
                    aws: None,
                },
                &clock,
            );
            let _ = answered.send(finished);
        })
        .expect("a run thread");
    answer
        .recv_timeout(RUN_ANSWER_CEILING)
        .unwrap_or_else(|_| panic!("case {case}: the run did not end, so nothing of it answered"))
}

/// The JSON the handler returned, or a panic naming what the run came to instead.
fn returned(finished: &Finished, case: &str) -> Value {
    match &finished.completion {
        Completion::Returned(payload) => serde_json::from_str(payload)
            .unwrap_or_else(|_| panic!("case {case}: expected JSON, got {payload:?}")),
        other => panic!("case {case}: expected the handler's own answer, got {other:?}"),
    }
}

/// One row of the surface table: what it is about, how the handler reaches the module, the
/// clock the host hands the run and the step budget it is given.
struct Reach {
    /// What the row is about.
    case: &'static str,
    /// The handler's body, which answers a sorted list of the module's public names.
    body: &'static str,
    /// The clock the run's own bounds are read against.
    clock: Clock,
    /// How many traced events the run may spend.
    steps: u64,
}

/// The surface a run reaches is the one the host installed, whichever way the guest reaches
/// it and whichever clock the host hands the run.
///
/// The rows span how the module is reached — its own directory, its namespace mapping, the
/// loaded-module table and a second import of it — and the clocks the host drives, because the
/// surface is a property of the run's execution environment and not of any of those.
#[test]
fn the_surface_a_run_reaches_is_the_one_the_host_installed() {
    let rows = vec![
        Reach {
            case: "the module's own directory, under a clock that does not advance",
            body: "host = __import__('_lambda_host')\n    \
                   return sorted(n for n in dir(host) if not n.startswith('_'))\n",
            clock: Clock::still(),
            steps: 2_000_000,
        },
        Reach {
            case: "the module's namespace mapping",
            body: "host = __import__('_lambda_host')\n    \
                   return sorted(n for n in vars(host) if not n.startswith('_'))\n",
            clock: Clock::still(),
            steps: 2_000_000,
        },
        Reach {
            case: "the loaded-module table, under a clock its own readings drive",
            body: "import sys\n    \
                   __import__('_lambda_host')\n    \
                   host = sys.modules['_lambda_host']\n    \
                   return sorted(n for n in dir(host) if not n.startswith('_'))\n",
            // A clock a reading drives by a span of its own, so the row's answer is the same
            // under a clock that moves as under one that stands still — and under neither is
            // it a reading of this host's time.
            clock: Clock::driven(Duration::from_micros(1)),
            steps: 2_000_000,
        },
        Reach {
            case: "a second import of the module, under a clock driven by a wider span and a \
                   step budget of its own",
            body: "__import__('_lambda_host')\n    \
                   host = __import__('_lambda_host')\n    \
                   return sorted(n for n in dir(host) if not n.startswith('_'))\n",
            clock: Clock::driven(Duration::from_millis(1)),
            steps: 2_000_000_000,
        },
    ];
    let expected: Value = json!(SURFACE);
    for row in rows {
        let source = format!("def handler(event, context):\n    {}", row.body);
        let finished = ran(
            &source,
            None,
            Budget {
                steps: row.steps,
                ..budget()
            },
            &row.clock,
            row.case,
        );
        let reachable = returned(&finished, row.case);
        assert_eq!(
            reachable, expected,
            "case {}: expected the run to reach {expected} and nothing besides, got {reachable}",
            row.case
        );
    }
}

/// No name that begins a phase of the run, or raises a bound the host reads it against, is on
/// the surface: each one is read from inside a run, over every way a guest reads a name, and
/// every reading answers that the name is absent.
#[test]
fn no_name_that_begins_a_phase_or_raises_a_bound_is_on_the_surface() {
    let source = "def handler(event, context):\n    \
                      host = __import__('_lambda_host')\n    \
                      read = {}\n    \
                      for name in event['names']:\n        \
                          by_call = 'present'\n        \
                          try:\n            \
                              getattr(host, name)\n        \
                          except AttributeError:\n            \
                              by_call = 'absent'\n        \
                          except BaseException as error:\n            \
                              by_call = type(error).__name__\n        \
                          read[name] = {'getattr': by_call,\n                        \
                                        'default': getattr(host, name, None) is None,\n                        \
                                        'in_dir': name in dir(host),\n                        \
                                        'in_vars': name in vars(host)}\n    \
                      return read\n";
    let event = json!({ "names": PHASE_AND_BOUND_NAMES }).to_string();
    let case = "the names a phase or a bound would be carried by";
    let finished = ran(
        source,
        Some(&event),
        budget(),
        // Read against a clock that does not advance: the row is about which NAMES a run can
        // reach, so no span of this host's own time may answer it.
        &Clock::still(),
        case,
    );
    let read = returned(&finished, case);
    for name in PHASE_AND_BOUND_NAMES {
        let answered = read.get(*name).cloned().unwrap_or(Value::Null);
        assert_eq!(
            answered,
            json!({"getattr": "absent", "default": true, "in_dir": false, "in_vars": false}),
            "case {name}: expected the name to be absent from the host surface however it is \
             read, got {answered} in {read}"
        );
    }
}

/// A run that calls every name the surface DOES carry is still ended by a bound it cannot
/// reach: the surface is not where the host's own hook or its bounds live, so hunting over it
/// changes neither.
///
/// The run is given a small step budget and a clock that does not advance, so the step budget
/// is the only bound that can answer it and what the row reads is the guest's own spending
/// rather than this host's speed.
#[test]
fn a_run_that_calls_every_name_on_the_surface_is_still_bounded() {
    let source = "def handler(event, context):\n    \
                      host = __import__('_lambda_host')\n    \
                      for name in dir(host):\n        \
                          try:\n            \
                              getattr(host, name)()\n        \
                          except TypeError:\n            \
                              pass\n    \
                      while True:\n        \
                          pass\n";
    let case = "a run that calls every name the host surface carries and never returns";
    let finished = ran(
        source,
        None,
        Budget {
            steps: 200_000,
            ..budget()
        },
        &Clock::still(),
        case,
    );
    assert!(
        matches!(finished.completion, Completion::Stopped(Stop::Steps)),
        "case {case}: expected the step budget the guest cannot reach to end the run, got {:?}",
        finished.completion
    );
}
