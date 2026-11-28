// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a `Task` state's service integration, driven over both
//! services' own wire protocols through the production `MultiServiceRouter` with
//! Step Functions and DynamoDB mounted on ONE engine — the shape
//! `core/router/src/lib.rs` mounts them in — so what is asserted is what a signed
//! AWS SDK request observes, including the FAR-END effect of the call the task
//! made.
//!
//! The rule under test: a `Task` state whose `Resource` names a service integration
//! performs that service's own API operation, with the document the state built (its
//! `Parameters` payload template over its effective input, or that input itself) as
//! the operation's REQUEST and the operation's response document as the state's
//! RESULT — so the execution's far-end effect is really there afterwards, read back
//! through that service's own API; and a call the service REFUSED is the task's own
//! failure, named `<the service's SDK id>.<the error code it answered>`, which the
//! state's `Retry` and `Catch` match on.
//!
//! The recorded case this suite's issue was filed about — an event-driven execution
//! whose only state is an `arn:aws:states:::dynamodb:putItem` task writing a key
//! taken from the execution input — is ONE row of the first table.
//!
//! What no call is made for is still not faked: the last table pins that each such
//! `Resource` ends the execution `FAILED` with ASL's `States.Runtime` and a cause
//! that says what was not run.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION;
use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

use crate::state_machine_lifecycle::{
    error_code, string, PAST_THE_EXECUTION_DELAY, REGION, ROLE_ARN,
};

/// The `X-Amz-Target` prefix and signing name of each service a request below is
/// addressed to, from each model's own `service_info` (service shape, `sigv4Name`
/// and the `awsJson` version its protocol declares).
const SFN: Wire = Wire {
    signing_name: "states",
    target_prefix: "AWSStepFunctions",
    json_version: "1.0",
};
const DYNAMODB: Wire = Wire {
    signing_name: "dynamodb",
    target_prefix: "DynamoDB_20120810",
    json_version: "1.0",
};

/// How one service's requests are addressed on the wire.
struct Wire {
    signing_name: &'static str,
    target_prefix: &'static str,
    json_version: &'static str,
}

/// Long enough that a created table has become `ACTIVE`: "You can perform read and
/// write operations only on an ACTIVE table", and a created one is `CREATING` for
/// one `STATUS_TRANSITION`.
const PAST_THE_TABLE_TRANSITION: Duration = STATUS_TRANSITION.saturating_add(Duration::from_millis(200));

/// Step Functions and DynamoDB over ONE engine, so the call a task makes is served
/// by the same store the test reads back through DynamoDB's own API.
///
/// `pub(crate)`, not private: a sibling suite that needs a task whose integration
/// really answers drives this one rather than standing up a second copy of it.
pub(crate) struct Chain {
    router: MultiServiceRouter,
}

