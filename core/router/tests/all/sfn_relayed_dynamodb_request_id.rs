// SPDX-License-Identifier: CC0-1.0

//! The id a DynamoDB answer names its request by, as a Step Functions `Task` state
//! RELAYS it — driven across the two crates the question spans (DynamoDB answers the
//! call, Step Functions reports what came back), mounted on ONE engine exactly as
//! `core/router/src/lib.rs` mounts them.
//!
//! RULE: an AWS request id is opaque to a client, so each service spells its own, and
//! DynamoDB's is one unbroken run of uppercase letters and digits — never the hyphenated
//! 8-4-4-4-12 UUID most other AWS services report — which is the form every sample
//! response in the DynamoDB API reference shows. A layer that relays a DynamoDB answer
//! carries that id through verbatim rather than minting one of its own, so a DynamoDB
//! request id has that form everywhere a client can observe it: in the request-id token of
//! the SDK exception sentence a failed `Task` state reports as its `cause`, and in the
//! `SdkResponseMetadata.RequestId` of the result a succeeded one reports. Each call is
//! named by its OWN id, so no two of them repeat one.
//!
//! Two crates have to agree for that to hold, so the suite lives here rather than in
//! either of them. The OTHER place the same id is observable — the `x-amzn-RequestId` of
//! DynamoDB's own answer, which is what this relay reads it out of — is pinned by
//! `all/per_service_response_request_id_form.rs` for every service at once, so this suite
//! asks only what that one cannot: what the RELAY reports.
//!
//! The rows differ along what the rule generalises over: which DynamoDB operation the
//! task called (`PutItem`, `UpdateItem`, `DeleteItem`, `GetItem`), which error class the
//! answer came back under (a condition that failed, a table the service does not hold),
//! which integration family the `Resource` is written in (the optimized
//! `arn:aws:states:::dynamodb:<op>` and the AWS SDK `arn:aws:states:::aws-sdk:dynamodb:<op>`
//! form), and whether the call SUCCEEDED or was refused — a success reports its id in the
//! result's own metadata rather than in a cause.
//!
//! Everything but the id itself is fixed text written out here, and the id is read out of
//! the answer and checked against the FORM alone: it identifies one call, so no particular
//! value is part of the sentence.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for.
const REGION: &str = "us-west-2";

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// How long a poll waits for the condition it is waiting on — a created table becoming
/// `ACTIVE`, an execution leaving `RUNNING`. Generous: the loop stops as soon as the
/// condition holds, so this bounds only a failure.
const PATIENCE: Duration = Duration::from_secs(60);

/// How long a poll waits between looks.
const GLANCE: Duration = Duration::from_millis(25);

/// The inclusive length band a DynamoDB request id is spelled at, and the alphabet it is
/// written in — read off the `x-amzn-RequestId` of the sample responses in the DynamoDB API
/// reference, and stated as the same band `all/per_service_response_request_id_form.rs`
/// states for this form, so the assertion pins the FORM rather than mirroring the width
/// constant the implementation happens to draw at.
const REQUEST_ID_WIDTHS: std::ops::RangeInclusive<usize> = 40..=64;

/// The marker that separates the invoked service's own sentence from the metadata its SDK
/// client appends to it.
const METADATA_MARKER: &str = " (Service: ";

/// The table every row below calls on, and the one key it is seeded with.
const TABLE: &str = "ledger";
/// See [`TABLE`].
const SEEDED_KEY: &str = "already-there";
/// A table name no table answers to, which is what the not-found rows call on.
const ABSENT_TABLE: &str = "no-such-ledger";

/// Whether `id` is spelled the way DynamoDB spells a request id: a [`REQUEST_ID_WIDTHS`]
/// run of characters, every one of them an uppercase letter or a digit. The hyphenated UUID
/// other AWS services report fails on both counts.
fn names_a_dynamodb_request(id: &str) -> bool {
    REQUEST_ID_WIDTHS.contains(&id.chars().count())
        && id
            .chars()
            .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit())
}

/// The two services this question spans, over ONE shared engine. Request authentication is
/// off, as in every other suite here; the in-process call Step Functions makes signs itself
/// either way (the credential scope is what routes it).
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One delivery of one `awsJson1_0` call: its status and its body.
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

