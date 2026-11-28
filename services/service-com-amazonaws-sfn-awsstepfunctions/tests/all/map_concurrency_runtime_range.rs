// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the RUN-TIME reading of a `Map` state's concurrency bound, driven
//! over this service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! RULE: as a `Map` state is entered its concurrency bound is READ as a whole number — the
//! figure the definition's `MaxConcurrency` states, held to the 64-bit integer such a
//! stated bound is held in, or the figure its `MaxConcurrencyPath` resolved out of the
//! execution's data, held to the integer the member itself carries — and an execution whose
//! bound is a value no such reading exists for, or a negative one, ends `FAILED` with
//! `States.Runtime` instead of running the state under a bound of the emulator's choosing.
//! The cause names the state, the id of the history event it was entered at, the member the
//! definition wrote and the figure read; the reading is the same whichever
//! `ItemProcessor` mode the state declares.
//!
//! The rows span what the rule generalises over: the FORM the bound is written in (a stated
//! figure, a reference path), what the figure is (inside the reading, one past it, far past
//! it, a fraction, a value of another JSON type, a negative figure), the `ItemProcessor`
//! mode (`INLINE` and `DISTRIBUTED`), and how many items the state is given. Each reading's
//! published bound is tested at the bound and one past it.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `describe_execution`,
//! `string`) is the one `state_machine_lifecycle` builds and the history reader is
//! `execution_history`'s; `tests/all/main.rs` declares every suite of this target, so both
//! are reached through `crate::<suite>`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::execution_history::history;
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, string, Emulator,
};

/// The error an execution that cannot be run under the concurrency it read fails with.
const RUNTIME_ERROR: &str = "States.Runtime";

/// The name of the one `Map` state every row below runs, which its cause must name.
const MAP_STATE: &str = "M";

/// The event the `Map` state is entered at, whose id the cause names.
const MAP_ENTERED: &str = "MapStateEntered";

/// Where a row's bound is written in the definition before its own text is put there, so a
/// row states the figure EXACTLY as a definition writes it — including figures no Rust
/// integer literal holds.
const BOUND_PLACEHOLDER: &str = "<<bound>>";

/// How long a row waits for an execution to reach a terminal status, and how often it
/// looks: generous, because what it waits for is the emulator's own settle delay and never
/// how fast the host runs.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(120);
const LOOK: Duration = Duration::from_millis(25);

/// What a row's execution must end as.
enum Outcome {
    /// It runs, producing one result per item.
    Runs { items: usize },
    /// It fails at the state, with this diagnosis after the sentence naming the state.
    Fails(&'static str),
}

/// One row: how the bound is written, the input that resolves it, the processor mode and
/// the outcome.
struct Row {
    case: &'static str,
    /// The `Map` state's own concurrency members, with [`BOUND_PLACEHOLDER`] wherever the
    /// row's `bound` text belongs.
    declared: Value,
    /// The text that replaces the placeholder, as a definition writes it.
    bound: &'static str,
    mode: &'static str,
    input: Value,
    outcome: Outcome,
}

/// The definition of a single `Map` state over `$.items`, carrying `declared`, whose
/// processor is one `Pass` in `mode`.
fn definition(declared: &Value, mode: &str, bound: &str) -> String {
    let processor_config = match mode {
        "DISTRIBUTED" => json!({ "Mode": "DISTRIBUTED", "ExecutionType": "STANDARD" }),
        other => json!({ "Mode": other }),
    };
    let mut state = json!({
        "Type": "Map",
        "ItemsPath": "$.items",
        "ItemProcessor": {
            "ProcessorConfig": processor_config,
            "StartAt": "Hold",
            "States": { "Hold": { "Type": "Pass", "End": true } },
        },
        "End": true,
    });
    let members = state.as_object_mut().expect("the state is an object");
    for (member, value) in declared.as_object().expect("the declaration is an object") {
        members.insert(member.clone(), value.clone());
    }
    let written = json!({ "StartAt": MAP_STATE, "States": { MAP_STATE: state } }).to_string();
    // The placeholder is written as the JSON STRING it is, so what is replaced is that
    // string including its quotes — leaving the row's own text as the figure the member
    // carries.
    written.replace(&format!("\"{BOUND_PLACEHOLDER}\""), bound)
}

/// Poll `execution` until it reports a status other than `RUNNING`, and answer that record.
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
        tokio::time::sleep(LOOK).await;
    }
}