impl Chain {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { router }
    }

    /// Call one operation of one service and return `(status, body)`. The
    /// `Authorization` header's credential scope is what routes the request and
    /// names the region its ARNs carry.
    async fn call(&self, wire: &Wire, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{}/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0123456789abcdef",
            wire.signing_name
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{}.{REGION}.amazonaws.com", wire.signing_name))
            .header(
                "content-type",
                format!("application/x-amz-json-{}", wire.json_version),
            )
            .header("x-amz-target", format!("{}.{operation}", wire.target_prefix))
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "case {operation}: expected a JSON body, got {error} for {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// Call one Step Functions operation, requiring a 200.
    pub(crate) async fn sfn(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(&SFN, operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation}: expected 200, got {status} with body {parsed:?}"
        );
        parsed
    }

    /// Call one Step Functions operation, requiring a FAILURE, and report its status
    /// and the error code it answered.
    pub(crate) async fn sfn_err(&self, operation: &str, body: &Value) -> (u16, String) {
        let (status, parsed) = self.call(&SFN, operation, body).await;
        assert_ne!(
            status, 200,
            "case {operation}: expected a failure, got 200 with body {parsed:?}"
        );
        (status, error_code(&parsed))
    }

    /// Call one DynamoDB operation, requiring a 200.
    async fn ddb(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(&DYNAMODB, operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation}: expected 200, got {status} with body {parsed:?}"
        );
        parsed
    }

    /// Create a table keyed by a single string partition key `pk`, and wait for it
    /// to serve items.
    pub(crate) async fn create_table(&self, name: &str) {
        self.ddb(
            "CreateTable",
            &json!({
                "TableName": name,
                "KeySchema": [{ "AttributeName": "pk", "KeyType": "HASH" }],
                "AttributeDefinitions": [{ "AttributeName": "pk", "AttributeType": "S" }],
                "BillingMode": "PAY_PER_REQUEST"
            }),
        )
        .await;
        std::thread::sleep(PAST_THE_TABLE_TRANSITION);
    }

    /// The item stored under `pk`, or `None` when the table holds none.
    ///
    /// `pub(crate)`, not private: a sibling suite that asserts what the far end of a
    /// task's call really received reads it back through this one.
    pub(crate) async fn item(&self, table: &str, pk: &str) -> Option<Value> {
        let read = self
            .ddb(
                "GetItem",
                &json!({ "TableName": table, "Key": { "pk": { "S": pk } } }),
            )
            .await;
        read.get("Item").cloned()
    }

    /// Write `item` into `table` through DynamoDB's own `PutItem`.
    ///
    /// `pub(crate)`, not private: a sibling suite that needs a table a task READS from
    /// seeds it through this one rather than standing up a second call path to it.
    pub(crate) async fn put_item(&self, table: &str, item: &Value) {
        self.ddb("PutItem", &json!({ "TableName": table, "Item": item }))
            .await;
    }

    /// Create a state machine and start one execution of it, and report what the
    /// execution ended as once the clock has made the outcome due.
    pub(crate) async fn run(&self, name: &str, definition: &Value, input: &Value) -> Value {
        self.run_after(name, definition, input, Duration::ZERO).await
    }

    /// Create a state machine of `name` and report its ARN.
    ///
    /// `pub(crate)`, not private: a sibling suite that has to act WHILE an execution
    /// is still running creates and starts in two steps rather than through `run`.
    pub(crate) async fn create(&self, name: &str, definition: &Value) -> String {
        let created = self
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": name,
                    "definition": definition.to_string(),
                    "roleArn": ROLE_ARN
                }),
            )
            .await;
        string(&created, "stateMachineArn")
    }

    /// Start one execution of `machine` and report its ARN.
    pub(crate) async fn start(&self, machine: &str, name: &str, input: &Value) -> String {
        let started = self
            .sfn(
                "StartExecution",
                &json!({
                    "stateMachineArn": machine,
                    "name": name,
                    "input": input.to_string()
                }),
            )
            .await;
        string(&started, "executionArn")
    }

    /// `DescribeExecution`, as the API answers it.
    pub(crate) async fn describe(&self, execution_arn: &str) -> Value {
        self.sfn(
            "DescribeExecution",
            &json!({ "executionArn": execution_arn }),
        )
        .await
    }

    /// The same, waiting a further `owed` for a definition whose own `Wait` or
    /// `Retry` intervals push the outcome's due instant out by that much.
    async fn run_after(
        &self,
        name: &str,
        definition: &Value,
        input: &Value,
        owed: Duration,
    ) -> Value {
        let created = self
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": name,
                    "definition": definition.to_string(),
                    "roleArn": ROLE_ARN
                }),
            )
            .await;
        let machine = string(&created, "stateMachineArn");
        let started = self
            .sfn(
                "StartExecution",
                &json!({
                    "stateMachineArn": machine,
                    "name": "only",
                    "input": input.to_string()
                }),
            )
            .await;
        let execution = string(&started, "executionArn");
        std::thread::sleep(PAST_THE_EXECUTION_DELAY.saturating_add(owed));
        self.sfn("DescribeExecution", &json!({ "executionArn": execution }))
            .await
    }
}

