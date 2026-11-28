// SPDX-License-Identifier: CC0-1.0

//! What the ASSEMBLED emulator — every service mounted, as
//! `aws_emulator_router::build_router` assembles the one the binary serves — makes of a
//! Step Functions definition whose `Task` state builds a request for ANOTHER service's
//! API: the members that API cannot be called without are judged where the definition is
//! submitted.
//!
//! RULE: a `Task` state's service integration performs one API operation, and the
//! document the state's payload template builds IS that operation's request, so a
//! template that omits a member the invoked operation's model marks `required` describes
//! a request that service could never serve — whatever an execution's data turns out to
//! be. `CreateStateMachine` refuses such a definition with `InvalidDefinition` (400) and
//! creates no state machine, and one carrying every required member is created.
//!
//! The question spans two crates — Step Functions judges the definition, and what it
//! judges it against is DynamoDB's own model — so the suite lives here rather than in
//! either of them, and it is asked of the emulator as it is really assembled. The
//! per-operation table, over both integration families and both query languages, is Step
//! Functions' own suite
//! (`services/service-com-amazonaws-sfn-awsstepfunctions/tests/all/
//! dynamodb_task_definition_rules.rs`); what this one adds is that the ASSEMBLED emulator
//! applies it.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for.
const REGION: &str = "us-west-2";

/// A SigV4 `Authorization` header whose credential scope names Step Functions, which is
/// what routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/states/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One Step Functions call, as `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("states.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSStepFunctions.{operation}"))
        .header("authorization", authorization())
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A one-state definition whose `Task` state calls `resource` with `parameters`.
fn definition(resource: &str, parameters: Value) -> String {
    json!({
        "StartAt": "Write",
        "States": {
            "Write": {
                "Type": "Task",
                "Resource": resource,
                "Parameters": parameters,
                "End": true,
            },
        },
    })
    .to_string()
}

/// The assembled emulator judges a `dynamodb:putItem` Task against DynamoDB's own
/// required members: a request without the item it writes is refused where the definition
/// is submitted, and one carrying it is created.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_assembled_emulator_judges_a_task_against_its_apis_required_members() {
    let mut router = aws_emulator_router::build_router(aws_emulator_router::new_engine());
    router.set_request_authentication(false);
    let role_arn = format!(
        "arn:aws:iam::{}:role/StepFunctionsRole",
        engine::EMULATOR_ACCOUNT_ID
    );

    let case = "a putItem Task whose Parameters omit the item it writes";
    let (status, answered) = call(
        &router,
        "CreateStateMachine",
        &json!({
            "name": "missing-item",
            "definition": definition(
                "arn:aws:states:::dynamodb:putItem",
                json!({ "TableName": "orders" }),
            ),
            "roleArn": role_arn,
        }),
    )
    .await;
    assert_eq!(
        (status, answered["__type"].as_str()),
        (400, Some("InvalidDefinition")),
        "case {case:?}: expected 400 InvalidDefinition, got {status} with {answered:?}"
    );
    let message = answered["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("The field 'Item' is required but was missing"),
        "case {case:?}: expected the refusal to name the member the request owes, got \
         {message:?}"
    );
    // A refused definition creates nothing.
    let (status, answered) = call(
        &router,
        "DescribeStateMachine",
        &json!({
            "stateMachineArn": format!(
                "arn:aws:states:{REGION}:{}:stateMachine:missing-item",
                engine::EMULATOR_ACCOUNT_ID
            ),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the refused definition to have created nothing, got {status} \
         with {answered:?}"
    );

    let case = "the same Task carrying every member the operation requires";
    let (status, answered) = call(
        &router,
        "CreateStateMachine",
        &json!({
            "name": "with-item",
            "definition": definition(
                "arn:aws:states:::dynamodb:putItem",
                json!({ "TableName": "orders", "Item": { "pk": { "S": "o-1" } } }),
            ),
            "roleArn": role_arn,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} with {answered:?}"
    );
}