/// The id of the event the `Map` state was ENTERED at, which the cause of a failure at that
/// state names.
async fn entered_at(emulator: &Emulator, case: &str, execution: &str) -> i64 {
    let events = history(emulator, execution, json!({})).await;
    events
        .iter()
        .find(|event| event.get("type").and_then(Value::as_str) == Some(MAP_ENTERED))
        .and_then(|event| event.get("id").and_then(Value::as_i64))
        .unwrap_or_else(|| panic!("case {case:?}: expected a {MAP_ENTERED} in {events:?}"))
}

/// A `Map` state's concurrency bound is read as a whole number as the state is entered, and
/// an execution whose bound is no such reading fails with `States.Runtime` naming the state,
/// the entry event, the member and the figure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_map_states_concurrency_is_read_as_a_whole_number_at_its_entry() {
    let rows = [
        Row {
            case: "a stated bound one past the 64-bit integer a stated bound is held in",
            declared: json!({ "MaxConcurrency": BOUND_PLACEHOLDER }),
            bound: "9223372036854775808",
            mode: "INLINE",
            input: json!({ "items": [1] }),
            outcome: Outcome::Fails(
                "The MaxConcurrency field cannot be parsed as an integer value: \
                 9223372036854775808",
            ),
        },
        Row {
            case: "the same stated bound under a DISTRIBUTED processor",
            declared: json!({ "MaxConcurrency": BOUND_PLACEHOLDER }),
            bound: "9223372036854775808",
            mode: "DISTRIBUTED",
            input: json!({ "items": [1] }),
            outcome: Outcome::Fails(
                "The MaxConcurrency field cannot be parsed as an integer value: \
                 9223372036854775808",
            ),
        },
        Row {
            case: "a stated bound far past that integer",
            declared: json!({ "MaxConcurrency": BOUND_PLACEHOLDER }),
            bound: "18446744073709551616",
            mode: "INLINE",
            input: json!({ "items": [1] }),
            outcome: Outcome::Fails(
                "The MaxConcurrency field cannot be parsed as an integer value: \
                 18446744073709551616",
            ),
        },
        Row {
            case: "a stated bound carrying a fraction, which is no iteration count",
            declared: json!({ "MaxConcurrency": BOUND_PLACEHOLDER }),
            bound: "2.5",
            mode: "INLINE",
            input: json!({ "items": [1] }),
            outcome: Outcome::Fails(
                "The MaxConcurrency field cannot be parsed as an integer value: 2.5",
            ),
        },
        Row {
            case: "the largest figure that integer holds, which is read and run under",
            declared: json!({ "MaxConcurrency": BOUND_PLACEHOLDER }),
            bound: "9223372036854775807",
            mode: "INLINE",
            input: json!({ "items": [1, 2] }),
            outcome: Outcome::Runs { items: 2 },
        },
        Row {
            case: "the member's documented default of no limit",
            declared: json!({ "MaxConcurrency": BOUND_PLACEHOLDER }),
            bound: "0",
            mode: "DISTRIBUTED",
            input: json!({ "items": [1, 2, 3] }),
            outcome: Outcome::Runs { items: 3 },
        },
        Row {
            case: "a path resolving to a negative figure",
            declared: json!({ "MaxConcurrencyPath": "$.mc" }),
            bound: "0",
            mode: "INLINE",
            input: json!({ "items": [1], "mc": -5 }),
            outcome: Outcome::Fails("MaxConcurrency cannot be negative but was: -5"),
        },
        Row {
            case: "a path resolving past every integer, under a DISTRIBUTED processor",
            declared: json!({ "MaxConcurrencyPath": "$.mc" }),
            bound: "0",
            mode: "DISTRIBUTED",
            input: json!({ "items": [1], "mc": 9_223_372_036_854_775_808_u64 }),
            outcome: Outcome::Fails(
                "The MaxConcurrencyPath field refers to value \"9223372036854775808\" which is \
                 not a valid integer: $.mc",
            ),
        },
        Row {
            case: "a path resolving one past the integer the member carries",
            declared: json!({ "MaxConcurrencyPath": "$.mc" }),
            bound: "0",
            mode: "INLINE",
            input: json!({ "items": [1], "mc": 2_147_483_648_i64 }),
            outcome: Outcome::Fails(
                "The MaxConcurrencyPath field refers to value \"2147483648\" which is not a \
                 valid integer: $.mc",
            ),
        },
        Row {
            case: "a path resolving to a value of another JSON type",
            declared: json!({ "MaxConcurrencyPath": "$.mc" }),
            bound: "0",
            mode: "INLINE",
            input: json!({ "items": [1], "mc": "2" }),
            outcome: Outcome::Fails(
                "The MaxConcurrencyPath field refers to value \"2\" which is not a valid \
                 integer: $.mc",
            ),
        },
        Row {
            case: "a path resolving to the largest figure the member carries",
            declared: json!({ "MaxConcurrencyPath": "$.mc" }),
            bound: "0",
            mode: "INLINE",
            input: json!({ "items": [1, 2], "mc": 2_147_483_647_i64 }),
            outcome: Outcome::Runs { items: 2 },
        },
        Row {
            case: "a path resolving to a small positive figure over three items",
            declared: json!({ "MaxConcurrencyPath": "$.mc" }),
            bound: "0",
            mode: "INLINE",
            input: json!({ "items": [1, 2, 3], "mc": 2 }),
            outcome: Outcome::Runs { items: 3 },
        },
    ];

    let emulator = Emulator::new();
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let machine = create_machine(
            &emulator,
            &format!("map-concurrency-{index}"),
            &definition(&row.declared, row.mode, row.bound),
        )
        .await;
        started.push(
            start_execution(&emulator, &machine, "only", &row.input.to_string()).await,
        );
    }

    for (row, execution) in rows.iter().zip(started) {
        let case = row.case;
        let described = settled(&emulator, case, &execution).await;
        match &row.outcome {
            Outcome::Runs { items } => {
                assert_eq!(
                    (
                        described.get("status").and_then(Value::as_str),
                        described.get("error").and_then(Value::as_str),
                    ),
                    (Some("SUCCEEDED"), None),
                    "case {case:?}: expected the state to run, got {described:?}"
                );
                let output = string(&described, "output");
                let results: Value = serde_json::from_str(&output).unwrap_or_else(|_| {
                    panic!("case {case:?}: expected a JSON output, got {output:?}")
                });
                let reported = results.as_array().map(Vec::len);
                assert_eq!(
                    reported,
                    Some(*items),
                    "case {case:?}: expected one result per item, got {results}"
                );
            }
            Outcome::Fails(diagnosis) => {
                assert_eq!(
                    (
                        described.get("status").and_then(Value::as_str),
                        described.get("error").and_then(Value::as_str),
                    ),
                    (Some("FAILED"), Some(RUNTIME_ERROR)),
                    "case {case:?}: expected FAILED with {RUNTIME_ERROR}, got {described:?}"
                );
                let entered = entered_at(&emulator, case, &execution).await;
                let expected = format!(
                    "An error occurred while executing the state '{MAP_STATE}' (entered at the \
                     event id #{entered}). {diagnosis}"
                );
                assert_eq!(
                    string(&described, "cause"),
                    expected,
                    "case {case:?}: expected the cause {expected:?}, got {described:?}"
                );
            }
        }
    }
}
