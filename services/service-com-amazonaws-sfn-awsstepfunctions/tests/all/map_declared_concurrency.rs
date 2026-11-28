// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CONCURRENCY a `Map` state declares, driven over this
//! service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! RULE: a `Map` state's concurrency is "an upper bound on how many invocations of the
//! Iterator may run in parallel", and the state RUNS under it. A bound the DEFINITION
//! states as a whole number was already held to the language's own range when the
//! definition was accepted, and one naming more parallelism than a run can have bounds
//! the iterations in flight no more tightly than the largest concurrency a run can have
//! does — so the state runs under that ceiling, every iteration runs, and the state's
//! result carries one entry per item in item order. A figure NO whole-number reading
//! exists for — one past the 64-bit integer a stated bound is held in, and one past the
//! integer the member itself carries where the state resolved the figure as it ran —
//! names no bound the state can run under, and fails the execution with
//! `States.Runtime`.
//!
//! The tables span what the rule generalises over: the figure the bound carries (the
//! member's documented default of no limit, a limit under the item count, the ceiling
//! itself, one past it, and the largest a 64-bit whole number carries), how many items
//! the state iterates, the FORM the bound is written in (a JSON literal, a reference
//! path, a JSONata expression), and — for a Distributed Map, which reports its own
//! concurrency back — what `DescribeMapRun` answers for a state that stated more than
//! the modelled integer carries.
//!
//! The run-time reading of a bound that is no whole number at all, and of a negative
//! one, is `map_concurrency_runtime_range`'s own table.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `describe_execution`,
//! `string`) is the one `state_machine_lifecycle` builds, and the Distributed Map
//! fixtures are `map_run_updates`'; `tests/all/main.rs` declares every suite of this
//! target, so both are reached through `crate::<suite>`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::map_run_updates::{describe, passing_processor};
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, string, Emulator,
};

/// The largest concurrency a `Map` state's run can have.
///
/// RECORDED value, written out rather than read off the implementation: a Map Run
/// reports its own concurrency through a modelled integer, and `DescribeMapRun` on a
/// `Map` state that declares no `MaxConcurrency` — the member's documented default of
/// `0`, which the Amazon States Language defines as no limit — answers `2147483647`
/// (`sfn.describe-map-run-default-fields-and-execution-counts`).
const CEILING: i64 = 2_147_483_647;

/// The error a state that cannot be run under the concurrency it resolved fails with.
const RUNTIME_ERROR: &str = "States.Runtime";

/// How long a row waits for an execution to reach a terminal status: generous, because
/// what it waits for is the emulator's own settle delay and not the host's speed.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(120);

