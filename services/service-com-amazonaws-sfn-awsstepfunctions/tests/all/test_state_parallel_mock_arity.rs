// SPDX-License-Identifier: CC0-1.0

//! What `TestState` makes of a mocked result standing in for a `Parallel` state's
//! branches — driven over this service's own wire protocol through the production
//! router, so what is asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a `Parallel` state's own result is a JSON array holding one
//! element per branch it runs, so a `mock.result` standing in for that invocation has to
//! be an array of exactly that length — anything else is refused 400
//! `ValidationException` in the service's fixed sentence for it, before the state is
//! simulated. The mock's `fieldValidationMode` does not decide it: that member sets "the
//! level of strictness when validating mocked results against their respective API
//! models" (`smithy_query(query="shape",shape="MockInput")`), and a `Parallel` state
//! calls no API, so no mode turns a fan-out the state could not have run into one.
//!
//! The counter-cases hold the rule to the `Parallel` branch count alone: a state of
//! another type whose mocked result carries another number of items is served, and a
//! mocked `errorOutput` — which stands in for a FAILED invocation rather than for the
//! branches' results — is served whatever the state's branch count is.
//!
//! The harness (`Emulator`, `error_code`) is the one `state_machine_lifecycle` builds;
//! `tests/all/main.rs` is this binary's crate root, so it is reached through
//! `crate::<suite>`.

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{error_code, Emulator, ROLE_ARN};

/// RECORDED text, and the whole message: the sentence is fixed, naming neither the
/// number of items the mock carried nor the number of branches the state declares.
const BRANCH_ARITY_REFUSAL: &str =
    "Mocked result must contain the same number of items as number of Parallel branches.";

/// One branch of a `Parallel` state: a state machine carrying its input through, since
/// what is under test is the state around the branches.
const BRANCH: &str = r#"{"StartAt":"B","States":{"B":{"Type":"Pass","End":true}}}"#;

