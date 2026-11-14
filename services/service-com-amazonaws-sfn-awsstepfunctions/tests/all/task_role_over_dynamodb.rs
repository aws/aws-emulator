// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the EXECUTION ROLE a `Task` state's DynamoDB integration call
//! is made as, driven over all three services' own wire protocols through the
//! production `MultiServiceRouter` with Step Functions, IAM and DynamoDB mounted on
//! ONE engine — the shape `core/router/src/lib.rs` mounts them in — so the policies
//! the call is judged against are the documents IAM's own handlers wrote and the
//! far-end effect is read back through DynamoDB's own API.
//!
//! The rule under test: a `Task` state's integration call is made as the state
//! machine's `roleArn`, so it is subject to an IAM access request over the action the
//! invoked operation publishes and the ARN of the resource its request names — and
//! where that request names the resource by NAME rather than by ARN, the resource is
//! the ARN of the resource of that name in the Region the call is made in. A call the
//! role's policies do not allow is NOT made: the task fails with the invoked service's
//! own access-denied refusal, the execution ends `FAILED`, and the table is untouched.
//! A call they do allow is made exactly as before, which is the positive control in
//! every row.
//!
//! The rows differ along what the rule generalises over: the ACTION (a write, a read,
//! an update and a delete, each publishing its own IAM action), the integration FAMILY
//! (the optimized `arn:aws:states:::dynamodb:<action>` and the AWS SDK
//! `arn:aws:states:::aws-sdk:dynamodb:<action>` form), how the request SPELLS the
//! table (its name, and its ARN, which the operation takes either way), and what the
//! policies say about it (a grant over that table, no statement at all, an explicit
//! `Deny` over a grant, and a wildcard grant).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION;
use service_com_amazonaws_sfn_awsstepfunctions::state::EVENT_TASK_SUBMITTED;
use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

use crate::execution_data_encryption::{form_encode, xml_element};
use crate::state_machine_lifecycle::{string, PAST_THE_EXECUTION_DELAY, REGION};

/// How each service's requests are addressed on the wire, from each model's own
/// `service_info` (service shape, `sigv4Name` and the `awsJson` version its protocol
/// declares).
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
/// write operations only on an ACTIVE table", and a created one is `CREATING` for one
/// `STATUS_TRANSITION`.
const PAST_THE_TABLE_TRANSITION: Duration =
    STATUS_TRANSITION.saturating_add(Duration::from_millis(200));

/// The error a `Task` state reports when DynamoDB refuses the call it made with an
/// error that operation does not model: DynamoDB's base SDK exception, as Step
/// Functions names the service in a task error.
const REFUSAL_ERROR: &str = "DynamoDB.AmazonDynamoDBException";

/// The code the refused answer came back under, as the cause reports it.
const REFUSAL_CODE: &str = "Error Code: AccessDeniedException";

/// Step Functions, IAM and DynamoDB over ONE engine.
struct Chain {
    router: MultiServiceRouter,
}

impl Chain {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One `awsJson` call to one service, requiring a 200. The `Authorization`
    /// header's credential scope is what routes the request and names the Region its
    /// ARNs carry.
    async fn json(&self, wire: &Wire, operation: &str, body: &Value) -> Value {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{}/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0123456789abcdef",
            wire.signing_name
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header(
                "host",
                format!("{}.{REGION}.amazonaws.com", wire.signing_name),
            )
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
        let parsed: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        assert_eq!(
            status, 200,
            "case {operation}: expected 200, got {status} with body {parsed:?}"
        );
        parsed
    }

    async fn sfn(&self, operation: &str, body: &Value) -> Value {
        self.json(&SFN, operation, body).await
    }

    async fn ddb(&self, operation: &str, body: &Value) -> Value {
        self.json(&DYNAMODB, operation, body).await
    }

    /// One `awsQuery` call to IAM, signed with the same credential.
    async fn iam(&self, action: &str, params: &[(&str, String)]) -> String {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iam/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(&response.into_body()).to_string();
        assert_eq!(
            status, 200,
            "case {action}: expected 200 from IAM, got {status} with {text}"
        );
        text
    }

