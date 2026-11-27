// SPDX-License-Identifier: CC0-1.0

//! What the SCHEMA of a definition admits, judged where the definition is SUBMITTED:
//! the bounds the Amazon States Language documents for the figures a definition writes,
//! the integration patterns the workflow type being created can perform, the closed
//! field set of every object the language declares fields on, the form a JSONPath
//! reference field and an embedded JSONata expression take, and the variables a field
//! may read.
//!
//! Driven end to end over this service's own wire protocol through the production
//! router, so what is asserted is what a signed SDK request observes — the refusal
//! `CreateStateMachine` answers and the `diagnostics[]`
//! `ValidateStateMachineDefinition` reports for the same definition, which cannot
//! disagree because one walk answers both.
//!
//! The rules under test, each over a table whose rows differ along what the rule
//! generalises over rather than along the one definition that exposed it:
//!
//! 1. a figure the definition writes outside the range the language documents for it is
//!    refused `InvalidDefinition`, stating the bound it broke at the figure itself —
//!    over every bounded figure the language declares, at each published bound and one
//!    past it;
//! 2. an integration PATTERN only the STANDARD workflow type performs is refused where
//!    the definition is submitted as an EXPRESS state machine, and served for a
//!    STANDARD one;
//! 3. a field outside the closed set of the object it is written in is refused, with
//!    the field it is a near miss of suggested — at every object position the language
//!    declares a field set for;
//! 4. a member whose name makes its value a path holds a path or an intrinsic
//!    invocation, and a variable NOTHING in scope assigns is an error where such a
//!    member names it and a warning where a JSONata expression reads it;//! 5. a JSONata expression is the whole value of the field it is written in, and the
//!    members that carry a document carry an object or one expression.
//!
//! The harness (`Emulator`, `create_machine`, `error_code`, `ROLE_ARN`) is the one
//! `state_machine_lifecycle` builds and the validation readers are
//! `definition_validation`'s; `tests/all/main.rs` picks this file up, so each is
//! reached through `crate::<suite>`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::{
    EXPRESS_CALLBACK_UNSUPPORTED, EXPRESS_RUN_A_JOB_UNSUPPORTED, POSSIBLY_UNDEFINED_VARIABLE,
    UNDEFINED_VARIABLE,
};
use service_com_amazonaws_sfn_awsstepfunctions::state::{
    INVALID_STATE_MACHINE_DEFINITION, VALIDATION_RESULT_FAIL, VALIDATION_RESULT_OK,
};

use crate::definition_validation::{diagnostics, result_of, validate};
use crate::state_machine_lifecycle::{error_code, string, Emulator, ROLE_ARN};

/// What `CreateStateMachine` answers for `definition`: the status, the error code, and
/// the message.
async fn created(
    emulator: &Emulator,
    name: &str,
    definition: &Value,
    machine_type: Option<&str>,
) -> (u16, String, String) {
    let mut body = json!({
        "name": name,
        "definition": definition.to_string(),
        "roleArn": ROLE_ARN,
    });
    if let Some(machine_type) = machine_type {
        body["type"] = json!(machine_type);
    }
    let (status, answered) = emulator.call("CreateStateMachine", &body).await;
    let message = match status {
        200 => String::new(),
        _ => string(&answered, "message"),
    };
    (status, error_code(&answered), message)
}

/// The refusal message `CreateStateMachine` states for a definition carrying the one
/// problem `subject` states: the service's own envelope over that subject.
fn refusal(subject: &str) -> String {
    format!("{INVALID_STATE_MACHINE_DEFINITION}: '{subject}'")
}

/// One subject of such a refusal: the code, the detail and the place it was found.
fn subject(code: &str, detail: &str, location: &str) -> String {
    format!("{code}: {detail} at {location}")
}

/// The code every defect of the language's own schema is reported under.
const SCHEMA_VALIDATION_FAILED: &str = "SCHEMA_VALIDATION_FAILED";

/// A state machine definition declaring `states` and starting at the first of them.
fn machine(start: &str, states: Value) -> Value {
    json!({ "StartAt": start, "States": states })
}

/// A one-state definition: the state `name`, of the shape `state`.
fn one_state(name: &str, state: Value) -> Value {
    machine(name, json!({ name: state }))
}

// ---------------------------------------------------------------------------
// Rule 1: the bounds the language documents for a figure
// ---------------------------------------------------------------------------

