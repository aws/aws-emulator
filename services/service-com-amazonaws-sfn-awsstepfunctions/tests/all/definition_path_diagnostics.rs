// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for two DIAGNOSTICS this service words about a caller's own
//! document, driven over this service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! The rules under test:
//!
//! 1. a payload-template member (`Parameters`, `ItemSelector`, `ResultSelector`) whose
//!    `"<name>.$"` value is a path that selects nothing fails the state with
//!    `States.Runtime`, and the cause names the PATH, the FIELD it was specified for
//!    and the document it was looked for in — never only the path;
//! 2. `TestState` with no `stateName` "accepts the definition of a single state", so a
//!    `definition` that is not one is refused as `InvalidDefinition` naming the
//!    required member the SUPPLIED object does not carry, and nothing in the refusal
//!    names a state, a state machine or a JSON pointer the request did not send —
//!    while a `definition` sent WITH a `stateName` is a state machine definition and is
//!    refused in the words every state machine definition of this service is refused
//!    in.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `describe_execution`,
//! `failure`) is the one `state_machine_lifecycle` and `error_messages` build;
//! `tests/all.rs` is this binary's crate root, so each is reached through
//! `crate::<suite>`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{
    asl::RUNTIME_ERROR, INVALID_STATE_MACHINE_DEFINITION,
};

use crate::error_messages::failure;
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, Emulator, PAST_THE_EXECUTION_DELAY,
};

/// The name the ONE state of each definition below carries.
const STATE: &str = "Work";

// ---------------------------------------------------------------------------
// Rule 1: a template field whose path selects nothing
// ---------------------------------------------------------------------------