    /// An IAM role carrying `policy` as its inline policy, by its own ARN.
    async fn role(&self, name: &str, policy: &Value) -> String {
        let created = self
            .iam(
                "CreateRole",
                &[
                    ("RoleName", name.to_string()),
                    ("AssumeRolePolicyDocument", trust_document().to_string()),
                ],
            )
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreateRole answers the role Arn, got {created}"));
        self.iam(
            "PutRolePolicy",
            &[
                ("RoleName", name.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
        arn
    }

    /// Create a table keyed by a single string partition key `pk`, wait for it to
    /// serve items, and report the ARN DynamoDB minted for it.
    async fn create_table(&self, name: &str) -> String {
        let created = self
            .ddb(
                "CreateTable",
                &json!({
                    "TableName": name,
                    "KeySchema": [{ "AttributeName": "pk", "KeyType": "HASH" }],
                    "AttributeDefinitions": [{ "AttributeName": "pk", "AttributeType": "S" }],
                    "BillingMode": "PAY_PER_REQUEST"
                }),
            )
            .await;
        let arn = created
            .get("TableDescription")
            .and_then(|described| described.get("TableArn"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("CreateTable answers a TableArn, got {created:?}"))
            .to_string();
        std::thread::sleep(PAST_THE_TABLE_TRANSITION);
        arn
    }

    /// Store one item under `pk`, so a row can show whether a refused call changed it.
    async fn seed(&self, table: &str, pk: &str) {
        self.ddb(
            "PutItem",
            &json!({
                "TableName": table,
                "Item": { "pk": { "S": pk }, "status": { "S": "seeded" } }
            }),
        )
        .await;
    }

    /// The item stored under `pk`, or `None` when the table holds none.
    async fn item(&self, table: &str, pk: &str) -> Option<Value> {
        let read = self
            .ddb(
                "GetItem",
                &json!({ "TableName": table, "Key": { "pk": { "S": pk } } }),
            )
            .await;
        read.get("Item").cloned()
    }

    /// Create a state machine under `role_arn` running `definition`, start one
    /// execution of it, and report `(the execution ARN, what it ended as)` once the
    /// clock has made the outcome due.
    async fn ran(&self, name: &str, definition: &Value, role_arn: &str) -> (String, Value) {
        let created = self
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": name,
                    "definition": definition.to_string(),
                    "roleArn": role_arn
                }),
            )
            .await;
        let machine = string(&created, "stateMachineArn");
        let started = self
            .sfn(
                "StartExecution",
                &json!({ "stateMachineArn": machine, "name": "only" }),
            )
            .await;
        let execution = string(&started, "executionArn");
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        let described = self
            .sfn("DescribeExecution", &json!({ "executionArn": execution }))
            .await;
        (execution, described)
    }

    /// The event types of one execution's history.
    async fn history(&self, execution_arn: &str) -> Vec<String> {
        let answered = self
            .sfn(
                "GetExecutionHistory",
                &json!({ "executionArn": execution_arn, "maxResults": 100 }),
            )
            .await;
        answered
            .get("events")
            .and_then(Value::as_array)
            .map(|events| {
                events
                    .iter()
                    .filter_map(|event| event.get("type").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The trust document a Step Functions execution role carries, which is what lets this
/// service act as it.
fn trust_document() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "states.amazonaws.com" },
            "Action": "sts:AssumeRole"
        }]
    })
}

/// A definition whose one `Task` state makes `resource`'s call with `parameters`.
fn task(resource: &str, parameters: Value) -> Value {
    json!({
        "StartAt": "Work",
        "States": {
            "Work": { "Type": "Task", "Resource": resource, "Parameters": parameters, "End": true }
        }
    })
}

/// The four item-level actions, each with the IAM action it publishes and the request
/// an item-level call of it is made with.
///
/// The members are DynamoDB's own, from
/// `smithy_query(query="operation",operation="PutItem,GetItem,UpdateItem,DeleteItem",
/// model_path="services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/
/// com-amazonaws-dynamodb-dynamodb-20120810.json")`: `TableName` and `Item` for a
/// write, `TableName` and `Key` for the other three, with `UpdateItem` taking an
/// `UpdateExpression` beside them.
fn parameters(action: &str, table: &str, pk: &str) -> Value {
    let key = json!({ "pk": { "S": pk } });
    match action {
        "putItem" => json!({
            "TableName": table,
            "Item": { "pk": { "S": pk }, "status": { "S": "written" } }
        }),
        "updateItem" => json!({
            "TableName": table,
            "Key": key,
            "UpdateExpression": "SET #s = :s",
            "ExpressionAttributeNames": { "#s": "status" },
            "ExpressionAttributeValues": { ":s": { "S": "updated" } }
        }),
        _ => json!({ "TableName": table, "Key": key }),
    }
}