/// A figure the definition writes outside the range the language documents for it is
/// refused where it is submitted, stating the bound it broke at the figure.
///
/// The rows span every bounded figure the language declares and the OBJECT each is
/// written in — a `Wait` state's duration, a `Task` state's two windows, a `Map`
/// state's concurrency and its two tolerances, a retrier's spacing, and the state
/// machine object's own timeout — each probed at the published bound (accepted) and one
/// step past it (refused), which is what makes the rule about the bound rather than
/// about the sign of a number. The figure catalog and its bounds are the Amazon States
/// Language's own for each object.
#[tokio::test]
async fn a_figure_outside_its_documented_bound_is_refused_at_the_figure() {
    // (case, the definition, the location the bound is reported at, the bound stated)
    let refused: Vec<(&str, Value, &str, &str)> = vec![
        (
            "a Wait state's Seconds below none at all",
            one_state("W", json!({ "Type": "Wait", "Seconds": -1, "End": true })),
            "/States/W/Seconds",
            "Minimum value is 0",
        ),
        (
            // A WINDOW the work has to finish something inside: the language admits
            // none of none at all, so the figure is probed at the second below its
            // floor as well as at the one right below it.
            "a Task state's TimeoutSeconds of no seconds at all",
            one_state(
                "T",
                json!({
                    "Type": "Task",
                    "Resource": "arn:aws:states:::aws-sdk:sts:getCallerIdentity",
                    "TimeoutSeconds": 0,
                    "End": true,
                }),
            ),
            "/States/T/TimeoutSeconds",
            "Minimum value is 1",
        ),
        (
            "a Task state's TimeoutSeconds below none at all",
            one_state(
                "T",
                json!({
                    "Type": "Task",
                    "Resource": "arn:aws:states:::aws-sdk:sts:getCallerIdentity",
                    "TimeoutSeconds": -5,
                    "End": true,
                }),
            ),
            "/States/T/TimeoutSeconds",
            "Minimum value is 1",
        ),
        (
            "a Task state's HeartbeatSeconds of no seconds at all",
            one_state(
                "T",
                json!({
                    "Type": "Task",
                    "Resource": "arn:aws:states:::aws-sdk:sts:getCallerIdentity",
                    "HeartbeatSeconds": 0,
                    "End": true,
                }),
            ),
            "/States/T/HeartbeatSeconds",
            "Minimum value is 1",
        ),
        (
            "a Map state's MaxConcurrency below none at all",
            one_state("M", map_state(json!({ "MaxConcurrency": -1 }))),
            "/States/M/MaxConcurrency",
            "Minimum value is 0",
        ),
        (
            "a Map state's ToleratedFailureCount below none at all",
            one_state("M", map_state(json!({ "ToleratedFailureCount": -2 }))),
            "/States/M/ToleratedFailureCount",
            "Minimum value is 0",
        ),
        (
            "a Map state's ToleratedFailurePercentage below the percentage range",
            one_state("M", map_state(json!({ "ToleratedFailurePercentage": -0.5 }))),
            "/States/M/ToleratedFailurePercentage",
            // A percentage is a fractional figure, so each end of its range is stated
            // as the decimal the language writes it as.
            "Minimum value is 0.0",
        ),
        (
            "a Map state's ToleratedFailurePercentage above the percentage range",
            one_state("M", map_state(json!({ "ToleratedFailurePercentage": 101 }))),
            "/States/M/ToleratedFailurePercentage",
            "Maximum value is 100.0",
        ),
        (
            "a retrier's IntervalSeconds below none at all",
            one_state("T", retrying(json!({ "IntervalSeconds": -1 }))),
            "/States/T/Retry[0]/IntervalSeconds",
            "Minimum value is 0",
        ),
        (
            "a retrier's MaxAttempts below none at all",
            one_state("T", retrying(json!({ "MaxAttempts": -1 }))),
            "/States/T/Retry[0]/MaxAttempts",
            "Minimum value is 0",
        ),
        (
            "a retrier's MaxDelaySeconds below none at all",
            one_state("T", retrying(json!({ "MaxDelaySeconds": -10 }))),
            "/States/T/Retry[0]/MaxDelaySeconds",
            "Minimum value is 0",
        ),
        (
            // The one figure the language bounds at something other than "not
            // negative": a rate below one would shrink the interval it grows.
            "a retrier's BackoffRate below the rate that keeps the interval",
            one_state("T", retrying(json!({ "BackoffRate": 0.9 }))),
            "/States/T/Retry[0]/BackoffRate",
            "Minimum value is 1.0",
        ),
        (
            "the state machine object's own TimeoutSeconds of no seconds at all",
            json!({
                "StartAt": "P",
                "TimeoutSeconds": 0,
                "States": { "P": { "Type": "Pass", "End": true } },
            }),
            "/TimeoutSeconds",
            "Minimum value is 1",
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, definition, location, bound)) in refused.iter().enumerate() {
        let expected = refusal(&subject(SCHEMA_VALIDATION_FAILED, bound, location));
        let (status, code, message) = created(
            &emulator,
            &format!("figure-{index}"),
            definition,
            None,
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidDefinition"),
            "case {case:?}: expected 400 InvalidDefinition, got {status} {code} with \
             {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        // The same walk answers the validation, so the two cannot disagree.
        let validated = validate(
            &emulator,
            case,
            &json!({ "definition": definition.to_string() }),
        )
        .await;
        assert_eq!(
            result_of(&validated),
            VALIDATION_RESULT_FAIL,
            "case {case:?}: expected the validation to FAIL too, got {validated:?}"
        );
    }

    // The complement: every bound AT its published value is admitted, which is what
    // makes the rows above about the bound and not about the member.
    let admitted: Vec<(&str, Value)> = vec![
        (
            "a Wait of no seconds at all",
            one_state("W", json!({ "Type": "Wait", "Seconds": 0, "End": true })),
        ),
        (
            "a Task window of the shortest the language admits",
            one_state(
                "T",
                json!({
                    "Type": "Task",
                    "Resource": "arn:aws:states:::aws-sdk:sts:getCallerIdentity",
                    "TimeoutSeconds": 1,
                    "HeartbeatSeconds": 1,
                    "End": true,
                }),
            ),
        ),
        (
            "a Map state at the ends of its own ranges",
            one_state(
                "M",
                map_state(json!({
                    "MaxConcurrency": 0,
                    "ToleratedFailureCount": 0,
                    "ToleratedFailurePercentage": 100,
                })),
            ),
        ),
        (
            "a retrier at the ends of its own ranges",
            one_state(
                "T",
                retrying(json!({
                    "IntervalSeconds": 0,
                    "MaxAttempts": 0,
                    "MaxDelaySeconds": 0,
                    "BackoffRate": 1.0,
                })),
            ),
        ),
        (
            "a state machine timeout of the shortest the language admits",
            json!({
                "StartAt": "P",
                "TimeoutSeconds": 1,
                "States": { "P": { "Type": "Pass", "End": true } },
            }),
        ),
    ];
    for (index, (case, definition)) in admitted.iter().enumerate() {
        let (status, code, message) = created(
            &emulator,
            &format!("at-the-bound-{index}"),
            definition,
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the bound itself to be admitted, got {status} {code} with \
             {message:?}"
        );
    }
}

/// A `Map` state carrying `members` beside the processor every one of them needs.
fn map_state(members: Value) -> Value {
    let mut state = json!({
        "Type": "Map",
        "ItemProcessor": { "StartAt": "P", "States": { "P": { "Type": "Pass", "End": true } } },
        "End": true,
    });
    for (member, value) in members.as_object().expect("the members of a Map state") {
        state[member] = value.clone();
    }
    state
}

/// A `Task` state whose one retrier carries `members`.
fn retrying(members: Value) -> Value {
    let mut retrier = json!({ "ErrorEquals": ["States.ALL"] });
    for (member, value) in members.as_object().expect("the members of a retrier") {
        retrier[member] = value.clone();
    }
    json!({
        "Type": "Task",
        "Resource": "arn:aws:states:::aws-sdk:sts:getCallerIdentity",
        "Retry": [retrier],
        "End": true,
    })
}

// ---------------------------------------------------------------------------
// Rule 2: the integration patterns a workflow type can perform
// ---------------------------------------------------------------------------

/// A definition asking for an integration pattern the EXPRESS workflow type is refused the
/// DEFINITION of — the callback pattern, and the run-a-job pattern of another Amazon Web
/// Services service's integration — is refused as an EXPRESS state machine, stating the
/// pattern at the `Resource` that asked for it, and is served as a STANDARD one. RECORDED:
/// the one run-a-job integration an EXPRESS create is SERVED is this service's own nested
/// execution, and every execution of that machine then fails at the state — that refusal
/// belongs to the execution and not to the submitted document
/// (`express_run_a_job_refusal` is the rule for both halves).
///
/// The rows differ along the INTEGRATION the pattern is asked of (a queue, a function, a
/// container task — including services not mounted beside this one, so nothing the
/// integration itself answered could decide the refusal) and along the name of the state
/// that asked.
#[tokio::test]
async fn an_express_definition_is_refused_the_patterns_only_standard_performs() {
    // (case, the state name, the Resource, the sentence the refusal states)
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a queue through the callback pattern",
            "SendMessage",
            "arn:aws:states:::sqs:sendMessage.waitForTaskToken",
            EXPRESS_CALLBACK_UNSUPPORTED,
        ),
        (
            "a function through the callback pattern, at a state of another name",
            "HandOverTheToken",
            "arn:aws:states:::lambda:invoke.waitForTaskToken",
            EXPRESS_CALLBACK_UNSUPPORTED,
        ),
        (
            "a container task through the run-a-job pattern",
            "RunTask",
            "arn:aws:states:::ecs:runTask.sync",
            EXPRESS_RUN_A_JOB_UNSUPPORTED,
        ),
        (
            "a query of an unmounted service through the run-a-job pattern's second revision",
            "RunQuery",
            "arn:aws:states:::athena:startQueryExecution.sync:2",
            EXPRESS_RUN_A_JOB_UNSUPPORTED,
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, state, resource, stated)) in rows.iter().enumerate() {
        let definition = one_state(
            state,
            json!({ "Type": "Task", "Resource": resource, "End": true }),
        );
        let expected = refusal(&subject(
            SCHEMA_VALIDATION_FAILED,
            stated,
            &format!("/States/{state}/Resource"),
        ));
        let (status, code, message) = created(
            &emulator,
            &format!("express-pattern-{index}"),
            &definition,
            Some("EXPRESS"),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidDefinition"),
            "case {case:?}: expected 400 InvalidDefinition, got {status} {code} with {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        // The same definition IS a state machine of the STANDARD type: the refusal
        // belongs to the workflow type and not to the definition.
        let (status, code, message) = created(
            &emulator,
            &format!("standard-pattern-{index}"),
            &definition,
            Some("STANDARD"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the STANDARD type to serve it, got {status} {code} with \
             {message:?}"
        );
        // And the validation answers for the type it is asked about, which is what
        // `type` is for: "the target type of state machine for this definition".
        for (machine_type, expected_result) in [
            ("EXPRESS", VALIDATION_RESULT_FAIL),
            ("STANDARD", VALIDATION_RESULT_OK),
        ] {
            let validated = validate(
                &emulator,
                case,
                &json!({ "definition": definition.to_string(), "type": machine_type }),
            )
            .await;
            assert_eq!(
                result_of(&validated),
                expected_result,
                "case {case:?} as {machine_type}: expected {expected_result}, got {validated:?}"
            );
        }
    }

    // A `Resource` naming NO pattern is served as either type, which is the control
    // for the reading of the suffix — and so is one asking for the RUN-A-JOB pattern of this
    // service's OWN nested execution, whichever revision of it: that pattern on that
    // integration is no defect of the submitted document, so the create stores the state
    // machine and every execution of it fails at the state instead.
    let served: Vec<(&str, &str, &str)> = vec![
        (
            "a request-response task, which asks for no pattern at all",
            "Call",
            "arn:aws:states:::aws-sdk:sts:getCallerIdentity",
        ),
        (
            "a nested execution through the run-a-job pattern",
            "CallChild",
            "arn:aws:states:::states:startExecution.sync",
        ),
        (
            "a nested execution through the pattern's second revision",
            "CallChild",
            "arn:aws:states:::states:startExecution.sync:2",
        ),
    ];
    for (index, (case, state, resource)) in served.iter().enumerate() {
        let definition = one_state(
            state,
            json!({ "Type": "Task", "Resource": resource, "End": true }),
        );
        for machine_type in ["EXPRESS", "STANDARD"] {
            let (status, code, message) = created(
                &emulator,
                &format!("served-{index}-{machine_type}"),
                &definition,
                Some(machine_type),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} as {machine_type}: expected 200, got {status} {code} with \
                 {message:?}"
            );
            let validated = validate(
                &emulator,
                case,
                &json!({ "definition": definition.to_string(), "type": machine_type }),
            )
            .await;
            assert_eq!(
                result_of(&validated),
                VALIDATION_RESULT_OK,
                "case {case:?} as {machine_type}: expected {VALIDATION_RESULT_OK}, got \
                 {validated:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the closed field set of each object, and the near miss
// ---------------------------------------------------------------------------

/// A field outside the closed set of the object it is written in is refused, and where
/// it is a NEAR MISS of a field that set declares, the refusal says which field was
/// meant.
///
/// The rows differ along the object the field is written in — every position the
/// language declares a field set for: the state machine object, a state, a `Retry`
/// retrier, a `Catch` catcher, a `Choice` rule, a `Map`'s processor configuration, its
/// item batcher and its item reader's own configuration — and along the KIND of near
/// miss (a character dropped, one added, one changed, two transposed, a difference of
/// case only, and a name that is a near miss of nothing).
#[tokio::test]
async fn an_unsupported_field_names_the_field_it_is_a_near_miss_of() {
    // (case, the definition, the location, the field written, the field meant)
    let rows: Vec<(&str, Value, &str, &str, Option<&str>)> = vec![
        (
            "a state machine object's own field with a character dropped",
            json!({
                "StartAt": "P",
                "Coment": "a near miss of Comment",
                "States": { "P": { "Type": "Pass", "End": true } },
            }),
            "/",
            "Coment",
            Some("Comment"),
        ),
        (
            "a state's field with two characters transposed",
            one_state("P", json!({ "Type": "Pass", "Reuslt": 1, "End": true })),
            "/States/P",
            "Reuslt",
            Some("Result"),
        ),
        (
            "a state's field differing from the declared one only in case",
            one_state("P", json!({ "Type": "Pass", "result": 1, "End": true })),
            "/States/P",
            "result",
            Some("Result"),
        ),
        (
            "a state's field that is a near miss of nothing the type declares",
            one_state(
                "P",
                json!({ "Type": "Pass", "Whatever": 1, "End": true }),
            ),
            "/States/P",
            "Whatever",
            None,
        ),
        (
            "a retrier's field with a character added",
            one_state(
                "T",
                json!({
                    "Type": "Task",
                    "Resource": "arn:aws:states:::aws-sdk:sts:getCallerIdentity",
                    "Retry": [{ "ErrorEquals": ["States.ALL"], "MaxxAttempts": 1 }],
                    "End": true,
                }),
            ),
            "/States/T/Retry[0]",
            "MaxxAttempts",
            Some("MaxAttempts"),
        ),
        (
            "a catcher's field with a character dropped",
            machine(
                "T",
                json!({
                    "T": {
                        "Type": "Task",
                        "Resource": "arn:aws:states:::aws-sdk:sts:getCallerIdentity",
                        "Catch": [{
                            "ErrorEquals": ["States.ALL"],
                            "Next": "Done",
                            "Outpt": { "kept": true },
                        }],
                        "End": true,
                    },
                    "Done": { "Type": "Succeed" },
                }),
            ),
            "/States/T/Catch[0]",
            "Outpt",
            Some("Output"),
        ),
        (
            "a Choice rule's comparator with a character dropped",
            machine(
                "C",
                json!({
                    "C": {
                        "Type": "Choice",
                        "Choices": [{ "Variable": "$.n", "NumericEqual": 1, "Next": "Done" }],
                        "Default": "Done",
                    },
                    "Done": { "Type": "Succeed" },
                }),
            ),
            "/States/C/Choices[0]",
            "NumericEqual",
            Some("NumericEquals"),
        ),
        (
            "a processor configuration's field with a character dropped",
            one_state(
                "M",
                json!({
                    "Type": "Map",
                    "ItemProcessor": {
                        "ProcessorConfig": { "Mod": "INLINE" },
                        "StartAt": "P",
                        "States": { "P": { "Type": "Pass", "End": true } },
                    },
                    "End": true,
                }),
            ),
            "/States/M/ItemProcessor/ProcessorConfig",
            "Mod",
            Some("Mode"),
        ),        (
            "an item batcher's field with a character dropped",
            one_state(
                "M",
                map_state(json!({ "ItemBatcher": { "MaxItemsPerBatc": 10 } })),
            ),
            "/States/M/ItemBatcher",
            "MaxItemsPerBatc",
            Some("MaxItemsPerBatch"),
        ),
        (
            "an item reader's nested configuration, two levels down",
            one_state(
                "M",
                map_state(json!({
                    "ItemReader": {
                        "Resource": "arn:aws:states:::s3:getObject",
                        "ReaderConfig": { "InputTyp": "CSV" },
                    },
                })),
            ),
            "/States/M/ItemReader/ReaderConfig",
            "InputTyp",
            Some("InputType"),
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, definition, location, written, meant)) in rows.iter().enumerate() {
        let detail = match meant {
            Some(meant) => format!("Field '{written}' is not supported. Did you mean '{meant}'?"),
            None => format!("Field '{written}' is not supported"),
        };
        let expected = refusal(&subject(SCHEMA_VALIDATION_FAILED, &detail, location));
        let (status, code, message) = created(
            &emulator,
            &format!("near-miss-{index}"),
            definition,
            None,
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidDefinition"),
            "case {case:?}: expected 400 InvalidDefinition, got {status} {code} with {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }

    // TWO such objects, each carrying an unsupported field: both are reported, in the
    // order the definition writes the objects they sit in.
    let two_objects = one_state(
        "M",
        map_state(json!({
            "ItemBatcher": { "MaxItemsPerBatc": 10 },
            "ResultWriter": { "Resorce": "arn:aws:states:::s3:putObject" },
        })),
    );
    let validated = validate(
        &emulator,
        "two objects of one state, each carrying an unsupported field",
        &json!({ "definition": two_objects.to_string() }),
    )
    .await;
    let located: Vec<(String, String)> = diagnostics(&validated)
        .iter()
        .map(|diagnostic| (string(diagnostic, "location"), string(diagnostic, "message")))
        .collect();
    assert_eq!(
        located,
        vec![
            (
                "/States/M/ItemBatcher".to_string(),
                "Field 'MaxItemsPerBatc' is not supported. Did you mean 'MaxItemsPerBatch'?"
                    .to_string(),
            ),
            (
                "/States/M/ResultWriter".to_string(),
                "Field 'Resorce' is not supported. Did you mean 'Resource'?".to_string(),
            ),
        ],
        "case \"two objects of one state\": expected one diagnostic per object, in the order the \
         definition writes them, got {validated:?}"
    );

    // The complement: every one of those objects carrying the fields the language DOES
    // declare is created, so the rule is about the closed set and not about the object.
    let admitted = one_state(
        "M",
        json!({
            "Type": "Map",
            "Comment": "every field below is one the language declares",
            "ItemReader": {
                "Resource": "arn:aws:states:::s3:getObject",
                "ReaderConfig": { "InputType": "CSV", "CSVHeaderLocation": "FIRST_ROW" },
                "Parameters": { "Bucket": "a-bucket", "Key": "a-key" },
            },
            "ItemBatcher": { "MaxItemsPerBatch": 10, "MaxInputBytesPerBatch": 1024 },
            "ResultWriter": {
                "Resource": "arn:aws:states:::s3:putObject",
                "Parameters": { "Bucket": "a-bucket", "Prefix": "out" },
            },
            "ItemProcessor": {
                "ProcessorConfig": { "Mode": "INLINE" },
                "StartAt": "P",
                "States": { "P": { "Type": "Pass", "End": true } },
            },
            "End": true,
        }),
    );
    let (status, code, message) = created(&emulator, "declared-fields", &admitted, None).await;
    assert_eq!(
        status, 200,
        "case \"every field the language declares\": expected 200, got {status} {code} with \
         {message:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: what a reference field holds, and the variables a field reads
// ---------------------------------------------------------------------------

/// A member whose name ends in `.$` holds a path or an intrinsic invocation, and a
/// variable NOTHING in scope assigns is an ERROR where such a member names it — while a
/// JSONata expression reading one is the warning the definition is still created with.
///
/// The rows differ along the member the reference is written in (a payload template, an
/// assignment block, a template nested inside one), along what it holds (a literal
/// string, a number, an object) and along the two ways a variable is read.
#[tokio::test]
async fn a_reference_field_holds_a_path_and_a_variable_in_scope() {
    // (case, the definition, the location, the field the refusal names)
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "a payload template member holding a literal string",
            one_state(
                "P",
                json!({ "Type": "Pass", "Parameters": { "x.$": "plain" }, "End": true }),
            ),
            "/States/P/Parameters/x.$",
            "x.$",
        ),
        (
            "a payload template member holding a number",
            one_state(
                "P",
                json!({ "Type": "Pass", "Parameters": { "x.$": 1 }, "End": true }),
            ),
            "/States/P/Parameters/x.$",
            "x.$",
        ),
        (
            "a member nested inside a payload template",
            one_state(
                "P",
                json!({
                    "Type": "Pass",
                    "Parameters": { "outer": { "inner.$": "nope" } },
                    "End": true,
                }),
            ),
            "/States/P/Parameters/outer/inner.$",
            "inner.$",
        ),
        (
            "an assignment reading something that is no path at all",
            one_state(
                "P",
                json!({ "Type": "Pass", "Assign": { "total.$": "total" }, "End": true }),
            ),
            "/States/P/Assign/total.$",
            "total.$",
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, definition, location, field)) in rows.iter().enumerate() {
        let detail = format!(
            "The value for the field '{field}' must be a valid JSONPath or intrinsic function \
             expression"
        );
        let expected = refusal(&subject(SCHEMA_VALIDATION_FAILED, &detail, location));
        let (status, code, message) = created(
            &emulator,
            &format!("reference-{index}"),
            definition,
            None,
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidDefinition"),
            "case {case:?}: expected 400 InvalidDefinition, got {status} {code} with {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }

    // TWO reference fields of one payload template, each holding something the language
    // cannot read: both are reported, in the order the template writes them.
    let two_fields = one_state(
        "P",
        json!({
            "Type": "Pass",
            "Parameters": { "first.$": "plain", "nested": { "second.$": 2 } },
            "End": true,
        }),
    );
    let validated = validate(
        &emulator,
        "two reference fields of one payload template",
        &json!({ "definition": two_fields.to_string() }),
    )
    .await;
    let located: Vec<String> = diagnostics(&validated)
        .iter()
        .map(|diagnostic| string(diagnostic, "location"))
        .collect();
    assert_eq!(
        located,
        vec![
            "/States/P/Parameters/first.$".to_string(),
            "/States/P/Parameters/nested/second.$".to_string(),
        ],
        "case \"two reference fields of one payload template\": expected one diagnostic per \
         field, in the order the template writes them, got {validated:?}"
    );

    // What such a member MAY hold: the state's own data, the context object, a
    // variable in scope, and an intrinsic invocation.
    let admitted = machine(
        "Set",
        json!({
            "Set": { "Type": "Pass", "Assign": { "total.$": "$.n" }, "Next": "Read" },
            "Read": {
                "Type": "Pass",
                "Parameters": {
                    "from_input.$": "$.n",
                    "from_context.$": "$$.Execution.Name",
                    "from_variable.$": "$total",
                    "from_intrinsic.$": "States.Format('{}', $.n)",
                },
                "End": true,
            },
        }),
    );
    let (status, code, message) = created(&emulator, "references", &admitted, None).await;
    assert_eq!(
        status, 200,
        "case \"the four forms such a member may hold\": expected 200, got {status} {code} with \
         {message:?}"
    );
}

/// A variable nothing in scope assigns is reported as an ERROR wherever the language
/// RESOLVES the reference and as a WARNING where a JSONata expression reads it — one
/// code and one sentence, at the severity the field decides — and a variable only SOME
/// path to the reading state assigns is the warning either way.
#[tokio::test]
async fn an_unassignable_variable_is_an_error_only_where_the_language_resolves_it() {
    let emulator = Emulator::new();

    // The positions the language resolves a path at: the `*Path` family, whose value IS
    // the path, a payload template's reference field, and a `Choice` rule's `Variable`.
    // Each row reads a variable nothing in scope assigns, so no run of the definition
    // could read it. (case, the state, the location the diagnostic is reported at)
    let resolving: Vec<(&str, Value, &str)> = vec![
        (
            "an InputPath naming a variable nothing assigns",
            json!({ "Type": "Pass", "InputPath": "$missing", "End": true }),
            "/States/S/InputPath",
        ),
        (
            "an OutputPath naming one",
            json!({ "Type": "Pass", "OutputPath": "$missing", "End": true }),
            "/States/S/OutputPath",
        ),
        (
            "a Fail state's CausePath naming one",
            json!({ "Type": "Fail", "CausePath": "$missing" }),
            "/States/S/CausePath",
        ),
        (
            "a Map state's ItemsPath naming one",
            map_state(json!({ "ItemsPath": "$missing" })),
            "/States/S/ItemsPath",
        ),
        (
            "a payload template's reference field naming one",
            json!({ "Type": "Pass", "Parameters": { "x.$": "$missing" }, "End": true }),
            "/States/S/Parameters/x.$",
        ),
        (
            "a reference field nested inside a payload template",
            json!({
                "Type": "Pass",
                "Parameters": { "payload": { "key.$": "$missing" } },
                "End": true,
            }),
            "/States/S/Parameters/payload/key.$",
        ),
    ];
    for (index, (case, state, location)) in resolving.iter().enumerate() {
        let definition = machine("S", json!({ "S": state }));
        let validated = validate(
            &emulator,
            case,
            &json!({ "definition": definition.to_string() }),
        )
        .await;
        assert_eq!(
            result_of(&validated),
            VALIDATION_RESULT_FAIL,
            "case {case:?}: expected FAIL, got {validated:?}"
        );
        let reported: Vec<(String, String, String)> = diagnostics(&validated)
            .iter()
            .map(|diagnostic| {
                (
                    string(diagnostic, "severity"),
                    string(diagnostic, "code"),
                    string(diagnostic, "location"),
                )
            })
            .collect();
        assert_eq!(
            reported,
            vec![(
                "ERROR".to_string(),
                UNDEFINED_VARIABLE.to_string(),
                (*location).to_string(),
            )],
            "case {case:?}: expected one ERROR {UNDEFINED_VARIABLE} at {location}, got \
             {validated:?}"
        );
        let (status, code, message) = created(
            &emulator,
            &format!("unresolvable-{index}"),
            &definition,
            None,
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidDefinition"),
            "case {case:?}: expected the same definition to be refused at a create, got {status} \
             {code} with {message:?}"
        );
    }

    // The JSONata expression: the read EVALUATES to nothing and fails the state that
    // reads it, so the definition is created and the diagnostic is a warning.
    let evaluated = json!({
        "QueryLanguage": "JSONata",
        "StartAt": "P",
        "States": { "P": { "Type": "Pass", "Output": "{% $missing %}", "End": true } },
    });
    let validated = validate(
        &emulator,
        "a JSONata expression reading a variable nothing assigns",
        &json!({ "definition": evaluated.to_string(), "severity": "WARNING" }),
    )
    .await;
    assert_eq!(
        result_of(&validated),
        VALIDATION_RESULT_OK,
        "case \"a JSONata expression reading a variable nothing assigns\": expected OK, got \
         {validated:?}"
    );
    let reported: Vec<(String, String)> = diagnostics(&validated)
        .iter()
        .map(|diagnostic| (string(diagnostic, "severity"), string(diagnostic, "code")))
        .collect();
    assert_eq!(
        reported,
        vec![(
            "WARNING".to_string(),
            POSSIBLY_UNDEFINED_VARIABLE.to_string()
        )],
        "case \"a JSONata expression reading a variable nothing assigns\": expected one WARNING \
         {POSSIBLY_UNDEFINED_VARIABLE} diagnostic, got {validated:?}"
    );
    let (status, code, message) = created(&emulator, "evaluated", &evaluated, None).await;
    assert_eq!(
        status, 200,
        "case \"the same definition at a create\": expected 200, got {status} {code} with \
         {message:?}"
    );

    // And a variable only ONE path to the read assigns is the WARNING in either
    // language: the read resolves on the runs that took the assigning path and not on
    // the others, so the definition is CREATED. RECORDED behaviour — a definition whose
    // catcher assigns a variable a later state reads is accepted — and an ERROR is for a
    // name NO run reaching the field could have assigned, which the rows above are.
    // `catcher_assign_downstream_scope` is the rule over that whole class.
    let partly = machine(
        "C",
        json!({
            "C": {
                "Type": "Choice",
                "Choices": [{ "Variable": "$.go", "BooleanEquals": true, "Next": "S" }],
                "Default": "J",
            },
            "S": { "Type": "Pass", "Assign": { "maybe.$": "$.n" }, "Next": "J" },
            "J": {
                "Type": "Pass",
                "Parameters": { "x.$": "$maybe" },
                "End": true,
            },
        }),
    );
    let validated = validate(
        &emulator,
        "a reference naming a variable only one path assigns",
        &json!({ "definition": partly.to_string(), "severity": "WARNING" }),
    )
    .await;
    assert_eq!(
        result_of(&validated),
        VALIDATION_RESULT_OK,
        "case \"a reference naming a variable only one path assigns\": expected OK, got \
         {validated:?}"
    );
    let reported: Vec<(String, String)> = diagnostics(&validated)
        .iter()
        .map(|diagnostic| (string(diagnostic, "severity"), string(diagnostic, "code")))
        .collect();
    assert_eq!(
        reported,
        vec![(
            "WARNING".to_string(),
            POSSIBLY_UNDEFINED_VARIABLE.to_string()
        )],
        "case \"a reference naming a variable only one path assigns\": expected one WARNING \
         {POSSIBLY_UNDEFINED_VARIABLE} diagnostic, got {validated:?}"
    );
    let (status, code, message) = created(&emulator, "partly", &partly, None).await;
    assert_eq!(
        status, 200,
        "case \"the same definition at a create\": expected 200, got {status} {code} with \
         {message:?}"
    );

    // While the same shape read by an EXPRESSION is created: the read evaluates to
    // nothing on the path that does not assign it, which is a warning.
    let evaluated_partly = json!({
        "QueryLanguage": "JSONata",
        "StartAt": "C",
        "States": {
            "C": {
                "Type": "Choice",
                "Choices": [{ "Condition": "{% $states.input.go %}", "Next": "S" }],
                "Default": "J",
            },
            "S": { "Type": "Pass", "Assign": { "maybe": 1 }, "Next": "J" },
            "J": { "Type": "Pass", "Output": "{% $maybe %}", "End": true },
        },
    });
    let (status, code, message) = created(&emulator, "partly-evaluated", &evaluated_partly, None)
        .await;
    assert_eq!(
        status, 200,
        "case \"the same shape read by an expression\": expected 200, got {status} {code} with \
         {message:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: the form an embedded expression and a document member take
// ---------------------------------------------------------------------------

/// An embedded JSONata expression is the WHOLE value of the field it is written in, and
/// a member the language declares as a document carries an object or one expression.
///
/// The rows differ along HOW the text fails to be one expression (opened and never
/// closed, closed without being opened, text outside the marks, two expressions in one
/// value) and along the member it is written in (a state's `Output`, a `Fail` state's
/// `Error` and `Cause`, a member of a nested document), and then along the member whose
/// SHAPE is wrong and what it holds instead.
#[tokio::test]
async fn an_embedded_expression_is_the_whole_value_of_its_field() {
    // (case, the state, the location, the text the diagnostic quotes)
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "an expression opened and never closed",
            json!({ "Type": "Pass", "Output": "{% $states.input", "End": true }),
            "/States/S/Output",
            "{% $states.input",
        ),
        (
            "an expression closed without being opened",
            json!({ "Type": "Pass", "Output": "$states.input %}", "End": true }),
            "/States/S/Output",
            "$states.input %}",
        ),
        (
            "text outside the marks",
            json!({ "Type": "Pass", "Output": "value: {% $states.input %}", "End": true }),
            "/States/S/Output",
            "value: {% $states.input %}",
        ),
        (
            // The parse of what the marks DO enclose is a second question, so this one
            // row's refusal may carry a subject about that too: what is asserted is
            // that the malformed value is stated.
            "two expressions in one value",
            json!({
                "Type": "Pass",
                "Output": "{% $states.input %}{% $states.context %}",
                "End": true,
            }),
            "/States/S/Output",
            "{% $states.input %}{% $states.context %}",
        ),
        (
            "a member of a nested document",
            json!({
                "Type": "Pass",
                "Output": { "deep": { "er": "{% $states.input" } },
                "End": true,
            }),
            "/States/S/Output/deep/er",
            "{% $states.input",
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, state, location, text)) in rows.iter().enumerate() {
        let definition = json!({
            "QueryLanguage": "JSONata",
            "StartAt": "S",
            "States": { "S": state },
        });
        let detail = format!(
            "'{text}' is not a JSONata expression: an expression is the whole value of the \
             field, opened with '{{%' and closed with '%}}'"
        );
        let expected = refusal(&subject(SCHEMA_VALIDATION_FAILED, &detail, location));
        let (status, code, message) = created(
            &emulator,
            &format!("malformed-{index}"),
            &definition,
            None,
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidDefinition"),
            "case {case:?}: expected 400 InvalidDefinition, got {status} {code} with {message:?}"
        );
        assert!(
            message.contains(&subject(SCHEMA_VALIDATION_FAILED, &detail, location)),
            "case {case:?}: expected a refusal stating {expected:?}, got {message:?}"
        );
    }

    // A `Fail` state's two sentences are two fields, so each earns its own diagnostic
    // at its own position.
    let both = json!({
        "QueryLanguage": "JSONata",
        "StartAt": "F",
        "States": {
            "F": { "Type": "Fail", "Error": "{% bad", "Cause": "{% worse" },
        },
    });
    let validated = validate(
        &emulator,
        "a Fail state whose Error and Cause are each malformed",
        &json!({ "definition": both.to_string() }),
    )
    .await;
    let located: Vec<String> = diagnostics(&validated)
        .iter()
        .map(|diagnostic| string(diagnostic, "location"))
        .collect();
    assert_eq!(
        located,
        vec!["/States/F/Error".to_string(), "/States/F/Cause".to_string()],
        "case \"a Fail state whose Error and Cause are each malformed\": expected one diagnostic \
         per field, got {validated:?}"
    );

    // And the SHAPE of a member the language declares as a document.
    for (case, state, location, member) in [
        (
            "a document member holding a list",
            json!({
                "Type": "Task",
                "Resource": "arn:aws:states:::aws-sdk:sts:getCallerIdentity",
                "Arguments": [1, 2],
                "End": true,
            }),
            "/States/S/Arguments",
            "Arguments",
        ),
        (
            "a document member holding a number",
            json!({
                "Type": "Map",
                "ItemSelector": 1,
                "ItemProcessor": {
                    "StartAt": "P",
                    "States": { "P": { "Type": "Pass", "End": true } },
                },
                "End": true,
            }),
            "/States/S/ItemSelector",
            "ItemSelector",
        ),
    ] {
        let definition = json!({
            "QueryLanguage": "JSONata",
            "StartAt": "S",
            "States": { "S": state },
        });
        let detail = format!("'{member}' must be a JSON object or a JSONata expression");
        let expected = refusal(&subject(SCHEMA_VALIDATION_FAILED, &detail, location));
        let (status, code, message) = created(
            &emulator,
            &format!("document-{member}"),
            &definition,
            None,
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidDefinition"),
            "case {case:?}: expected 400 InvalidDefinition, got {status} {code} with {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }
}

/// A block of assignments is an object whose every key is a name the language admits
/// for a variable.
///
/// The rows differ along WHERE the block is written (a state's own, a `Catch`
/// catcher's), along what makes the name one the language does not admit (a character
/// it does not carry, a leading digit, the reserved context object, the empty name) and
/// along the shape the block itself holds.
#[tokio::test]
async fn an_assignment_block_names_variables_the_language_admits() {
    // (case, the state, the location, the name the refusal names)
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "a name carrying a character no variable name carries",
            json!({ "Type": "Pass", "Assign": { "my-total": 1 }, "End": true }),
            "/States/S/Assign",
            "my-total",
        ),
        (
            "a name beginning with a digit",
            json!({ "Type": "Pass", "Assign": { "1st": 1 }, "End": true }),
            "/States/S/Assign",
            "1st",
        ),
        (
            "the reserved context object's own name",
            json!({ "Type": "Pass", "Assign": { "states": 1 }, "End": true }),
            "/States/S/Assign",
            "states",
        ),
        (
            "the empty name",
            json!({ "Type": "Pass", "Assign": { "": 1 }, "End": true }),
            "/States/S/Assign",
            "",
        ),
        (
            "a reference field assigning a name the language does not admit",
            json!({ "Type": "Pass", "Assign": { "my total.$": "$.n" }, "End": true }),
            "/States/S/Assign",
            "my total",
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, state, location, name)) in rows.iter().enumerate() {
        let definition = machine("S", json!({ "S": state }));
        let detail = format!(
            "Invalid variable name '{name}': the variable name contains invalid characters."
        );
        let expected = refusal(&subject(SCHEMA_VALIDATION_FAILED, &detail, location));
        let (status, code, message) = created(
            &emulator,
            &format!("variable-name-{index}"),
            &definition,
            None,
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidDefinition"),
            "case {case:?}: expected 400 InvalidDefinition, got {status} {code} with {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }

    // The catcher's own block is judged exactly as the state's is, at its own position.
    let caught = machine(
        "T",
        json!({
            "T": {
                "Type": "Task",
                "Resource": "arn:aws:states:::aws-sdk:sts:getCallerIdentity",
                "Catch": [{
                    "ErrorEquals": ["States.ALL"],
                    "Assign": { "not a name": 1 },
                    "Next": "Done",
                }],
                "End": true,
            },
            "Done": { "Type": "Succeed" },
        }),
    );
    let expected = refusal(&subject(
        SCHEMA_VALIDATION_FAILED,
        "Invalid variable name 'not a name': the variable name contains invalid characters.",
        "/States/T/Catch[0]/Assign",
    ));
    let (status, code, message) = created(&emulator, "caught-assign", &caught, None).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidDefinition"),
        "case \"a catcher's own block\": expected 400 InvalidDefinition, got {status} {code} with \
         {message:?}"
    );
    assert_eq!(
        message, expected,
        "case \"a catcher's own block\": expected the message {expected:?}, got {message:?}"
    );

    // A block that is not an object at all declares no assignment.
    let unshaped = machine(
        "S",
        json!({ "S": { "Type": "Pass", "Assign": [1], "End": true } }),
    );
    let expected = refusal(&subject(
        SCHEMA_VALIDATION_FAILED,
        "'Assign' must be a JSON object of variables",
        "/States/S/Assign",
    ));
    let (status, code, message) = created(&emulator, "unshaped-assign", &unshaped, None).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidDefinition"),
        "case \"a block that is not an object\": expected 400 InvalidDefinition, got {status} \
         {code} with {message:?}"
    );
    assert_eq!(
        message, expected,
        "case \"a block that is not an object\": expected the message {expected:?}, got \
         {message:?}"
    );

    // The complement: the names the language DOES admit are assigned.
    let admitted = machine(
        "S",
        json!({
            "S": {
                "Type": "Pass",
                "Assign": { "total": 1, "_private": 2, "camelCase9": 3, "x.$": "$.n" },
                "End": true,
            },
        }),
    );
    let (status, code, message) = created(&emulator, "variable-names", &admitted, None).await;
    assert_eq!(
        status, 200,
        "case \"the names the language admits\": expected 200, got {status} {code} with \
         {message:?}"
    );
}