/// A `Task` state whose only member beyond the integration is where it goes next.
fn task(resource: &str, parameters: Value) -> Value {
    json!({
        "StartAt": "Work",
        "States": {
            "Work": { "Type": "Task", "Resource": resource, "Parameters": parameters, "End": true }
        }
    })
}

/// The `output` a succeeded execution reports, parsed.
fn output(described: &Value, case: &str) -> Value {
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case {case:?}: expected SUCCEEDED, got {described:?}"
    );
    described
        .get("output")
        .and_then(Value::as_str)
        .map(|text| serde_json::from_str(text).expect("the output is JSON"))
        .unwrap_or_else(|| panic!("case {case:?}: expected an output in {described:?}"))
}

// ---------------------------------------------------------------------------
// The call is made, and its effect is really there
// ---------------------------------------------------------------------------

/// A `Task` state's integration performs the API operation its action names: the
/// item a `putItem` task wrote is readable afterwards through DynamoDB's own
/// `GetItem`, and the document a reading operation answered is the state's result.
///
/// The rows differ along what the rule generalises over: the integration FAMILY
/// (the optimized `arn:aws:states:::<service>:<action>` and the AWS SDK
/// `arn:aws:states:::aws-sdk:<service>:<action>` form), the ACTION — a write, a
/// read, an update and a delete, none of which this emulator names in code — and
/// where the request's values come from (a path into the execution input, a
/// literal, an intrinsic).
#[tokio::test]
async fn a_task_state_makes_the_call_its_resource_names() {
    let chain = Chain::new();
    chain.create_table("orders").await;

    // The recorded case: the key is taken from the event's own detail, which is the
    // execution input a rule's delivery carries.
    let placed = chain
        .run(
            "chain-optimized",
            &task(
                "arn:aws:states:::dynamodb:putItem",
                json!({
                    "TableName": "orders",
                    "Item": {
                        "pk": { "S.$": "$.detail.order_id" },
                        "status": { "S": "received" }
                    }
                }),
            ),
            &json!({ "detail": { "order_id": "o-1" } }),
        )
        .await;
    assert_eq!(
        placed.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case \"the optimized putItem integration\": expected SUCCEEDED, got {placed:?}"
    );
    assert_eq!(
        chain.item("orders", "o-1").await,
        Some(json!({ "pk": { "S": "o-1" }, "status": { "S": "received" } })),
        "case \"the optimized putItem integration\": the item the task wrote must be readable \
         through GetItem"
    );

    // The same operation through the AWS SDK integration family.
    let sdk = chain
        .run(
            "chain-aws-sdk",
            &task(
                "arn:aws:states:::aws-sdk:dynamodb:putItem",
                json!({
                    "TableName": "orders",
                    "Item": {
                        "pk": { "S.$": "$.detail.order_id" },
                        "status": { "S.$": "States.Format('{}-{}', 'from', $.detail.source)" }
                    }
                }),
            ),
            &json!({ "detail": { "order_id": "o-2", "source": "sdk" } }),
        )
        .await;
    assert_eq!(
        sdk.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case \"the aws-sdk putItem integration\": expected SUCCEEDED, got {sdk:?}"
    );
    assert_eq!(
        chain.item("orders", "o-2").await,
        Some(json!({ "pk": { "S": "o-2" }, "status": { "S": "from-sdk" } })),
        "case \"the aws-sdk putItem integration\": the item the task wrote must be readable \
         through GetItem"
    );

    // An action no code in this emulator names: the operation follows from the
    // action, so every operation of a reachable service is reachable.
    let updated = chain
        .run(
            "chain-update",
            &task(
                "arn:aws:states:::aws-sdk:dynamodb:updateItem",
                json!({
                    "TableName": "orders",
                    "Key": { "pk": { "S.$": "$.id" } },
                    "UpdateExpression": "SET #s = :s",
                    "ExpressionAttributeNames": { "#s": "status" },
                    "ExpressionAttributeValues": { ":s": { "S": "shipped" } }
                }),
            ),
            &json!({ "id": "o-1" }),
        )
        .await;
    assert_eq!(
        updated.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case \"an updateItem integration\": expected SUCCEEDED, got {updated:?}"
    );
    assert_eq!(
        chain.item("orders", "o-1").await,
        Some(json!({ "pk": { "S": "o-1" }, "status": { "S": "shipped" } })),
        "case \"an updateItem integration\": the update the task made must be readable through \
         GetItem"
    );

    // A READING operation's response document is the state's result, beside the SDK
    // response envelope an optimized integration reports it in.
    let read = chain
        .run(
            "chain-get",
            &task(
                "arn:aws:states:::dynamodb:getItem",
                json!({ "TableName": "orders", "Key": { "pk": { "S": "o-1" } } }),
            ),
            &json!({}),
        )
        .await;
    let result = output(&read, "a getItem integration");
    assert_eq!(
        result.get("Item"),
        Some(&json!({ "pk": { "S": "o-1" }, "status": { "S": "shipped" } })),
        "case \"a getItem integration\": the state's result is the response document, got \
         {result}"
    );
    assert_eq!(
        result.pointer("/SdkHttpMetadata/HttpStatusCode"),
        Some(&json!(200)),
        "case \"a getItem integration\": the result carries the status the call answered, got \
         {result}"
    );

    // A DELETE really removes the item.
    let deleted = chain
        .run(
            "chain-delete",
            &task(
                "arn:aws:states:::dynamodb:deleteItem",
                json!({ "TableName": "orders", "Key": { "pk": { "S": "o-2" } } }),
            ),
            &json!({}),
        )
        .await;
    assert_eq!(
        deleted.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case \"a deleteItem integration\": expected SUCCEEDED, got {deleted:?}"
    );
    assert_eq!(
        chain.item("orders", "o-2").await,
        None,
        "case \"a deleteItem integration\": the item the task deleted must be gone"
    );
}

