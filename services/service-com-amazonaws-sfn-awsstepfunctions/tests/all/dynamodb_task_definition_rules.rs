// SPDX-License-Identifier: CC0-1.0

//! What a definition that builds a service-integration request is judged on where it is
//! SUBMITTED, over this service's own wire protocol through the production router: the
//! members the invoked API cannot be called without, and where a reference field whose
//! value is not an expression is reported.
//!
//! The rules under test, each over a table whose rows differ along what the rule
//! generalises over rather than along the one definition that exposed it:
//!
//! 1. a `Task` state whose `Resource` names a service integration must carry, in the
//!    payload template it builds its request in, every member the invoked operation's
//!    model marks `required` — a request without one is one that service could never
//!    serve, whatever an execution's data turns out to be, so the definition is refused
//!    `InvalidDefinition` (400), naming each member it owes, and no state machine is
//!    created;
//! 2. a member whose name ends in `.$` holds a JSONPath or an intrinsic invocation, and
//!    a definition whose does not is refused with the FIELD named in the sentence and the
//!    location at the template node whose schema failed — whatever depth inside that
//!    template the member sits at.
//!
//! The required members are each invoked model's own
//! (`smithy_query(query="operation",operation="PutItem,GetItem,UpdateItem,DeleteItem",
//! model_path="services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/
//! com-amazonaws-dynamodb-dynamodb-20120810.json")`: `TableName` with `Item` for the
//! write of an item and `TableName` with `Key` for the three that address one by its key),
//! and the rows cover operations the recorded case did not name.
//!
//! The harness (`Emulator`, `error_code`, `string`, `ROLE_ARN`) is the one
//! `state_machine_lifecycle` builds; `tests/all/main.rs` picks this file up, so it is
//! reached through `crate::<suite>`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::INVALID_STATE_MACHINE_DEFINITION;

use crate::state_machine_lifecycle::{error_code, string, Emulator, ROLE_ARN};

/// The code every defect of the language's own schema is reported under.
const SCHEMA_VALIDATION_FAILED: &str = "SCHEMA_VALIDATION_FAILED";