/// One Step Functions call (`AWSStepFunctions.<Operation>`), requiring a 200.
async fn sfn(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = deliver(router, "states", "AWSStepFunctions", operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation}: expected 200 from Step Functions, got {status} with {parsed:?}"
    );
    parsed
}

/// One DynamoDB call that must answer 200, as its body: the seeding and the read-back every
/// row is set up and checked through.
async fn dynamodb_ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = deliver(router, "dynamodb", "DynamoDB_20120810", operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation}: expected 200 from DynamoDB, got {status} with {parsed:?}"
    );
    parsed
}

/// The text a member of a response carries.
fn string(body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a {name} in {body}"))
        .to_string()
}

/// The table every row calls on, seeded with the one item the condition rows are refused
/// over. "You can perform read and write operations only on an ACTIVE table", so the
/// create is followed by a poll until it is one.
async fn seeded_table(router: &MultiServiceRouter) {
    dynamodb_ok(
        router,
        "CreateTable",
        &json!({
            "TableName": TABLE,
            "KeySchema": [{ "AttributeName": "pk", "KeyType": "HASH" }],
            "AttributeDefinitions": [{ "AttributeName": "pk", "AttributeType": "S" }],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    let began = Instant::now();
    loop {
        let described =
            dynamodb_ok(router, "DescribeTable", &json!({ "TableName": TABLE })).await;
        if described["Table"]["TableStatus"].as_str() == Some("ACTIVE") {
            break;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case the table under test: expected {TABLE} to report ACTIVE within {PATIENCE:?}, \
             got {described:?}"
        );
        ::tokio::time::sleep(GLANCE).await;
    }
    dynamodb_ok(
        router,
        "PutItem",
        &json!({ "TableName": TABLE, "Item": { "pk": { "S": SEEDED_KEY } } }),
    )
    .await;
}

/// A state machine whose single `Task` state makes `resource`'s call with `parameters`,
/// by its ARN. No `Catch`: a refused call is what the execution's own record has to
/// report.
async fn machine(
    router: &MultiServiceRouter,
    name: &str,
    resource: &str,
    parameters: &Value,
) -> String {
    let definition = json!({
        "StartAt": "Call",
        "States": {
            "Call": {
                "Type": "Task",
                "Resource": resource,
                "Parameters": parameters,
                "End": true,
            }
        }
    });
    let created = sfn(
        router,
        "CreateStateMachine",
        &json!({
            "name": name,
            "definition": definition.to_string(),
            "roleArn": format!("arn:aws:iam::{ACCOUNT}:role/StepFunctionsRole"),
        }),
    )
    .await;
    string(&created, "stateMachineArn")
}

/// Start one execution of `machine_arn` and report the `DescribeExecution` that settled
/// it, polled until it has left `RUNNING`.
async fn ran(router: &MultiServiceRouter, machine_arn: &str, case: &str) -> Value {
    let started = sfn(
        router,
        "StartExecution",
        &json!({ "stateMachineArn": machine_arn, "name": "only" }),
    )
    .await;
    let execution_arn = string(&started, "executionArn");
    let began = Instant::now();
    loop {
        let described = sfn(
            router,
            "DescribeExecution",
            &json!({ "executionArn": execution_arn }),
        )
        .await;
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

/// What a row expects of the invoked service's own sentence, ahead of the metadata its
/// SDK client appends: the whole sentence where the service states a fixed one, and its
/// opening where the sentence goes on to name what the request asked for.
enum Sentence {
    Whole(&'static str),
    Opening(&'static str),
}

/// One row: the case, the `Resource` the `Task` names, the parameters it calls with, the
/// error name the task must report, the code the answer came back under and the sentence
/// that code carries.
struct Row {
    case: &'static str,
    resource: &'static str,
    parameters: Value,
    error: &'static str,
    code: &'static str,
    sentence: Sentence,
}

/// Every row's class is refused with HTTP 400:
/// `smithy_query(query="shape", shape="com.amazonaws.dynamodb#ConditionalCheckFailedException,\
/// com.amazonaws.dynamodb#ResourceNotFoundException", model_path="services/\
/// service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/\
/// com-amazonaws-dynamodb-dynamodb-20120810.json")` reports `httpError` 400 for both.
const REFUSAL_STATUS: u16 = 400;

/// The id the relayed sentence names the refused request by, with the WHOLE sentence
/// pinned around it.
///
/// Everything but the id is fixed text this suite writes out — the service's own sentence,
/// the client the call was made with, the status, the error code — so the id is the one
/// part read out of the answer, and it is judged by its FORM alone.
fn relayed_request_id(case: &str, cause: &str, sentence: &Sentence, code: &str) -> String {
    let (stated, metadata) = cause.split_once(METADATA_MARKER).unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected the invoked service's sentence followed by \
             {METADATA_MARKER:?}, got {cause:?}"
        )
    });
    match sentence {
        Sentence::Whole(text) => assert_eq!(
            stated, *text,
            "case {case:?}: expected the sentence {text:?}, got {stated:?}"
        ),
        Sentence::Opening(text) => assert!(
            stated.starts_with(text),
            "case {case:?}: expected a sentence opening {text:?}, got {stated:?}"
        ),
    }
    const MARKER: &str = "Request ID: ";
    let at = metadata.find(MARKER).unwrap_or_else(|| {
        panic!("case {case:?}: expected the relayed metadata to name the request, got {cause:?}")
    });
    let rest = &metadata[at + MARKER.len()..];
    let id = rest[..rest.find(';').unwrap_or(rest.len())].to_string();
    assert_eq!(
        metadata,
        format!(
            "AmazonDynamoDBv2; Status Code: {REFUSAL_STATUS}; Error Code: {code}; \
             Request ID: {id}; Proxy: null)"
        ),
        "case {case:?}: expected the metadata DynamoDB's own SDK client appends, got {cause:?}"
    );
    assert!(
        names_a_dynamodb_request(&id),
        "case {case:?}: expected the relayed request id in DynamoDB's own form — an unbroken \
         run of uppercase letters and digits, {REQUEST_ID_WIDTHS:?} of them — got {id:?}"
    );
    id
}

/// A DynamoDB failure a `Task` state relays names the refused request by a DynamoDB
/// request id, and a DynamoDB success relays one too — each call by its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relayed_dynamodb_answer_names_its_request_the_way_dynamodb_does() {
    let router = emulator();
    seeded_table(&router).await;

    let rows = [
        Row {
            case: "the optimized putItem whose condition fails",
            resource: "arn:aws:states:::dynamodb:putItem",
            parameters: json!({
                "TableName": TABLE,
                "Item": { "pk": { "S": SEEDED_KEY }, "note": { "S": "second" } },
                "ConditionExpression": "attribute_not_exists(pk)",
            }),
            error: "DynamoDB.ConditionalCheckFailedException",
            code: "ConditionalCheckFailedException",
            sentence: Sentence::Whole("The conditional request failed"),
        },
        Row {
            // The other family makes the same call on the same table, so the relayed id
            // has to have the same form through it.
            case: "the same putItem through the AWS SDK integration",
            resource: "arn:aws:states:::aws-sdk:dynamodb:putItem",
            parameters: json!({
                "TableName": TABLE,
                "Item": { "pk": { "S": SEEDED_KEY }, "note": { "S": "second" } },
                "ConditionExpression": "attribute_not_exists(pk)",
            }),
            error: "DynamoDB.ConditionalCheckFailedException",
            code: "ConditionalCheckFailedException",
            sentence: Sentence::Whole("The conditional request failed"),
        },
        Row {
            case: "the optimized updateItem whose condition fails",
            resource: "arn:aws:states:::dynamodb:updateItem",
            parameters: json!({
                "TableName": TABLE,
                "Key": { "pk": { "S": SEEDED_KEY } },
                "UpdateExpression": "SET #note = :note",
                "ExpressionAttributeNames": { "#note": "note" },
                "ExpressionAttributeValues": { ":note": { "S": "updated" } },
                "ConditionExpression": "attribute_not_exists(pk)",
            }),
            error: "DynamoDB.ConditionalCheckFailedException",
            code: "ConditionalCheckFailedException",
            sentence: Sentence::Whole("The conditional request failed"),
        },
        Row {
            case: "the optimized deleteItem whose condition fails",
            resource: "arn:aws:states:::dynamodb:deleteItem",
            parameters: json!({
                "TableName": TABLE,
                "Key": { "pk": { "S": "never-written" } },
                "ConditionExpression": "attribute_exists(pk)",
            }),
            error: "DynamoDB.ConditionalCheckFailedException",
            code: "ConditionalCheckFailedException",
            sentence: Sentence::Whole("The conditional request failed"),
        },
        Row {
            // A second error class, so the form is a property of the SERVICE's answer
            // rather than of one exception.
            case: "the optimized getItem on a table the service does not hold",
            resource: "arn:aws:states:::dynamodb:getItem",
            parameters: json!({
                "TableName": ABSENT_TABLE,
                "Key": { "pk": { "S": SEEDED_KEY } },
            }),
            error: "DynamoDB.ResourceNotFoundException",
            code: "ResourceNotFoundException",
            sentence: Sentence::Opening("Requested resource not found"),
        },
        Row {
            case: "the same getItem through the AWS SDK integration",
            resource: "arn:aws:states:::aws-sdk:dynamodb:getItem",
            parameters: json!({
                "TableName": ABSENT_TABLE,
                "Key": { "pk": { "S": SEEDED_KEY } },
            }),
            error: "DynamoDB.ResourceNotFoundException",
            code: "ResourceNotFoundException",
            sentence: Sentence::Opening("Requested resource not found"),
        },
    ];

    // Every id any of these calls was named by: each call is its own request, so no two
    // of them may repeat one.
    let mut named: BTreeSet<String> = BTreeSet::new();

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let machine_arn = machine(
            &router,
            &format!("relayed-{index}"),
            row.resource,
            &row.parameters,
        )
        .await;
        let described = ran(&router, &machine_arn, case).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected FAILED, got {described:?}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some(row.error),
            "case {case:?}: expected the error {:?}, got {described:?}",
            row.error
        );
        let cause = string(&described, "cause");
        let id = relayed_request_id(case, &cause, &row.sentence, row.code);
        assert!(
            named.insert(id.clone()),
            "case {case:?}: every call is named by its own request id, and {id} was already \
             relayed"
        );
    }

    // A call that SUCCEEDS relays its id too, in the response metadata of the result the
    // task reports rather than in a cause — and the write it made is readable afterwards,
    // which is what says the call was really served.
    let case = "the optimized putItem that succeeds";
    let machine_arn = machine(
        &router,
        "relayed-success",
        "arn:aws:states:::dynamodb:putItem",
        &json!({
            "TableName": TABLE,
            "Item": { "pk": { "S": "written-by-the-task" } },
            "ConditionExpression": "attribute_not_exists(pk)",
        }),
    )
    .await;
    let described = ran(&router, &machine_arn, case).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case {case:?}: expected SUCCEEDED, got {described:?}"
    );
    let output: Value = serde_json::from_str(&string(&described, "output"))
        .unwrap_or_else(|why| panic!("case {case:?}: expected a JSON result, got {why}"));
    let id = output["SdkResponseMetadata"]["RequestId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the SDK response metadata to name the request it \
                    answered, got {output}")
        })
        .to_string();
    assert!(
        names_a_dynamodb_request(&id),
        "case {case:?}: expected the relayed request id in DynamoDB's own form — an unbroken \
         run of uppercase letters and digits, {REQUEST_ID_WIDTHS:?} of them — got {id:?}"
    );
    assert!(
        named.insert(id.clone()),
        "case {case:?}: every call is named by its own request id, and {id} was already relayed"
    );
    let read = dynamodb_ok(
        &router,
        "GetItem",
        &json!({ "TableName": TABLE, "Key": { "pk": { "S": "written-by-the-task" } } }),
    )
    .await;
    assert_eq!(
        read.get("Item"),
        Some(&json!({ "pk": { "S": "written-by-the-task" } })),
        "case {case:?}: expected the item the task wrote to be readable, got {read:?}"
    );
}
