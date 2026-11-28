// SPDX-License-Identifier: CC0-1.0

//! What a workflow that READS DynamoDB, waits, and WRITES BACK does to a change made
//! while it was waiting — driven over both services' own wire protocols through the
//! production `MultiServiceRouter` with Step Functions and DynamoDB mounted on ONE
//! engine, the shape `core/router/src/lib.rs` mounts them in.
//!
//! RULE: "A `Wait` state delays the state machine from continuing for a specified time",
//! so the states AFTER a `Wait` do their work when that span has run out and not when the
//! execution started — a `Task` state among them makes its service-integration call at
//! that instant, over the data the state carries, so the call both acts on the service as
//! the wait left it and is itself observable only from then on. A read-then-write workflow
//! therefore OVERWRITES a change committed between its read and its write, and a workflow
//! whose control flow polls for such a change observes it and takes the branch it unlocks.
//! Either way the execution reaches a terminal status.
//!
//! The rows differ along what the rule generalises over: what the span after the read is
//! (one `Wait`, two in a row, a `Wait` a `Choice` loop comes back round to), what the
//! states after it do with it (write back what was read, write what the poll found), and
//! where the wait sits in the definition (between two tasks, inside a loop). The second
//! test pins the INSTANTS the same rule decides: where the execution's clock stands when
//! a `Wait` is left, over the span declared and the number of waits a run passes.
//!
//! Every value asserted here is one this suite wrote through DynamoDB's own API and read
//! back through it: what the workflow did is what that store holds afterwards.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::task_integrations::Chain;

/// The table every row below reads and writes.
const TABLE: &str = "ledger";

/// How long a poll waits for the condition it is waiting on — an execution leaving
/// `RUNNING`, an item reaching a value. Generous next to every span the definitions below
/// declare, because each loop stops as soon as its condition holds, so this bounds only a
/// failure.
const PATIENCE: Duration = Duration::from_secs(90);

/// How long one poll pauses between looks.
const GLANCE: Duration = Duration::from_millis(25);

/// The quantity the item is seeded with, the one a change commits while the workflow
/// waits, and the one a polling workflow writes when it sees that change.
const SEEDED: &str = "1";
/// See [`SEEDED`].
const CHANGED: &str = "99";
/// See [`SEEDED`].
const AFTER_THE_POLL: &str = "7";

/// The `Task` state that reads the item under `key` into `$.read`.
fn read_leg(key: &str, next: &str) -> Value {
    json!({
        "Type": "Task",
        "Resource": "arn:aws:states:::dynamodb:getItem",
        "Parameters": { "TableName": TABLE, "Key": { "pk": { "S": key } } },
        "ResultPath": "$.read",
        "Next": next,
    })
}

/// The `Task` state that writes the quantity the read reported back under `key`.
fn write_back_leg(key: &str) -> Value {
    json!({
        "Type": "Task",
        "Resource": "arn:aws:states:::dynamodb:putItem",
        "Parameters": {
            "TableName": TABLE,
            "Item": { "pk": { "S": key }, "qty": { "N.$": "$.read.Item.qty.N" } },
        },
        "End": true,
    })
}

/// The quantity the item under `key` holds, or `None` where the table holds no such item.
async fn quantity(chain: &Chain, key: &str) -> Option<String> {
    chain
        .item(TABLE, key)
        .await
        .and_then(|item| item["qty"]["N"].as_str().map(str::to_string))
}

/// Poll `execution_arn` until it has left `RUNNING`, and report what it then says.
async fn settled(chain: &Chain, case: &str, execution_arn: &str) -> Value {
    let began = Instant::now();
    loop {
        let described = chain.describe(execution_arn).await;
        if described.get("status").and_then(Value::as_str) != Some("RUNNING") {
            return described;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: expected a terminal status within {PATIENCE:?}, got {described:?}"
        );
        ::tokio::time::sleep(GLANCE).await;
    }
}

/// Poll the item under `key` until it holds `quantity`.
async fn held(chain: &Chain, case: &str, key: &str, wanted: &str) {
    let began = Instant::now();
    loop {
        let held = quantity(chain, key).await;
        if held.as_deref() == Some(wanted) {
            return;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: expected {key} to hold {wanted} within {PATIENCE:?}, got {held:?}"
        );
        ::tokio::time::sleep(GLANCE).await;
    }
}