/// A `"<name>.$"` member whose path selects nothing fails the state, and the cause
/// names the path, the field and the document the path was looked for in.
///
/// Rows differ along WHICH payload template carries the member (`Parameters` of a
/// `Pass`, of a `Task` naming an activity, `ResultSelector`, a `Map`'s
/// `ItemSelector`), along the path's form (a path into the state's data, a path into
/// the context object, a nested member, a bracketed member) and along the field's own
/// name, so what is pinned is the rule rather than one definition.
#[tokio::test]
async fn a_template_field_whose_path_selects_nothing_names_the_field_and_the_path() {
    let emulator = Emulator::new();

    // (case, the one state under test, the execution input, the field the cause must
    // name, the path it must name)
    let cases: Vec<(&str, Value, &str, &str, &str)> = vec![
        (
            "a context-object path no activity task carries",
            json!({
                "Type": "Task",
                "Resource": "arn:aws:states:us-west-2:000000000000:activity:handoff.waitForTaskToken",
                "Parameters": { "TaskToken.$": "$$.Task.Token", "Payload.$": "$" },
                "End": true,
            }),
            "{}",
            "TaskToken.$",
            "$$.Task.Token",
        ),
        (
            "a path into the state's data the input does not carry",
            json!({
                "Type": "Pass",
                "Parameters": { "who.$": "$.name" },
                "End": true,
            }),
            r#"{"other":1}"#,
            "who.$",
            "$.name",
        ),
        (
            "a nested path the input stops short of",
            json!({
                "Type": "Pass",
                "Parameters": { "city.$": "$.address.city" },
                "End": true,
            }),
            r#"{"address":{"street":"one"}}"#,
            "city.$",
            "$.address.city",
        ),
        (
            "a bracketed member the input does not carry",
            json!({
                "Type": "Pass",
                "Parameters": { "picked.$": "$['missing']" },
                "End": true,
            }),
            r#"{"present":true}"#,
            "picked.$",
            "$['missing']",
        ),
        (
            "a ResultSelector field over the state's result",
            json!({
                "Type": "Map",
                "ItemsPath": "$.items",
                "ItemProcessor": {
                    "StartAt": "P",
                    "States": { "P": { "Type": "Pass", "End": true } },
                },
                "ResultSelector": { "code.$": "$.status" },
                "End": true,
            }),
            r#"{"items":[1,2]}"#,
            "code.$",
            "$.status",
        ),
        (
            "an ItemSelector field over a Map's item",
            json!({
                "Type": "Map",
                "ItemsPath": "$.items",
                "ItemSelector": { "id.$": "$$.Map.Item.Value.id" },
                "ItemProcessor": {
                    "StartAt": "P",
                    "States": { "P": { "Type": "Pass", "End": true } },
                },
                "End": true,
            }),
            r#"{"items":[{"name":"no id here"}]}"#,
            "id.$",
            "$$.Map.Item.Value.id",
        ),
    ];

    for (index, (case, state, input, field, path)) in cases.into_iter().enumerate() {
        let definition =
            json!({ "StartAt": STATE, "States": { STATE: state } }).to_string();
        let machine = create_machine(&emulator, &format!("path-{index}"), &definition).await;
        let execution = start_execution(&emulator, &machine, "only", input).await;
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);

        let described = describe_execution(&emulator, &execution).await;
        let read = |name: &str| {
            described
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        assert_eq!(
            (read("status").as_str(), read("error").as_str()),
            ("FAILED", RUNTIME_ERROR),
            "case {case:?}: expected FAILED with {RUNTIME_ERROR}, got {described:?}"
        );
        let cause = read("cause");
        // The recorded service words this one sentence about the member it could not
        // build: the path, the field it was specified for, and the document it was
        // looked for in.
        let opening = format!("The JSONPath '{path}' specified for the field '{field}' could not be found in the input '");
        assert!(
            cause.contains(&opening),
            "case {case:?}: expected a cause carrying {opening:?}, got {cause:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: what TestState says about the single state it was given
// ---------------------------------------------------------------------------

/// `TestState` with no `stateName` refuses a `definition` that is not the definition of
/// a single state by naming the required member that object does not carry, and says
/// nothing about any state, state machine or pointer the request did not send.
///
/// Rows span the member the language requires of each state type whose work ONE member
/// declares — `Type` for every state, then `Resource`, `Choices`, `Branches` and
/// `ItemProcessor` — including types the issue that filed this rule never cited, and
/// end with the documents that DO carry each of them, which this refusal must not
/// touch.
#[tokio::test]
async fn test_state_names_the_required_member_the_supplied_state_does_not_carry() {
    let emulator = Emulator::new();

    // (case, the definition sent with no stateName, the member the message must name)
    let missing: Vec<(&str, &str, &str)> = vec![
        ("a state with no Type at all", r#"{"Result":1,"End":true}"#, "Type"),
        ("the empty object", "{}", "Type"),
        (
            "a whole state-machine document, which is a state with no Type",
            r#"{"StartAt":"A","States":{"A":{"Type":"Pass","Next":"B"},"B":{"Type":"Pass","End":true}}}"#,
            "Type",
        ),
        (
            "a Task state, which names its work with Resource",
            r#"{"Type":"Task","End":true}"#,
            "Resource",
        ),
        (
            "a Choice state, whose rules live in Choices",
            r#"{"Type":"Choice","Default":"Elsewhere"}"#,
            "Choices",
        ),
        (
            "a Parallel state, which runs its Branches",
            r#"{"Type":"Parallel","End":true}"#,
            "Branches",
        ),
        (
            "a Map state, whose items go through its ItemProcessor",
            r#"{"Type":"Map","End":true}"#,
            "ItemProcessor",
        ),
    ];

    for (case, definition, member) in missing {
        let (status, code, message) = failure(
            &emulator,
            case,
            "TestState",
            &json!({ "definition": definition }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidDefinition"),
            "case {case:?}: expected 400 InvalidDefinition, got {status} {code} with message \
             {message:?}"
        );
        let expected = format!("Missing required field: '{member}'");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        // Nothing the request did not send is named: no state of a state machine this
        // service wrapped the state in, no pointer into one, and no claim that the
        // request carried a state machine definition.
        for leaked in ["States", "TheStateUnderTest", INVALID_STATE_MACHINE_DEFINITION] {
            assert!(
                !message.contains(leaked),
                "case {case:?}: expected the message to name nothing the request did not send, \
                 got {leaked:?} in {message:?}"
            );
        }
    }

    // The same state types WITH the member they require are single states this
    // operation serves or refuses for another reason — never for a member they carry.
    let carried: Vec<(&str, Value)> = vec![
        (
            "a Task state naming its Resource",
            json!({ "definition": r#"{"Type":"Task","Resource":"arn:aws:states:us-west-2:000000000000:activity:a","End":true}"#, "mock": { "result": r#"{"ok":true}"# } }),
        ),
        (
            "a Choice state carrying its Choices",
            json!({ "definition": r#"{"Type":"Choice","Choices":[{"Variable":"$.n","NumericEquals":1,"Next":"P"}],"Default":"Q"}"#, "input": r#"{"n":1}"# }),
        ),
        (
            "a Map state carrying the legacy Iterator instead",
            json!({ "definition": r#"{"Type":"Map","Iterator":{"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}},"End":true}"#, "mock": { "result": "[]" } }),
        ),
        (
            // One item per branch: a mocked result standing in for this state's one
            // branch, so what the row shows is still the `Branches` member it carries.
            "a Parallel state carrying its Branches",
            json!({ "definition": r#"{"Type":"Parallel","Branches":[{"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}}],"End":true}"#, "mock": { "result": "[{}]" } }),
        ),
    ];
    for (case, body) in carried {
        let (status, parsed) = emulator.call("TestState", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the state carrying its required member to be tested, got \
             {status} with body {parsed:?}"
        );
    }

    // A `definition` sent WITH a `stateName` is a state machine definition, so it is
    // refused in the words every state machine definition of this service is refused
    // in — the wrapper the single-state refusal above must not carry.
    let (status, code, message) = failure(
        &emulator,
        "a state machine definition whose state carries no Type",
        "TestState",
        &json!({
            "definition": r#"{"StartAt":"A","States":{"A":{"Result":1,"End":true}}}"#,
            "stateName": "A",
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidDefinition"),
        "expected 400 InvalidDefinition for a state machine definition, got {status} {code} with \
         message {message:?}"
    );
    assert!(
        message.starts_with(INVALID_STATE_MACHINE_DEFINITION),
        "expected a state machine definition to be refused as \
         {INVALID_STATE_MACHINE_DEFINITION}, got {message:?}"
    );
    assert!(
        message.contains("/States/A"),
        "expected the refusal to point at the state the request DID send, got {message:?}"
    );
}