/// Poll `execution` until it reports a status other than `RUNNING`, and answer that
/// record. Panics naming `case` when [`TERMINAL_DEADLINE`] passes first.
async fn settled(emulator: &Emulator, case: &str, execution: &str) -> Value {
    let deadline = Instant::now() + TERMINAL_DEADLINE;
    loop {
        let described = describe_execution(emulator, execution).await;
        if string(&described, "status") != "RUNNING" {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the execution to reach a terminal status, got {described:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The `output` of a terminal execution, parsed.
fn output_of(case: &str, described: &Value) -> Value {
    let text = described
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an output in {described:?}"));
    serde_json::from_str(text)
        .unwrap_or_else(|_| panic!("case {case:?}: expected JSON output, got {text:?}"))
}

/// One state machine definition holding the single state `state`, as the text
/// `CreateStateMachine` takes.
fn definition(state: Value) -> String {
    json!({ "StartAt": "MapState", "States": { "MapState": state } }).to_string()
}

/// An inline `Map` over `$.items` carrying the members `declared`, whose processor
/// doubles each item's `n` through an intrinsic — so one entry of the state's result
/// comes from each item's own value rather than from a constant.
fn map_over_items(declared: Value) -> Value {
    let mut state = json!({
        "Type": "Map",
        "ItemsPath": "$.items",
        "ItemProcessor": {
            "ProcessorConfig": { "Mode": "INLINE" },
            "StartAt": "Double",
            "States": {
                "Double": {
                    "Type": "Pass",
                    "Parameters": { "doubled.$": "States.MathAdd($.n, $.n)" },
                    "End": true,
                },
            },
        },
        "End": true,
    });
    let members = state.as_object_mut().expect("the state is an object");
    for (member, value) in declared.as_object().expect("the declaration is an object") {
        members.insert(member.clone(), value.clone());
    }
    state
}

/// An execution input of `items` items, each carrying a distinct `n`.
fn items_input(items: usize) -> String {
    let items: Vec<Value> = (1..=items).map(|index| json!({ "n": index * 10 })).collect();
    json!({ "items": items }).to_string()
}

/// What the processor of [`map_over_items`] produces for that input, in item order.
fn doubled_results(items: usize) -> Value {
    (1..=items)
        .map(|index| json!({ "doubled": index * 20 }))
        .collect()
}

// ---------------------------------------------------------------------------
// A bound the definition states runs every iteration, whatever the figure
// ---------------------------------------------------------------------------

/// One row: the figure the `Map` state's `MaxConcurrency` states, and how many items it
/// iterates.
struct DeclaredRow {
    case: &'static str,
    concurrency: Value,
    items: usize,
}

/// A `Map` state whose concurrency the DEFINITION states runs every one of its items
/// and produces one result per item in item order, whatever figure the bound carries.
///
/// The rows differ along the figure — the member's documented default of no limit, a
/// limit below the item count, the ceiling itself, the figure one past it, and the
/// largest figure a 64-bit whole number carries — and along how many items the state
/// iterates.
#[tokio::test]
async fn a_stated_map_concurrency_runs_every_item_whatever_the_figure() {
    let rows = [
        DeclaredRow {
            case: "the member's documented default, which is no limit",
            concurrency: json!(0),
            items: 3,
        },
        DeclaredRow {
            case: "a limit of one, which serialises the iterations",
            concurrency: json!(1),
            items: 4,
        },
        DeclaredRow {
            case: "a limit below the item count",
            concurrency: json!(2),
            items: 5,
        },
        DeclaredRow {
            case: "the largest concurrency a run can have",
            concurrency: json!(CEILING),
            items: 2,
        },
        DeclaredRow {
            case: "one past the largest concurrency a run can have",
            concurrency: json!(CEILING + 1),
            items: 5,
        },
        DeclaredRow {
            case: "a figure far past it",
            concurrency: json!(1_000_000_000_000_i64),
            items: 3,
        },
        DeclaredRow {
            case: "the largest figure a 64-bit whole number carries",
            concurrency: json!(i64::MAX),
            items: 2,
        },
    ];

    let emulator = Emulator::new();
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let state = map_over_items(json!({ "MaxConcurrency": row.concurrency }));
        let machine = create_machine(
            &emulator,
            &format!("stated-concurrency-{index}"),
            &definition(state),
        )
        .await;
        started.push(start_execution(&emulator, &machine, "only", &items_input(row.items)).await);
    }

    for (row, execution) in rows.iter().zip(started) {
        let case = row.case;
        let described = settled(&emulator, case, &execution).await;
        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").and_then(Value::as_str),
            ),
            (Some("SUCCEEDED"), None),
            "case {case:?}: expected SUCCEEDED with no error, got {described:?}"
        );
        let output = output_of(case, &described);
        assert_eq!(
            output,
            doubled_results(row.items),
            "case {case:?}: expected one result per item in item order, got {output}"
        );
    }
}

// ---------------------------------------------------------------------------
// A figure the state resolves as it runs is held to the range
// ---------------------------------------------------------------------------

/// One row: the `Map` state, the execution input that resolves its concurrency, and the
/// diagnosis the cause ends with.
struct ResolvedRow {
    case: &'static str,
    state: Value,
    input: String,
    diagnosis: String,
}

/// A `Map` state written in the other query language `QueryLanguage` selects, whose
/// concurrency is an EXPRESSION it computes as it runs rather than a figure the
/// definition states: its processor carries no JSONPath member, because a state written
/// in JSONata may not.
fn jsonata_map(concurrency: String) -> Value {
    json!({
        "Type": "Map",
        "QueryLanguage": "JSONata",
        "Items": "{% $states.input.items %}",
        "MaxConcurrency": concurrency,
        "ItemProcessor": {
            "ProcessorConfig": { "Mode": "INLINE" },
            "StartAt": "Double",
            "States": { "Double": { "Type": "Pass", "End": true } },
        },
        "End": true,
    })
}

/// A concurrency the `Map` state RESOLVES as it runs is held to the integer the member
/// carries: a figure outside it fails the execution with `States.Runtime`, and the cause
/// names the member the definition wrote, the figure as the whole number it was read as,
/// and — for the reference-path form — the path it came from.
///
/// The rows differ along the FORM the figure is resolved through (a reference path, a
/// JSONata expression) and along what it resolves to (one past the ceiling, a figure far
/// past it, and a fraction, which is no iteration count either).
#[tokio::test]
async fn a_resolved_map_concurrency_outside_the_range_fails_the_execution() {
    let rows = [
        ResolvedRow {
            case: "a path resolving to one past the largest concurrency a run can have",
            state: map_over_items(json!({ "MaxConcurrencyPath": "$.limit" })),
            input: json!({ "items": [{ "n": 1 }], "limit": CEILING + 1 }).to_string(),
            diagnosis: referred_value("2147483648"),
        },
        ResolvedRow {
            case: "a path resolving far past it",
            state: map_over_items(json!({ "MaxConcurrencyPath": "$.limit" })),
            input: json!({ "items": [{ "n": 1 }], "limit": 1_000_000_000_000_i64 }).to_string(),
            diagnosis: referred_value("1000000000000"),
        },
        ResolvedRow {
            case: "a path resolving to a fraction",
            state: map_over_items(json!({ "MaxConcurrencyPath": "$.limit" })),
            input: json!({ "items": [{ "n": 1 }], "limit": 2.5 }).to_string(),
            diagnosis: referred_value("2.5"),
        },
        ResolvedRow {
            case: "a JSONata expression computing one past the largest concurrency",
            state: jsonata_map(format!("{{% {CEILING} + 1 %}}")),
            input: json!({ "items": [{ "n": 1 }] }).to_string(),
            diagnosis: "The MaxConcurrency field cannot be parsed as an integer value: 2147483648"
                .to_string(),
        },
    ];

    let emulator = Emulator::new();
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let machine = create_machine(
            &emulator,
            &format!("resolved-concurrency-{index}"),
            &definition(row.state.clone()),
        )
        .await;
        started.push(start_execution(&emulator, &machine, "only", &row.input).await);
    }

    for (row, execution) in rows.iter().zip(started) {
        let case = row.case;
        let described = settled(&emulator, case, &execution).await;
        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").and_then(Value::as_str),
            ),
            (Some("FAILED"), Some(RUNTIME_ERROR)),
            "case {case:?}: expected FAILED with {RUNTIME_ERROR}, got {described:?}"
        );
        let cause = string(&described, "cause");
        let expected = &row.diagnosis;
        assert!(
            cause.ends_with(expected.as_str()),
            "case {case:?}: expected a cause ending {expected:?}, got {cause:?}"
        );
        assert!(
            cause.starts_with("An error occurred while executing the state 'MapState' (entered at \
                               the event id #"),
            "case {case:?}: expected the cause to name the state it happened in, got {cause:?}"
        );
    }
}

