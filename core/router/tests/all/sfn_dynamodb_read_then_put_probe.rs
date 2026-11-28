// SPDX-License-Identifier: CC0-1.0

//! What a Step Functions workflow that READS a DynamoDB item, waits, and WRITES it back
//! does to a change committed while it was waiting — asked of the ASSEMBLED emulator,
//! every service mounted as `aws_emulator_router::build_router` assembles the one the
//! binary serves, so the execution's work is really authorized as its role and really
//! served by DynamoDB's own handlers.
//!
//! RULE: "A `Wait` state delays the state machine from continuing for a specified time",
//! so the states AFTER a `Wait` do their work when that span has run out and not when the
//! execution started: a `Task` state among them makes its service-integration call at
//! that instant, over the data the state carries. A read-then-write workflow therefore
//! OVERWRITES a change committed between its read and its write — the value it writes is
//! the one it read — and the execution reaches a terminal status either way.
//!
//! Three crates have to agree for that to hold (Step Functions runs the workflow, IAM
//! holds the role its calls are made as, DynamoDB serves them), so the suite lives here.
//! The per-shape table — one `Wait`, two in a row, a `Wait` a `Choice` loop comes back
//! round to — is Step Functions' own suite
//! (`services/service-com-amazonaws-sfn-awsstepfunctions/tests/all/
//! dynamodb_read_then_write_workflow.rs`); what this one adds is the same rule on the
//! emulator as it is really assembled.

use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The IAM caller and the role helper `all/sfn_task_target_authorization.rs` owns:
// `tests/all/main.rs` is one crate root, so the one way a suite here creates a Step
// Functions execution role is borrowed rather than copied.
use crate::sfn_task_target_authorization::role;

/// The region every request here is signed for.
const REGION: &str = "us-west-2";

/// How long a poll waits for the condition it is waiting on — a created table becoming
/// `ACTIVE`, an execution leaving `RUNNING`. Generous next to every span the definition
/// declares: each loop stops as soon as its condition holds, so this bounds only a
/// failure.
const PATIENCE: Duration = Duration::from_secs(120);

/// How long a poll waits between looks.
const GLANCE: Duration = Duration::from_millis(50);

/// The table the workflow reads and writes, and the one key it works on.
const TABLE: &str = "ledger";
/// See [`TABLE`].
const KEY: &str = "o-1";

/// The quantity the item is seeded with, and the one a change commits while the workflow
/// is waiting.
const SEEDED: &str = "1";
/// See [`SEEDED`].
const CHANGED: &str = "99";

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One `awsJson1_0` call, as `(status, body)`.
async fn deliver(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request")
    ;
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One Step Functions call that must answer 200.
async fn sfn(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = deliver(router, "states", "AWSStepFunctions", operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation}: expected 200 from Step Functions, got {status} with {parsed:?}"
    );
    parsed
}

/// One DynamoDB call that must answer 200.
async fn ddb(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = deliver(router, "dynamodb", "DynamoDB_20120810", operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation}: expected 200 from DynamoDB, got {status} with {parsed:?}"
    );
    parsed
}

/// The quantity the item holds, or `None` where the table holds no such item.
async fn quantity(router: &MultiServiceRouter) -> Option<String> {
    let read = ddb(
        router,
        "GetItem",
        &json!({ "TableName": TABLE, "Key": { "pk": { "S": KEY } } }),
    )
    .await;
    read["Item"]["qty"]["N"].as_str().map(str::to_string)
}

/// Write `qty` under the one key, through DynamoDB's own `PutItem`.
async fn put(router: &MultiServiceRouter, qty: &str) {
    ddb(
        router,
        "PutItem",
        &json!({
            "TableName": TABLE,
            "Item": { "pk": { "S": KEY }, "qty": { "N": qty } },
        }),
    )
    .await;
}

/// A read-then-write workflow on the assembled emulator overwrites a change committed
/// while it waits.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_then_write_workflow_overwrites_a_change_made_while_it_waits() {
    let mut router = aws_emulator_router::build_router(aws_emulator_router::new_engine());
    router.set_request_authentication(false);

    // The role the execution's own calls are made as, carrying exactly the two item
    // operations the definition asks for.
    let role_arn = role(
        &router,
        "ReadThenWriteRole",
        &json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": ["dynamodb:GetItem", "dynamodb:PutItem"],
                "Resource": "*"
            }]
        }),
    )
    .await;

    ddb(
        &router,
        "CreateTable",
        &json!({
            "TableName": TABLE,
            "KeySchema": [{ "AttributeName": "pk", "KeyType": "HASH" }],
            "AttributeDefinitions": [{ "AttributeName": "pk", "AttributeType": "S" }],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    // "You can perform read and write operations only on an ACTIVE table", so the create
    // is followed by a poll until it is one.
    let began = Instant::now();
    loop {
        let described = ddb(&router, "DescribeTable", &json!({ "TableName": TABLE })).await;
        if described["Table"]["TableStatus"].as_str() == Some("ACTIVE") {
            break;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "expected {TABLE} to report ACTIVE within {PATIENCE:?}, got {described:?}"
        );
        ::tokio::time::sleep(GLANCE).await;
    }
    put(&router, SEEDED).await;

    let definition = json!({
        "StartAt": "Read",
        "States": {
            "Read": {
                "Type": "Task",
                "Resource": "arn:aws:states:::dynamodb:getItem",
                "Parameters": { "TableName": TABLE, "Key": { "pk": { "S": KEY } } },
                "ResultPath": "$.read",
                "Next": "Pause",
            },
            "Pause": { "Type": "Wait", "Seconds": 2, "Next": "WriteCopy" },
            "WriteCopy": {
                "Type": "Task",
                "Resource": "arn:aws:states:::dynamodb:putItem",
                "Parameters": {
                    "TableName": TABLE,
                    "Item": { "pk": { "S": KEY }, "qty": { "N.$": "$.read.Item.qty.N" } },
                },
                "End": true,
            },
        },
    });
    let created = sfn(
        &router,
        "CreateStateMachine",
        &json!({
            "name": "read-then-write",
            "definition": definition.to_string(),
            "roleArn": role_arn,
        }),
    )
    .await;
    let machine = created["stateMachineArn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a state machine ARN, got {created}"))
        .to_string();
    let started = sfn(
        &router,
        "StartExecution",
        &json!({ "stateMachineArn": machine, "name": "only", "input": "{}" }),
    )
    .await;
    let execution = started["executionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an execution ARN, got {started}"))
        .to_string();

    // The change committed while the workflow is waiting: its read is behind the wait, so
    // the value it will write back is the seeded one.
    put(&router, CHANGED).await;

    let began = Instant::now();
    let described = loop {
        let described = sfn(
            &router,
            "DescribeExecution",
            &json!({ "executionArn": execution }),
        )
        .await;
        if described.get("status").and_then(Value::as_str) != Some("RUNNING") {
            break described;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "expected a terminal status within {PATIENCE:?}, got {described:?}"
        );
        ::tokio::time::sleep(GLANCE).await;
    };
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "expected the workflow to succeed, got {described:?}"
    );
    assert_eq!(
        quantity(&router).await.as_deref(),
        Some(SEEDED),
        "expected the write made after the wait to overwrite the change with what the \
         workflow read"
    );
}