/// Which table a row addresses, and under which spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Addressed {
    /// The table the role is granted every item-level action on.
    Granted,
    /// That same table, written as its ARN rather than its name.
    GrantedByArn,
    /// A table no statement of the role's policy is about.
    Unstated,
    /// A table an explicit `Deny` covers, over a grant that would otherwise allow it.
    Denied,
}

/// One row: the case, the `Resource` its `Task` names, the table it addresses, and
/// whether the role's policies let the call be made.
type Row = (&'static str, &'static str, Addressed, bool);

/// A `Task` state's DynamoDB call is made only when the execution role's policies
/// allow the action on the table the request names.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tasks_dynamodb_call_is_made_only_when_the_execution_role_allows_it() {
    let chain = Chain::new();
    let granted = chain.create_table("granted").await;
    let unstated = chain.create_table("unstated").await;
    let denied = chain.create_table("denied").await;
    // Every item-level action on one table, the same four on another with one of them
    // explicitly denied, and nothing at all about the third.
    let actions = json!([
        "dynamodb:PutItem",
        "dynamodb:GetItem",
        "dynamodb:UpdateItem",
        "dynamodb:DeleteItem"
    ]);
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": actions, "Resource": [granted, denied] },
            { "Effect": "Deny", "Action": actions, "Resource": denied }
        ]
    });
    let role_arn = chain.role("DynamoDbTaskRole", &policy).await;

    let rows: [Row; 10] = [
        (
            "a write the role is granted, optimized",
            "arn:aws:states:::dynamodb:putItem",
            Addressed::Granted,
            true,
        ),
        (
            "the same write through the AWS SDK integration",
            "arn:aws:states:::aws-sdk:dynamodb:putItem",
            Addressed::Granted,
            true,
        ),
        (
            "the same write naming the table by its ARN, which is the same table",
            "arn:aws:states:::dynamodb:putItem",
            Addressed::GrantedByArn,
            true,
        ),
        (
            "a write on a table no statement is about",
            "arn:aws:states:::dynamodb:putItem",
            Addressed::Unstated,
            false,
        ),
        (
            "a write an explicit deny covers, over a grant that would allow it",
            "arn:aws:states:::dynamodb:putItem",
            Addressed::Denied,
            false,
        ),
        (
            "a read the role is granted",
            "arn:aws:states:::dynamodb:getItem",
            Addressed::Granted,
            true,
        ),
        (
            "a read on a table no statement is about",
            "arn:aws:states:::dynamodb:getItem",
            Addressed::Unstated,
            false,
        ),
        (
            "an update the role is granted",
            "arn:aws:states:::dynamodb:updateItem",
            Addressed::Granted,
            true,
        ),
        (
            "a delete on a table no statement is about",
            "arn:aws:states:::dynamodb:deleteItem",
            Addressed::Unstated,
            false,
        ),
        (
            "a delete an explicit deny covers, through the AWS SDK integration",
            "arn:aws:states:::aws-sdk:dynamodb:deleteItem",
            Addressed::Denied,
            false,
        ),
    ];

    for (index, (case, resource, addressed, allowed)) in rows.into_iter().enumerate() {
        let (table, table_arn) = match addressed {
            Addressed::Granted => ("granted", granted.as_str()),
            Addressed::GrantedByArn => (granted.as_str(), granted.as_str()),
            Addressed::Unstated => ("unstated", unstated.as_str()),
            Addressed::Denied => ("denied", denied.as_str()),
        };
        // Which table the far-end read goes to, whatever spelling the task used.
        let held = match addressed {
            Addressed::Granted | Addressed::GrantedByArn => "granted",
            Addressed::Unstated => "unstated",
            Addressed::Denied => "denied",
        };
        let action = resource
            .rsplit(':')
            .next()
            .expect("every row's Resource names an action");
        let pk = format!("row-{index}");
        // Every action but the write needs an item to act on, and a refused write must
        // leave the table without one: a write's key is left empty on purpose.
        if action != "putItem" {
            chain.seed(held, &pk).await;
        }

        let definition = task(resource, parameters(action, table, &pk));
        let machine_name = format!("ddb-role-{index}");
        let (execution_arn, described) = chain.ran(&machine_name, &definition, &role_arn).await;
        let types = chain.history(&execution_arn).await;
        let stored = chain.item(held, &pk).await;

        if allowed {
            assert_eq!(
                described.get("status").and_then(Value::as_str),
                Some("SUCCEEDED"),
                "case {case:?}: expected the call the role allows to be made, got {described:?}"
            );
            match action {
                "putItem" => assert_eq!(
                    stored,
                    Some(json!({ "pk": { "S": pk }, "status": { "S": "written" } })),
                    "case {case:?}: expected the item the task wrote"
                ),
                "updateItem" => assert_eq!(
                    stored,
                    Some(json!({ "pk": { "S": pk }, "status": { "S": "updated" } })),
                    "case {case:?}: expected the item the task updated"
                ),
                "deleteItem" => assert_eq!(
                    stored, None,
                    "case {case:?}: expected the item the task deleted to be gone"
                ),
                _ => assert!(
                    stored.is_some(),
                    "case {case:?}: a read changes nothing, so the item must still be there"
                ),
            }
            continue;
        }

        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected FAILED, got {described:?}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some(REFUSAL_ERROR),
            "case {case:?}: expected {REFUSAL_ERROR}, got {described:?}"
        );
        let cause = string(&described, "cause");
        // The principal is the SESSION of the role the call was made as, under the name
        // the assume gave it.
        let principal =
            crate::task_refusal_session_principal::session_principal(&role_arn, &cause);
        for fragment in [
            REFUSAL_CODE,
            &format!("on resource: {table_arn} "),
            &format!("dynamodb:{}", upper_first(action)),
            &principal,
        ] {
            assert!(
                cause.contains(fragment),
                "case {case:?}: expected the cause to carry {fragment:?}, got {cause:?}"
            );
        }
        assert!(
            !types.contains(&EVENT_TASK_SUBMITTED.to_string()),
            "case {case:?}: nothing was submitted, so no {EVENT_TASK_SUBMITTED} may appear, got \
             {types:?}"
        );
        // And the table the role may not act on is exactly as it was.
        let expected = match action {
            "putItem" => None,
            _ => Some(json!({ "pk": { "S": pk }, "status": { "S": "seeded" } })),
        };
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the table the call was refused on to be untouched"
        );
    }
}