/// One row of the table below: the case, the key it works on, the definition, and the
/// quantity the item holds once the execution has ended.
struct Row {
    case: &'static str,
    key: &'static str,
    definition: Value,
    /// What the workflow leaves behind: the quantity it read for a row that writes back
    /// what it read, and the one it writes for a row whose poll found the change.
    left: &'static str,
}

/// A read-then-write workflow overwrites a change made while it waits, and a workflow
/// that polls for such a change sees it — each over a span the definition declares, and
/// each reaching a terminal status.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_states_after_a_wait_act_when_the_wait_has_run_out() {
    let chain = Chain::new();
    chain.create_table(TABLE).await;

    let rows = [
        Row {
            case: "one wait between the read and the write",
            key: "one-wait",
            definition: json!({
                "StartAt": "Read",
                "States": {
                    "Read": read_leg("one-wait", "Pause"),
                    "Pause": { "Type": "Wait", "Seconds": 2, "Next": "WriteCopy" },
                    "WriteCopy": write_back_leg("one-wait"),
                },
            }),
            left: SEEDED,
        },
        Row {
            // TWO spans in a row: the states after the second one act when BOTH have run
            // out, so a run is held at each of them in turn.
            case: "two waits in a row between the read and the write",
            key: "two-waits",
            definition: json!({
                "StartAt": "Read",
                "States": {
                    "Read": read_leg("two-waits", "Pause"),
                    "Pause": { "Type": "Wait", "Seconds": 1, "Next": "PauseAgain" },
                    "PauseAgain": { "Type": "Wait", "Seconds": 1, "Next": "WriteCopy" },
                    "WriteCopy": write_back_leg("two-waits"),
                },
            }),
            left: SEEDED,
        },
        Row {
            // The same `Wait` reached again and again: the loop's own read observes the
            // change the next span lets through, and the branch that unlocks decides what
            // the workflow writes.
            case: "a wait a choice loop comes back round to",
            key: "polled",
            definition: json!({
                "StartAt": "Read",
                "States": {
                    "Read": read_leg("polled", "Decide"),
                    "Decide": {
                        "Type": "Choice",
                        "Choices": [{
                            "Variable": "$.read.Item.qty.N",
                            "StringEquals": CHANGED,
                            "Next": "WriteSeen",
                        }],
                        "Default": "Pause",
                    },
                    "Pause": { "Type": "Wait", "Seconds": 1, "Next": "Read" },
                    "WriteSeen": {
                        "Type": "Task",
                        "Resource": "arn:aws:states:::dynamodb:putItem",
                        "Parameters": {
                            "TableName": TABLE,
                            "Item": {
                                "pk": { "S": "polled" },
                                "qty": { "N": AFTER_THE_POLL },
                            },
                        },
                        "End": true,
                    },
                },
            }),
            left: AFTER_THE_POLL,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        chain
            .put_item(
                TABLE,
                &json!({ "pk": { "S": row.key }, "qty": { "N": SEEDED } }),
            )
            .await;
        let machine = chain
            .create(&format!("waited-{index}"), &row.definition)
            .await;
        let execution = chain.start(&machine, "only", &json!({})).await;
        // The change committed while the workflow is waiting. The workflow has done its
        // READ by the time this lands: the wait is what holds it, and the read is before
        // the wait.
        held(&chain, case, row.key, SEEDED).await;
        chain
            .put_item(
                TABLE,
                &json!({ "pk": { "S": row.key }, "qty": { "N": CHANGED } }),
            )
            .await;
        let described = settled(&chain, case, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected SUCCEEDED, got {described:?}"
        );
        assert_eq!(
            quantity(&chain, row.key).await.as_deref(),
            Some(row.left),
            "case {case:?}: expected the write made after the wait to leave {:?}",
            row.left
        );
    }
}