/// The diagnosis a `MaxConcurrencyPath` whose resolved value is no whole number the
/// member carries reports, for a row whose path is `$.limit`.
fn referred_value(stated: &str) -> String {
    format!(
        "The MaxConcurrencyPath field refers to value \"{stated}\" which is not a valid integer: \
         $.limit"
    )
}

// ---------------------------------------------------------------------------
// What a Map Run reports for a bound stated past its own integer
// ---------------------------------------------------------------------------

/// One row: the figure a Distributed Map's `MaxConcurrency` states, and the
/// `maxConcurrency` its Map Run reports.
struct ReportedRow {
    case: &'static str,
    concurrency: Value,
    reported: i64,
}

/// A Distributed Map runs under the concurrency it stated, held to what a run's own
/// concurrency carries, and its Map Run reports that figure: a bound stated past the
/// ceiling is reported as the ceiling rather than refusing the run or overflowing the
/// modelled integer, and every item is still counted.
#[tokio::test]
async fn a_map_run_reports_the_concurrency_its_state_stated_held_to_the_ceiling() {
    let rows = [
        ReportedRow {
            case: "a figure inside the range, reported as stated",
            concurrency: json!(2),
            reported: 2,
        },
        ReportedRow {
            case: "the ceiling itself",
            concurrency: json!(CEILING),
            reported: CEILING,
        },
        ReportedRow {
            case: "one past the ceiling",
            concurrency: json!(CEILING + 1),
            reported: CEILING,
        },
        ReportedRow {
            case: "a figure far past the ceiling",
            concurrency: json!(1_000_000_000_000_i64),
            reported: CEILING,
        },
    ];
    /// How many items every row's Distributed Map iterates.
    const ITEMS: i64 = 3;

    let emulator = Emulator::new();
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let state = json!({
            "Type": "Map",
            "Items": [1, 2, 3],
            "MaxConcurrency": row.concurrency,
            "ItemProcessor": passing_processor(),
            "End": true,
        });
        let machine = create_machine(
            &emulator,
            &format!("reported-concurrency-{index}"),
            &definition(state),
        )
        .await;
        started.push(start_execution(&emulator, &machine, "only", r#"{"go":true}"#).await);
    }

    for (row, execution) in rows.iter().zip(started) {
        let case = row.case;
        let described = settled(&emulator, case, &execution).await;
        assert_eq!(
            string(&described, "status"),
            "SUCCEEDED",
            "case {case:?}: expected SUCCEEDED, got {described:?}"
        );
        let listed = emulator
            .ok("ListMapRuns", &json!({ "executionArn": execution }))
            .await;
        let map_runs = listed
            .get("mapRuns")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            map_runs.len(),
            1,
            "case {case:?}: one Distributed Map state started one Map Run, got {listed:?}"
        );
        let run = describe(&emulator, &string(&map_runs[0], "mapRunArn")).await;
        assert_eq!(
            run.get("maxConcurrency").and_then(Value::as_i64),
            Some(row.reported),
            "case {case:?}: expected maxConcurrency {}, got {run:?}",
            row.reported
        );
        let counts = (
            run.pointer("/itemCounts/total").and_then(Value::as_i64),
            run.pointer("/itemCounts/succeeded").and_then(Value::as_i64),
            run.pointer("/itemCounts/failed").and_then(Value::as_i64),
        );
        assert_eq!(
            counts,
            (Some(ITEMS), Some(ITEMS), Some(0)),
            "case {case:?}: expected every item to have run and succeeded, got {run:?}"
        );
    }
}