/// The state's own data flow runs over what the call answered: `ResultSelector`
/// reshapes the response document, `ResultPath` places it into the state's raw
/// input, and a second `Task` reads what the first one wrote — so a chain of
/// integrations is computed, not fabricated.
#[tokio::test]
async fn the_response_document_runs_through_the_states_result_filters() {
    let chain = Chain::new();
    chain.create_table("ledger").await;

    let described = chain
        .run(
            "filters",
            &json!({
                "StartAt": "Write",
                "States": {
                    "Write": {
                        "Type": "Task",
                        "Resource": "arn:aws:states:::dynamodb:putItem",
                        "Parameters": {
                            "TableName": "ledger",
                            "Item": {
                                "pk": { "S.$": "$.id" },
                                "amount": { "N": "42" }
                            }
                        },
                        "ResultPath": null,
                        "Next": "Read"
                    },
                    "Read": {
                        "Type": "Task",
                        "Resource": "arn:aws:states:::dynamodb:getItem",
                        "Parameters": {
                            "TableName": "ledger",
                            "Key": { "pk": { "S.$": "$.id" } }
                        },
                        "ResultSelector": { "amount.$": "$.Item.amount.N" },
                        "ResultPath": "$.found",
                        "End": true
                    }
                }
            }),
            &json!({ "id": "l-1" }),
        )
        .await;

    assert_eq!(
        output(&described, "two tasks in sequence"),
        json!({ "id": "l-1", "found": { "amount": "42" } }),
        "case \"two tasks in sequence\": the second task reads what the first one wrote, through \
         the state's own result filters"
    );
}

// ---------------------------------------------------------------------------
// A refused call is the task's failure
// ---------------------------------------------------------------------------

/// One row of the failure table.
struct FailureCase {
    case: &'static str,
    definition: Value,
    input: Value,
    /// How much longer than the settle delay this row's outcome is owed: the sum of
    /// the `Retry` intervals the definition really spends.
    owed: Duration,
    /// `Err` is the `error` name the execution ends `FAILED` with; `Ok` is the
    /// output of an execution whose catcher took the failure over.
    expected: Result<Value, &'static str>,
}