/// What `CreateStateMachine` answers for `definition`: the status, the error code and the
/// message.
async fn created(
    emulator: &Emulator,
    name: &str,
    definition: &Value,
) -> (u16, String, String) {
    let (status, answered) = emulator
        .call(
            "CreateStateMachine",
            &json!({
                "name": name,
                "definition": definition.to_string(),
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    let message = match status {
        200 => String::new(),
        _ => string(&answered, "message"),
    };
    (status, error_code(&answered), message)
}

/// The refusal message `CreateStateMachine` states for a definition carrying the one
/// problem `subject` states.
fn refusal(subject: &str) -> String {
    format!("{INVALID_STATE_MACHINE_DEFINITION}: '{subject}'")
}

/// One subject of such a refusal: the code, the detail and the place it was found.
fn subject(code: &str, detail: &str, location: &str) -> String {
    format!("{code}: {detail} at {location}")
}

/// A one-state definition whose `Task` state `Work` names `resource` and declares
/// `template` under `member`.
fn task(resource: &str, member: &str, template: Value) -> Value {
    json!({
        "StartAt": "Work",
        "States": {
            "Work": {
                "Type": "Task",
                "Resource": resource,
                member: template,
                "End": true,
            },
        },
    })
}

/// Whether a state machine of `name` is there to be described, which is what says a
/// refused create created nothing.
async fn is_created(emulator: &Emulator, name: &str) -> bool {
    let arn = format!(
        "arn:aws:states:us-west-2:{}:stateMachine:{name}",
        engine::EMULATOR_ACCOUNT_ID
    );
    let (status, _) = emulator
        .call("DescribeStateMachine", &json!({ "stateMachineArn": arn }))
        .await;
    status == 200
}

// ---------------------------------------------------------------------------
// Rule 1: the members the invoked API cannot be called without
// ---------------------------------------------------------------------------

/// One row: the case, the `Resource`, the member the request is built in, the template,
/// and the members the refusal must name, in the order the invoked model declares them.
struct Missing {
    case: &'static str,
    resource: &'static str,
    member: &'static str,
    template: Value,
    named: &'static [&'static str],
}

/// A `Task` whose payload template omits a member the invoked operation requires is
/// refused where the definition is submitted, and no state machine is created.
///
/// The rows differ along what the rule generalises over: WHICH operation the integration
/// names (the write of an item, the two that address one by its key, the update), WHICH
/// member is missing (the item written, the key addressed, the table they act on, both of
/// an operation's members at once), the integration FAMILY the `Resource` is written in
/// (the optimized form and the AWS SDK one, which name the same operation), the
/// integration PATTERN after the action (which says what finishes the task rather than
/// what its request carries), and the query language the request is built in (JSONPath's
/// `Parameters`, JSONata's `Arguments`).
#[tokio::test]
async fn a_task_without_a_member_its_api_requires_is_refused_at_create() {
    let rows = [
        Missing {
            case: "a putItem without the item it writes",
            resource: "arn:aws:states:::dynamodb:putItem",
            member: "Parameters",
            template: json!({ "TableName": "orders" }),
            named: &["Item"],
        },
        Missing {
            case: "a getItem without the key it addresses",
            resource: "arn:aws:states:::dynamodb:getItem",
            member: "Parameters",
            template: json!({ "TableName": "orders" }),
            named: &["Key"],
        },
        Missing {
            case: "a deleteItem without the key it addresses",
            resource: "arn:aws:states:::dynamodb:deleteItem",
            member: "Parameters",
            template: json!({ "TableName": "orders" }),
            named: &["Key"],
        },
        Missing {
            case: "an updateItem without the table it acts on",
            resource: "arn:aws:states:::dynamodb:updateItem",
            member: "Parameters",
            template: json!({ "Key": { "pk": { "S": "o-1" } } }),
            named: &["TableName"],
        },
        Missing {
            case: "a putItem carrying neither of its required members",
            resource: "arn:aws:states:::dynamodb:putItem",
            member: "Parameters",
            template: json!({ "ReturnValues": "ALL_OLD" }),
            named: &["TableName", "Item"],
        },
        Missing {
            // The AWS SDK family names the same operation, so its request owes the same
            // members.
            case: "the same putItem through the AWS SDK integration family",
            resource: "arn:aws:states:::aws-sdk:dynamodb:putItem",
            member: "Parameters",
            template: json!({ "TableName": "orders" }),
            named: &["Item"],
        },
        Missing {
            // The pattern after the action says what FINISHES the task; the call it makes
            // is the same one.
            case: "a putItem asking for the callback pattern",
            resource: "arn:aws:states:::dynamodb:putItem.waitForTaskToken",
            member: "Parameters",
            template: json!({ "TableName": "orders" }),
            named: &["Item"],
        },
        Missing {
            // The other query language builds the same request in its own member.
            case: "a JSONata putItem whose Arguments omit the item",
            resource: "arn:aws:states:::dynamodb:putItem",
            member: "Arguments",
            template: json!({ "TableName": "orders" }),
            named: &["Item"],
        },
    ];

    let emulator = Emulator::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let name = format!("missing-{index}");
        let mut definition = task(row.resource, row.member, row.template.clone());
        if row.member == "Arguments" {
            definition["QueryLanguage"] = json!("JSONata");
        }
        let (status, code, message) = created(&emulator, &name, &definition).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidDefinition"),
            "case {case:?}: expected 400 InvalidDefinition, got {status} {code} with {message:?}"
        );
        let expected = refusal(
            &row.named
                .iter()
                .map(|field| {
                    subject(
                        SCHEMA_VALIDATION_FAILED,
                        &format!("The field '{field}' is required but was missing"),
                        &format!("/States/Work/{}", row.member),
                    )
                })
                .collect::<Vec<String>>()
                .join(", "),
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        assert!(
            !is_created(&emulator, &name).await,
            "case {case:?}: a refused definition creates no state machine"
        );
    }
}

/// What such a template MAY be: every required member carried, a member written for
/// EVALUATION (`"<name>.$"`, which carries it as much as a literal does), a `Task` that
/// declares no template at all (whose request is its own effective input, which no
/// create-time check can judge), and an integration whose invoked operation this service
/// states no required member for.
#[tokio::test]
async fn a_task_carrying_what_its_api_requires_is_created() {
    let admitted: Vec<(&str, Value)> = vec![
        (
            "every required member carried as a literal",
            task(
                "arn:aws:states:::dynamodb:putItem",
                "Parameters",
                json!({ "TableName": "orders", "Item": { "pk": { "S": "o-1" } } }),
            ),
        ),
        (
            "the required members read from the state's data",
            task(
                "arn:aws:states:::dynamodb:getItem",
                "Parameters",
                json!({ "TableName.$": "$.table", "Key.$": "$.key" }),
            ),
        ),
        (
            "one member read through an intrinsic invocation",
            task(
                "arn:aws:states:::dynamodb:deleteItem",
                "Parameters",
                json!({
                    "TableName.$": "States.Format('{}', $.table)",
                    "Key": { "pk": { "S": "o-1" } },
                }),
            ),
        ),
        (
            "a Task that declares no payload template at all",
            json!({
                "StartAt": "Work",
                "States": {
                    "Work": {
                        "Type": "Task",
                        "Resource": "arn:aws:states:::dynamodb:putItem",
                        "End": true,
                    },
                },
            }),
        ),
        (
            "an integration no required member is stated for",
            task(
                "arn:aws:states:::sns:publish",
                "Parameters",
                json!({ "Message": "hello" }),
            ),
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, definition)) in admitted.iter().enumerate() {
        let (status, code, message) =
            created(&emulator, &format!("admitted-{index}"), definition).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {code} with {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: where a reference field that is no expression is reported
// ---------------------------------------------------------------------------

/// A `.$` member holding something the language cannot evaluate is refused with the
/// field named in the sentence and the location at the template node that carries it.
///
/// The rows differ along the DEPTH the member sits at inside the template (a member of
/// the template itself, one two objects down, one inside an array), along the template it
/// is written in (a `Task` state's request, a `Map` state's item projection, a result
/// selector, an assignment block) and along what the member holds (a plain string, a
/// number) — the reported place is the template node in every one of them.
#[tokio::test]
async fn a_reference_field_is_reported_at_the_template_node_that_carries_it() {
    // (case, the definition, the field named, the location reported)
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "a member of the request itself",
            task(
                "arn:aws:states:::dynamodb:putItem",
                "Parameters",
                json!({ "TableName.$": "orders", "Item": { "pk": { "S": "o-1" } } }),
            ),
            "TableName.$",
            "/States/Work/Parameters",
        ),
        (
            // The recorded shape: an attribute value three objects down.
            "a member three objects down inside the request",
            task(
                "arn:aws:states:::dynamodb:putItem",
                "Parameters",
                json!({
                    "TableName": "orders",
                    "Item": { "pk": { "S": "o-1" }, "qty": { "N.$": "Item.qty.N" } },
                }),
            ),
            "N.$",
            "/States/Work/Parameters",
        ),
        (
            "a member inside an array of the request",
            task(
                "arn:aws:states:::dynamodb:putItem",
                "Parameters",
                json!({
                    "TableName": "orders",
                    "Item": { "pk": { "S": "o-1" }, "tags": { "L": [{ "S.$": 7 }] } },
                }),
            ),
            "S.$",
            "/States/Work/Parameters",
        ),
        (
            "a member of a result selector",
            json!({
                "StartAt": "Work",
                "States": {
                    "Work": {
                        "Type": "Task",
                        "Resource": "arn:aws:states:::dynamodb:getItem",
                        "Parameters": { "TableName": "orders", "Key": { "pk": { "S": "o-1" } } },
                        "ResultSelector": { "found": { "qty.$": "plain" } },
                        "End": true,
                    },
                },
            }),
            "qty.$",
            "/States/Work/ResultSelector",
        ),
        (
            "a member of a Map state's item projection",
            json!({
                "StartAt": "Each",
                "States": {
                    "Each": {
                        "Type": "Map",
                        "ItemSelector": { "wrapped": { "item.$": 1 } },
                        "ItemProcessor": {
                            "StartAt": "P",
                            "States": { "P": { "Type": "Pass", "End": true } },
                        },
                        "End": true,
                    },
                },
            }),
            "item.$",
            "/States/Each/ItemSelector",
        ),
        (
            "a member of an assignment block",
            json!({
                "StartAt": "Set",
                "States": {
                    "Set": {
                        "Type": "Pass",
                        "Assign": { "totals": { "qty.$": "total" } },
                        "End": true,
                    },
                },
            }),
            "qty.$",
            "/States/Set/Assign",
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, definition, field, location)) in rows.iter().enumerate() {
        let (status, code, message) =
            created(&emulator, &format!("reference-{index}"), definition).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidDefinition"),
            "case {case:?}: expected 400 InvalidDefinition, got {status} {code} with {message:?}"
        );
        let expected = refusal(&subject(
            SCHEMA_VALIDATION_FAILED,
            &format!(
                "The value for the field '{field}' must be a valid JSONPath or a valid \
                 intrinsic function call"
            ),
            location,
        ));
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }
}