/// The instant a `Wait` state is LEFT at is the instant its span ran out: the history
/// stamps its exit one declared span after its entry — counted ONCE, however many waits
/// the run passes — and the execution's own window covers every one of them.
///
/// The rows differ along the span declared and along how many of them the run passes, and
/// every figure asserted is one the SERVICE reported (the `timestamp` of its own history
/// events, the `startDate`/`stopDate` of its own record) rather than anything this test
/// timed: what is pinned is where the execution's clock stands, which its definition
/// decides.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wait_is_left_at_the_instant_its_span_ran_out() {
    // (case, the spans the run's `Wait` states declare, in order)
    let rows: [(&str, &[u64]); 3] = [
        ("one span of one second", &[1]),
        ("one span of three seconds", &[3]),
        ("two spans in a row", &[1, 2]),
    ];

    let chain = Chain::new();
    chain.create_table(TABLE).await;
    for (index, (case, spans)) in rows.iter().enumerate() {
        let mut states = serde_json::Map::new();
        for (step, seconds) in spans.iter().enumerate() {
            let next = match step + 1 == spans.len() {
                true => "Done".to_string(),
                false => format!("Pause{}", step + 1),
            };
            states.insert(
                format!("Pause{step}"),
                json!({ "Type": "Wait", "Seconds": seconds, "Next": next }),
            );
        }
        states.insert("Done".to_string(), json!({ "Type": "Succeed" }));
        let definition = json!({ "StartAt": "Pause0", "States": states });
        let machine = chain
            .create(&format!("stamped-{index}"), &definition)
            .await;
        let execution = chain.start(&machine, "only", &json!({})).await;
        let described = settled(&chain, case, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected SUCCEEDED, got {described:?}"
        );

        let events = history(&chain, case, &execution).await;
        for (step, seconds) in spans.iter().enumerate() {
            let state = format!("Pause{step}");
            let entered = stamp(case, &events, "WaitStateEntered", &state);
            let exited = stamp(case, &events, "WaitStateExited", &state);
            let declared = i64::try_from(*seconds).expect("a span inside an i64") * 1_000;
            let held = exited - entered;
            assert!(
                held >= declared,
                "case {case:?}: expected {state} to be left no sooner than the {declared} ms it \
                 declares, got {held} ms"
            );
            // Counting the span a second time — once where the run reaches the state and
            // again where it carries on from it — would stamp the exit at twice the
            // declared span, which is what this bounds out. The gap left over it is the
            // service's own: the instant the continuation stamps its events at.
            assert!(
                held < 2 * declared,
                "case {case:?}: expected {state}'s span to be counted once, so an exit \
                 inside {} ms of its entry, got {held} ms",
                2 * declared
            );
        }
        let window = epoch_millis(case, &described, "stopDate")
            - epoch_millis(case, &described, "startDate");
        let total: i64 = spans
            .iter()
            .map(|seconds| i64::try_from(*seconds).expect("a span inside an i64") * 1_000)
            .sum();
        assert!(
            window >= total,
            "case {case:?}: expected the execution's own window to cover the {total} ms its \
             waits declare, got {window} ms"
        );
    }
}

/// The `timestamp` of the one event of `events` of type `event_type` that names the state
/// `state`, in whole milliseconds since the epoch.
fn stamp(case: &str, events: &[Value], event_type: &str, state: &str) -> i64 {
    let found = events
        .iter()
        .find(|event| {
            event.get("type").and_then(Value::as_str) == Some(event_type)
                && [
                    "stateEnteredEventDetails",
                    "stateExitedEventDetails",
                ]
                .iter()
                .any(|details| {
                    event[*details].get("name").and_then(Value::as_str) == Some(state)
                })
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a {event_type} of {state} among {events:?}")
        });
    epoch_millis(case, found, "timestamp")
}

/// An `awsJson1_0` timestamp is epoch SECONDS with a fractional part, so every instant
/// read here — of a history event and of an execution's own record alike — is read
/// through this one reader, in whole milliseconds.
fn epoch_millis(case: &str, body: &Value, name: &str) -> i64 {
    let seconds = body
        .get(name)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a timestamp {name:?} in {body}"));
    (seconds * 1000.0).round() as i64
}

/// The events `execution`'s history carries, in order.
async fn history(chain: &Chain, case: &str, execution: &str) -> Vec<Value> {
    let reported = chain
        .sfn(
            "GetExecutionHistory",
            &json!({ "executionArn": execution }),
        )
        .await;
    reported
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an events list in {reported}"))
}
