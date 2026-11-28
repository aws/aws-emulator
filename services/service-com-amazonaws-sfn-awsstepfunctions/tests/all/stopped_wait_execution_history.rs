// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the HISTORY of an execution a `StopExecution` ended while it was
//! suspended inside a `Wait` state, driven over this service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`) through the
//! production `MultiServiceRouter`.
//!
//! RULE: a stop ends the run AT the state it was suspended in, and the history is
//! TRUNCATED there — that state is entered and never left, nothing of its own is recorded
//! for it, the transition it would have made never fires, and the execution's own
//! `ExecutionAborted` is the only abort the history holds, reporting the `error` and
//! `cause` the stop supplied and neither where it supplied neither. The ids stay the run's
//! own numbering, so the abort is the event straight after the entry.
//!
//! The rows differ along what the rule generalises over: how the `Wait`'s duration is
//! written (a literal `Seconds`, a `SecondsPath` into the execution's input), whether the
//! state ends the run or names a `Next` the stop prevents, and which of `error`/`cause` the
//! stop supplies (both, only one, neither).
//!
//! Nothing here waits for a span to pass: the events before a run's own `Wait` are written
//! as the execution starts (`running_execution_history` is that rule), and the stop applies
//! at the call.
//!
//! The harness (`Emulator`, `create_machine`, `string`) is the one
//! `state_machine_lifecycle` builds; `tests/all/main.rs` declares every suite of this
//! target, so it is reached through `crate::state_machine_lifecycle`.

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{create_machine, describe_execution, string, Emulator};

/// The suffix of the event type a state records when the work around it was aborted while
/// that state was still running: `HistoryEventType` models one per kind of state that can
/// still be running (`smithy_query(query="shape",shape="HistoryEventType")`), and the walk
/// writes one for a state another unit's FAILURE stopped — never for a state a stop caught.
const STATE_ABORTED_SUFFIX: &str = "StateAborted";

/// The suffix of the event type a state records when it is LEFT.
const STATE_EXITED_SUFFIX: &str = "StateExited";

/// The events a stopped run's history holds, in order.
const TRUNCATED_HISTORY: [&str; 3] = ["ExecutionStarted", "WaitStateEntered", "ExecutionAborted"];

/// How many events one page of the paged read below carries, so every row's history really
/// is read across pages the way a client reads it.
const PAGE: usize = 2;

/// One row: the definition and input whose run parks in a `Wait`, the members the stop
/// supplies, and the detail its `ExecutionAborted` event must report.
struct Row {
    case: &'static str,
    definition: Value,
    input: Value,
    /// The members `StopExecution` is called with, beside the execution ARN.
    stop: Value,
    /// The `executionAbortedEventDetails` the history's own abort must carry.
    aborted_details: Value,
}

/// Every event of `execution_arn`'s history, read page by page in the order the service
/// reports them.
async fn paged_history(emulator: &Emulator, case: &str, execution_arn: &str) -> Vec<Value> {
    let mut events = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut request = json!({
            "executionArn": execution_arn,
            "maxResults": PAGE,
            "reverseOrder": false,
        });
        if let Some(token) = &next {
            request["nextToken"] = json!(token);
        }
        let answered = emulator.ok("GetExecutionHistory", &request).await;
        let page = answered
            .get("events")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected an events list in {answered:?}"))
            .clone();
        events.extend(page);
        next = answered
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next.is_none() {
            return events;
        }
    }
}

/// The `type` of each event, in the order the history reports them.
fn types(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|event| {
            event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("expected a type in {event:?}"))
                .to_string()
        })
        .collect()
}