/// A call the service refused is the task's OWN failure, named for the service and
/// the error code it answered — so a definition's `Retry` and `Catch` match on it,
/// and a caught failure's error output is the state's output.
///
/// The rows differ along which refusal the service makes (a table that is not
/// there, a request the service will not validate), and along what the definition
/// does with it (nothing, catch it, retry then catch it).
#[tokio::test]
async fn a_refused_call_fails_the_task_with_the_services_own_error() {
    let missing_table = json!({
        "TableName": "absent",
        "Item": { "pk": { "S": "x" } }
    });
    let cases = [
        FailureCase {
            case: "a table that is not there",
            definition: task("arn:aws:states:::dynamodb:putItem", missing_table.clone()),
            input: json!({}),
            owed: Duration::ZERO,
            expected: Err("DynamoDB.ResourceNotFoundException"),
        },
        FailureCase {
            case: "a request the service refuses to validate",
            definition: task(
                "arn:aws:states:::dynamodb:getItem",
                json!({ "TableName": "" , "Key": { "pk": { "S": "x" } } }),
            ),
            input: json!({}),
            owed: Duration::ZERO,
            // A parameter-level refusal is raised by the request framework that fronts the
            // service rather than by its API, so the shape it names belongs to no DynamoDB
            // model (that service answers the validation class under the framework's own
            // namespace, `com.amazon.coral.validate#ValidationException`). No SDK carries a
            // class for it, so the client deserializes it into the service's BASE exception,
            // which is the error name the task reports — with the code itself in the cause,
            // where the SDK's own message puts it.
            expected: Err("DynamoDB.AmazonDynamoDBException"),
        },
        FailureCase {
            case: "a Catch on the service's own error takes the failure over",
            definition: json!({
                "StartAt": "Work",
                "States": {
                    "Work": {
                        "Type": "Task",
                        "Resource": "arn:aws:states:::dynamodb:putItem",
                        "Parameters": missing_table.clone(),
                        "Catch": [{
                            "ErrorEquals": ["DynamoDB.ResourceNotFoundException"],
                            "Next": "Report",
                            "ResultPath": "$.failure"
                        }],
                        "End": true
                    },
                    "Report": { "Type": "Pass", "OutputPath": "$.failure.Error", "End": true }
                }
            }),
            input: json!({}),
            owed: Duration::ZERO,
            expected: Ok(json!("DynamoDB.ResourceNotFoundException")),
        },
        FailureCase {
            case: "a Retry on States.ALL spends its attempts and the Catch then takes it",
            definition: json!({
                "StartAt": "Work",
                "States": {
                    "Work": {
                        "Type": "Task",
                        "Resource": "arn:aws:states:::dynamodb:putItem",
                        "Parameters": missing_table.clone(),
                        "Retry": [{
                            "ErrorEquals": ["States.ALL"],
                            "MaxAttempts": 2,
                            "IntervalSeconds": 1
                        }],
                        "Catch": [{ "ErrorEquals": ["States.ALL"], "Next": "Report" }],
                        "End": true
                    },
                    "Report": { "Type": "Pass", "Result": "recovered", "End": true }
                }
            }),
            input: json!({}),
            // Two attempts at `IntervalSeconds` 1 and the documented `BackoffRate`
            // 2.0: one second, then two, really spent before the catcher takes it.
            owed: Duration::from_secs(3),
            expected: Ok(json!("recovered")),
        },
    ];

    let chain = Chain::new();
    for (index, case) in cases.iter().enumerate() {
        let described = chain
            .run_after(
                &format!("refused-{index}"),
                &case.definition,
                &case.input,
                case.owed,
            )
            .await;
        match &case.expected {
            Ok(expected) => assert_eq!(
                &output(&described, case.case),
                expected,
                "case {:?}: expected the output {expected}, got {described:?}",
                case.case
            ),
            Err(error) => {
                assert_eq!(
                    described.get("status").and_then(Value::as_str),
                    Some("FAILED"),
                    "case {:?}: expected FAILED, got {described:?}",
                    case.case
                );
                assert_eq!(
                    described.get("error").and_then(Value::as_str),
                    Some(*error),
                    "case {:?}: expected the error {error}, got {described:?}",
                    case.case
                );
                assert!(
                    described
                        .get("cause")
                        .and_then(Value::as_str)
                        .is_some_and(|cause| !cause.is_empty()),
                    "case {:?}: a failed task must say what happened, got {described:?}",
                    case.case
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// What no call is made for
// ---------------------------------------------------------------------------

/// A `Resource` this emulator makes no call for is not faked: the execution ends
/// `FAILED` with ASL's `States.Runtime` and a cause that says what was not run, and
/// nothing was written anywhere.
///
/// The rows span the classes [`state::integration::read`] declines: a service whose
/// request is not one JSON document, a run-a-job pattern over an integration whose
/// job this emulator cannot report or stop, an integration pattern this service does
/// not name, a bare resource ARN, and a `Resource` that is no
/// ARN at all. An ACTIVITY `Resource` is not among them — its work is a worker's, so
/// the walk schedules a task and the execution waits at that state (its own suites
/// cover that) — and neither is a run-a-job pattern this emulator DOES perform, whose
/// job is started, polled and resolved (`sync_service_integration.rs`), nor the
/// callback pattern, whose call is made and whose token is then waited for
/// (`callback_service_integration.rs`).
#[tokio::test]
async fn a_resource_no_call_is_made_for_fails_rather_than_being_faked() {
    let cases: [(&str, &str); 5] = [
        (
            "a service with no wire facts carried for it at all",
            "arn:aws:states:::batch:submitJob",
        ),
        (
            "the run-a-job pattern over an integration whose job cannot be reported",
            "arn:aws:states:::dynamodb:putItem.sync",
        ),
        (
            "an integration pattern this service does not name",
            "arn:aws:states:::dynamodb:putItem.whenever",
        ),
        (
            // A bare resource ARN of a service whose resources are not work in
            // themselves. A Lambda FUNCTION's ARN is not one of these: it names the
            // function the task invokes.
            "a bare resource ARN",
            "arn:aws:dynamodb:us-west-2:000000000000:table/orders",
        ),
        ("a Resource that is no ARN at all", "putItem"),
    ];

    let chain = Chain::new();
    for (index, (case, resource)) in cases.iter().enumerate() {
        let described = chain
            .run(
                &format!("unrun-{index}"),
                &task(resource, json!({ "TableName": "orders" })),
                &json!({}),
            )
            .await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected FAILED, got {described:?}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some("States.Runtime"),
            "case {case:?}: expected States.Runtime, got {described:?}"
        );
        let cause = described
            .get("cause")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            cause.contains("nothing was run for it"),
            "case {case:?}: the cause must say nothing ran, got {cause:?}"
        );
    }
}

/// A `Task` state carrying a member whose semantics this emulator does not
/// interpret is not run either — the rule the walk applies at every state — so a member
/// that decides what the state does is reported rather than quietly ignored.
///
/// The rows are members of the OTHER query language: a state written in JSONPath reads and
/// writes its data through the `*Path` family and the payload templates, so a JSONata
/// data-flow member written on one names a behaviour this emulator does not give it.
#[tokio::test]
async fn a_task_member_this_emulator_does_not_interpret_is_not_ignored() {
    let chain = Chain::new();
    chain.create_table("members").await;
    let parameters = json!({ "TableName": "members", "Item": { "pk": { "S": "m-1" } } });
    let cases: [(&str, Value); 2] = [
        // The document a JSONata `Task` hands its work, which is `Parameters` in this
        // state's own language.
        ("Arguments", json!({ "TableName": "members" })),
        // What a JSONata state puts out, which is the `Result*`/`OutputPath` family here.
        ("Output", json!({ "done": true })),
    ];

    for (index, (member, value)) in cases.iter().enumerate() {
        let mut definition = task("arn:aws:states:::dynamodb:putItem", parameters.clone());
        definition["States"]["Work"][*member] = value.clone();
        let described = chain
            .run(&format!("member-{index}"), &definition, &json!({}))
            .await;
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some("States.Runtime"),
            "case {member:?}: expected States.Runtime, got {described:?}"
        );
        assert_eq!(
            chain.item("members", "m-1").await,
            None,
            "case {member:?}: a state that was not run must have written nothing"
        );
    }
}

/// One row of the timeout table: the case, the `TimeoutSeconds` member, its
/// `TimeoutSecondsPath` form, the execution input, and the key the call writes when
/// it is made at all.
type TimeoutRow = (
    &'static str,
    Option<Value>,
    Option<&'static str>,
    Value,
    Option<&'static str>,
);

/// What a `Task`'s `TimeoutSeconds` is read as, over a TABLE spanning the forms a
/// caller can write it in: the member and its `*Path` form, a figure the in-process
/// call is far inside, and figures the language does not define.
///
/// The rule: the member is "a positive integer" of seconds and it bounds the call's
/// own span, so a figure the call is inside changes nothing (the task reports what
/// the service answered), and a figure that is not a positive whole number is a
/// defect in the definition — reported as the failure it is, from either form, and
/// never a panic in the walk whatever magnitude a caller writes.
#[tokio::test]
async fn a_timeout_is_read_as_the_positive_whole_seconds_it_bounds_the_call_with() {
    // (case, the TimeoutSeconds member, its TimeoutSecondsPath form, the input, the
    //  key the call writes when it is made at all)
    let rows: [TimeoutRow; 5] = [
        (
            "a figure the in-process call is far inside",
            Some(json!(60)),
            None,
            json!({}),
            Some("t-1"),
        ),
        (
            "the same figure reached through TimeoutSecondsPath",
            None,
            Some("$.allowed"),
            json!({ "allowed": 30 }),
            Some("t-2"),
        ),
        (
            "a figure past every whole number of seconds a clock carries",
            Some(json!(1e20)),
            None,
            json!({}),
            None,
        ),
        (
            "that figure reached through TimeoutSecondsPath",
            None,
            Some("$.allowed"),
            json!({ "allowed": 1e20 }),
            None,
        ),
        (
            "a figure that is not a whole number",
            Some(json!(1.5)),
            None,
            json!({}),
            None,
        ),
    ];

    let chain = Chain::new();
    chain.create_table("bounded").await;

    for (index, (case, member, path, input, written)) in rows.iter().enumerate() {
        let key = format!("t-{}", index + 1);
        let mut definition = task(
            "arn:aws:states:::dynamodb:putItem",
            json!({ "TableName": "bounded", "Item": { "pk": { "S": key } } }),
        );
        if let Some(member) = member {
            definition["States"]["Work"]["TimeoutSeconds"] = member.clone();
        }
        if let Some(path) = path {
            definition["States"]["Work"]["TimeoutSecondsPath"] = json!(path);
        }

        let described = chain
            .run(&format!("timeout-{index}"), &definition, input)
            .await;
        match written {
            Some(expected_key) => {
                assert_eq!(
                    described.get("status").and_then(Value::as_str),
                    Some("SUCCEEDED"),
                    "case {case:?}: expected SUCCEEDED, got {described:?}"
                );
                assert!(
                    chain.item("bounded", expected_key).await.is_some(),
                    "case {case:?}: the call must still have been made"
                );
            }
            None => {
                assert_eq!(
                    described.get("error").and_then(Value::as_str),
                    Some("States.Runtime"),
                    "case {case:?}: expected States.Runtime, got {described:?}"
                );
                assert_eq!(
                    chain.item("bounded", &key).await,
                    None,
                    "case {case:?}: a state that was not run must have written nothing"
                );
            }
        }
    }
}

/// The error code of the one refusal this suite asks a service for directly, so the
/// rows above name a code DynamoDB really answers rather than one this suite made
/// up.
#[tokio::test]
async fn the_services_refusal_is_the_code_the_task_error_is_built_from() {
    let chain = Chain::new();
    let (status, body) = chain
        .call(
            &DYNAMODB,
            "PutItem",
            &json!({ "TableName": "absent", "Item": { "pk": { "S": "x" } } }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"a table that is not there\": expected 400, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"a table that is not there\": got {body:?}"
    );
}