/// A `Parallel` state declaring `branches` branches, as the whole `definition` a request
/// carries.
fn parallel_state(branches: usize) -> String {
    let declared = vec![BRANCH; branches].join(",");
    format!(r#"{{"Type":"Parallel","Branches":[{declared}],"End":true}}"#)
}

/// `TestState`'s request body for the state `definition` with the mock `mock`.
fn tested(definition: &str, mock: Value) -> Value {
    json!({
        "definition": definition,
        "input": r#"{"items":[1,2]}"#,
        "roleArn": ROLE_ARN,
        "inspectionLevel": "DEBUG",
        "mock": mock,
    })
}

/// One row of the branch-arity table.
struct ArityCase {
    case: &'static str,
    /// How many branches the `Parallel` state under test declares.
    branches: usize,
    /// The `mock.result` the request carries, as the caller wrote it: "a JSON string
    /// containing the mocked result of the state invocation".
    result: &'static str,
    /// The `fieldValidationMode` the mock asks for, where it names one.
    mode: Option<&'static str>,
    /// Whether the request is refused for the branch count.
    refused: bool,
}

/// A mocked result standing in for a `Parallel` state's branches is refused unless it
/// carries exactly one item per declared branch.
///
/// Rows vary along the branch count (1, 2, 3 and 5), along how the mocked item count
/// differs from it (none, fewer, more, one item holding the others), along the JSON TYPE
/// of a result that is no array at all (an object, a string, a number, `null`), and
/// along the validation mode the mock asks for.
#[tokio::test]
async fn a_mocked_parallel_result_must_carry_one_item_per_branch() {
    let cases: [ArityCase; 17] = [
        ArityCase {
            case: "one branch, one item",
            branches: 1,
            result: r#"[{"b":1}]"#,
            mode: None,
            refused: false,
        },
        ArityCase {
            case: "one branch, no item",
            branches: 1,
            result: "[]",
            mode: None,
            refused: true,
        },
        ArityCase {
            case: "one branch, two items",
            branches: 1,
            result: "[1,2]",
            mode: None,
            refused: true,
        },
        ArityCase {
            case: "two branches, two items",
            branches: 2,
            result: r#"[{"b":1},{"b":2}]"#,
            mode: None,
            refused: false,
        },
        ArityCase {
            case: "two branches, one item",
            branches: 2,
            result: "[1]",
            mode: None,
            refused: true,
        },
        ArityCase {
            case: "two branches, three items",
            branches: 2,
            result: "[1,2,3]",
            mode: None,
            refused: true,
        },
        ArityCase {
            case: "two branches, one item holding two",
            branches: 2,
            result: "[[1,2]]",
            mode: None,
            refused: true,
        },
        ArityCase {
            case: "three branches, three items of mixed types",
            branches: 3,
            result: r#"[null,"two",{"three":3}]"#,
            mode: None,
            refused: false,
        },
        ArityCase {
            case: "three branches, no item",
            branches: 3,
            result: "[]",
            mode: None,
            refused: true,
        },
        ArityCase {
            case: "five branches, five items",
            branches: 5,
            result: "[1,2,3,4,5]",
            mode: None,
            refused: false,
        },
        ArityCase {
            case: "five branches, four items",
            branches: 5,
            result: "[1,2,3,4]",
            mode: None,
            refused: true,
        },
        // A result that is no array carries no branch output at all, whichever JSON
        // type it is.
        ArityCase {
            case: "two branches, an object for a result",
            branches: 2,
            result: r#"{"b":1}"#,
            mode: None,
            refused: true,
        },
        ArityCase {
            case: "one branch, a string for a result",
            branches: 1,
            result: r#""one""#,
            mode: None,
            refused: true,
        },
        ArityCase {
            case: "one branch, a number for a result",
            branches: 1,
            result: "1",
            mode: None,
            refused: true,
        },
        ArityCase {
            case: "one branch, null for a result",
            branches: 1,
            result: "null",
            mode: None,
            refused: true,
        },
        // The branch count is the state's own shape rather than an API model's field,
        // so the mode a mock asks for neither excuses a wrong count nor refuses a right
        // one.
        ArityCase {
            case: "validating nothing does not excuse a wrong branch count",
            branches: 2,
            result: "[1]",
            mode: Some("NONE"),
            refused: true,
        },
        ArityCase {
            case: "the strictest mode serves the right branch count",
            branches: 2,
            result: "[1,2]",
            mode: Some("STRICT"),
            refused: false,
        },
    ];

    let emulator = Emulator::new();
    for ArityCase {
        case,
        branches,
        result,
        mode,
        refused,
    } in cases
    {
        let mut mock = json!({ "result": result });
        if let Some(mode) = mode {
            mock["fieldValidationMode"] = json!(mode);
        }
        let request = tested(&parallel_state(branches), mock);
        let (status, body) = emulator.call("TestState", &request).await;
        if !refused {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the mocked branches to be tested, got {status} with \
                 {body:?}"
            );
            assert_eq!(
                body.get("status").and_then(Value::as_str),
                Some("SUCCEEDED"),
                "case {case:?}: expected the state to produce the mocked branches, got {body:?}"
            );
            // The state's result is the array the mock named, which a state declaring
            // no result-side filter puts out unchanged.
            assert_eq!(
                body.get("output")
                    .and_then(Value::as_str)
                    .map(|text| serde_json::from_str::<Value>(text).expect("JSON output")),
                Some(serde_json::from_str::<Value>(result).expect("a JSON mocked result")),
                "case {case:?}: expected the mocked branches as the output, got {body:?}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {status} with {body:?}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(BRANCH_ARITY_REFUSAL),
            "case {case:?}: expected the refusal {BRANCH_ARITY_REFUSAL:?}, got {body:?}"
        );
    }
}

/// The branch count decides only a `Parallel` state's mocked RESULT: a mocked result for
/// a state of another type, and a mocked error output for a `Parallel` state, are served.
///
/// Rows vary along the state type the mock stands in for and along which outcome the
/// mock names.
#[tokio::test]
async fn only_a_mocked_parallel_result_is_judged_by_the_branch_count() {
    const MAP: &str = r#"{"Type":"Map","ItemProcessor":{"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}},"ItemsPath":"$.items","End":true}"#;
    const TASK: &str = r#"{"Type":"Task","Resource":"arn:aws:states:::lambda:invoke","End":true}"#;

    // (case, the state under test, the mock the request carries, the status it ends in)
    let cases: [(&str, &str, Value, &str); 5] = [
        (
            "a Map state's mocked iterations outnumber its items",
            MAP,
            json!({ "result": "[1,2,3]" }),
            "SUCCEEDED",
        ),
        (
            "a Map state's mocked iterations are fewer than its items",
            MAP,
            json!({ "result": "[]" }),
            "SUCCEEDED",
        ),
        (
            "a Task state's mocked result is no array at all",
            TASK,
            json!({ "result": r#"{"ok":true}"#, "fieldValidationMode": "NONE" }),
            "SUCCEEDED",
        ),
        (
            "a Parallel state's mocked failure stands in for no branch output",
            &parallel_state(3),
            json!({ "errorOutput": { "error": "States.TaskFailed", "cause": "the mock said so" } }),
            "FAILED",
        ),
        (
            "a one-branch Parallel state's mocked failure likewise",
            &parallel_state(1),
            json!({ "errorOutput": { "error": "States.TaskFailed" } }),
            "FAILED",
        ),
    ];

    let emulator = Emulator::new();
    for (case, definition, mock, expected) in cases {
        let request = tested(definition, mock);
        let (status, body) = emulator.call("TestState", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the mocked state to be tested, got {status} with {body:?}"
        );
        assert_eq!(
            body.get("status").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected status {expected:?}, got {body:?}"
        );
    }
}