/// A run a stop ended inside a `Wait` reports exactly the start, that state's entry and the
/// execution's own abort — no exit of the state, no per-state abort, and nothing of the
/// transition the state would have made.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_inside_a_wait_truncates_the_history_at_that_state() {
    let error = "Sim2Real.OperatorHalt";
    let cause = "stopped while the state was still waiting";
    let rows = [
        Row {
            case: "a Wait of literal Seconds ending the run, stopped with an error and a cause",
            definition: json!({
                "StartAt": "Park",
                "States": { "Park": { "Type": "Wait", "Seconds": 3600, "End": true } },
            }),
            input: json!({}),
            stop: json!({ "error": error, "cause": cause }),
            aborted_details: json!({ "error": error, "cause": cause }),
        },
        Row {
            case: "the same run, stopped with neither an error nor a cause",
            definition: json!({
                "StartAt": "Park",
                "States": { "Park": { "Type": "Wait", "Seconds": 3600, "End": true } },
            }),
            input: json!({}),
            stop: json!({}),
            aborted_details: json!({}),
        },
        Row {
            case: "the same run, stopped with only a cause",
            definition: json!({
                "StartAt": "Park",
                "States": { "Park": { "Type": "Wait", "Seconds": 900, "End": true } },
            }),
            input: json!({}),
            stop: json!({ "cause": cause }),
            aborted_details: json!({ "cause": cause }),
        },
        Row {
            case: "a Wait whose SecondsPath the input resolves, with a Next the stop prevents",
            definition: json!({
                "StartAt": "Hold",
                "States": {
                    "Hold": { "Type": "Wait", "SecondsPath": "$.seconds", "Next": "After" },
                    "After": { "Type": "Succeed" },
                },
            }),
            input: json!({ "seconds": 86_400 }),
            stop: json!({ "error": error }),
            aborted_details: json!({ "error": error }),
        },
    ];

    let emulator = Emulator::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let machine = create_machine(
            &emulator,
            &format!("stopped-in-wait-{index}"),
            &row.definition.to_string(),
        )
        .await;
        let started = emulator
            .ok(
                "StartExecution",
                &json!({
                    "stateMachineArn": machine,
                    "name": "only",
                    "input": row.input.to_string(),
                }),
            )
            .await;
        let execution = string(&started, "executionArn");

        // The run really is parked in the `Wait`: the state is entered, the clock has not
        // reached its end, and no outcome has been reported.
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("RUNNING"),
            "case {case:?}: expected the run to be waiting before it is stopped, got {described:?}"
        );
        let parked = paged_history(&emulator, case, &execution).await;
        assert_eq!(
            types(&parked),
            TRUNCATED_HISTORY[..2].to_vec(),
            "case {case:?}: expected the run to have entered its Wait and written nothing \
             further, got {parked:?}"
        );

        let mut request = json!({ "executionArn": execution.clone() });
        let members = request.as_object_mut().expect("the request is an object");
        for (member, value) in row.stop.as_object().expect("the stop is an object") {
            members.insert(member.clone(), value.clone());
        }
        emulator.ok("StopExecution", &request).await;

        // The record reports what the stop said, and no output: the run produced none.
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").cloned(),
                described.get("cause").cloned(),
                described.get("output").cloned(),
            ),
            (
                Some("ABORTED"),
                row.stop.get("error").cloned(),
                row.stop.get("cause").cloned(),
                None,
            ),
            "case {case:?}: expected the caller's error and cause and no output, got {described:?}"
        );

        // And the history is truncated at the state the stop caught.
        let events = paged_history(&emulator, case, &execution).await;
        let reported = types(&events);
        assert_eq!(
            reported,
            TRUNCATED_HISTORY.to_vec(),
            "case {case:?}: expected exactly {TRUNCATED_HISTORY:?}, got {events:?}"
        );
        assert!(
            !reported.iter().any(|event_type| {
                event_type.ends_with(STATE_ABORTED_SUFFIX)
                    || event_type.ends_with(STATE_EXITED_SUFFIX)
            }),
            "case {case:?}: expected no state of the run to be reported as left or aborted, got \
             {reported:?}"
        );
        // The ids are the run's own numbering: the abort is the event straight after the
        // entry, so nothing shifts the chain a caller follows.
        for (position, event) in events.iter().enumerate() {
            let expected = i64::try_from(position + 1).expect("the position fits");
            let previous = if expected <= 2 { 0 } else { expected - 1 };
            assert_eq!(
                (
                    event.get("id").and_then(Value::as_i64),
                    event.get("previousEventId").and_then(Value::as_i64),
                ),
                (Some(expected), Some(previous)),
                "case {case:?}: expected event {expected} to follow {previous}, got {event:?}"
            );
        }
        let aborted = events
            .last()
            .unwrap_or_else(|| panic!("case {case:?}: expected an abort in {events:?}"));
        let details = aborted
            .get("executionAbortedEventDetails")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected the abort's detail in {aborted:?}"));
        assert_eq!(
            details, row.aborted_details,
            "case {case:?}: expected the abort to report {}, got {details}",
            row.aborted_details
        );
    }
}