/// A wildcard grant allows the call, so it is made.
///
/// The way a DynamoDB call goes ahead for a reason other than a statement naming its
/// exact table: a grant written with IAM's own wildcards. Whether the role can be
/// ASSUMED at all is the question before this one, which
/// `tests/all/execution_role_assumption.rs` holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wildcard_grant_lets_the_dynamodb_call_be_made() {
    let chain = Chain::new();
    chain.create_table("wildcard").await;
    let wildcard = json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": "dynamodb:*", "Resource": "*" }]
    });
    let wildcard_role = chain.role("WildcardDynamoDbRole", &wildcard).await;
    let rows: [(&str, String); 1] = [("a wildcard grant", wildcard_role)];

    for (index, (case, role_arn)) in rows.into_iter().enumerate() {
        let pk = format!("wildcard-{index}");
        let definition = task(
            "arn:aws:states:::dynamodb:putItem",
            parameters("putItem", "wildcard", &pk),
        );
        let (_execution_arn, described) = chain
            .ran(&format!("ddb-wildcard-{index}"), &definition, &role_arn)
            .await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected the call to be made, got {described:?}"
        );
        assert_eq!(
            chain.item("wildcard", &pk).await,
            Some(json!({ "pk": { "S": pk }, "status": { "S": "written" } })),
            "case {case:?}: expected the item the task wrote"
        );
    }
}

/// `action` with its first character upper-cased, which is how the operation an
/// integration's lowerCamel action names publishes its IAM action.
fn upper_first(action: &str) -> String {
    let mut characters = action.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}
