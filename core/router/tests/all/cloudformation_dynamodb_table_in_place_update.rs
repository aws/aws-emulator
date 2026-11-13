// SPDX-License-Identifier: CC0-1.0

//! An `UpdateStack` that changes an `AWS::DynamoDB::Table` in place is not reported complete
//! before Amazon DynamoDB reports that table settled again — so the first call a client makes
//! after the stack completes finds a table it can use and change.
//!
//! The rule under test: **CloudFormation does not report a resource (and so the stack
//! operation holding it) CHANGED while the service that owns it still reports the resource
//! short of the state it is settled in; it holds the resource `UPDATE_IN_PROGRESS`, under the
//! identifier that service already assigned, until that service reports it settled.** For a
//! DynamoDB table the settled state is `ACTIVE`: "UpdateTable is an asynchronous operation;
//! while it's executing, the table status changes from ACTIVE to UPDATING … While it's
//! UPDATING, you can't issue another UpdateTable request" (`UpdateTable`), and `TableStatus`
//! is "ACTIVE" once the change asked for is in force. Two crates have to agree for that to
//! hold (CloudFormation makes the calls; DynamoDB decides the table's status), so the suite
//! lives here rather than in either one.
//!
//! What this pins is the pair of observations an infrastructure-as-code client makes: the
//! instant the stack first reports the update complete, DynamoDB reports the table `ACTIVE`
//! with the configuration the new template declared, and takes the next control-plane change
//! of it. A stack reported complete while its table is still `UPDATING` fails these cases,
//! because every DynamoDB waiter (`table_exists`/`table_updated`, `aws dynamodb wait`, a CDK
//! or Terraform convergence loop) then blocks on a table that is in fact ready, and the
//! `UpdateTable` the next deployment step makes is refused as a conflict.
//!
//! The rows differ along WHICH no-interruption property the update changes — the class the
//! rule generalises over, taken from the members `UpdateTable` carries: the read half of the
//! provisioned capacity (the recorded case), the write half, both halves at once, the storage
//! class, the warm pair, and the one flag that service applies with the call itself. The
//! recorded case is one row of it. Every expected value is computed here from what the row
//! declares, never read back out of the answer being checked.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery CloudFormation caller, the poll that waits for a stack to FIRST report a
// status, the XML element readers and the SigV4 header builder are each written ONCE in this
// test crate: a suite that copies one drifts from the others.
use crate::cloudformation_apigatewayv2_resources::cloudformation;
use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{elements, physical_ids_of};
use crate::rds_docdb_neptune_signing_name_routing::authorization;

/// The Region every request here is signed for — the Region `wait_for_stack_status` polls in
/// — and so the Region the stack's table belongs to.
const REGION: &str = "us-east-1";

/// The `StackStatus` values the rule is stated in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";

/// The `TableStatus` a table is settled in, and the one it reports while a change of it is
/// still being applied.
const ACTIVE: &str = "ACTIVE";

/// The logical id every row declares its table under.
const TABLE_LOGICAL_ID: &str = "Table";

/// The partition key every row's table is declared with.
const KEY_ATTRIBUTE: &str = "pk";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them: CloudFormation makes the calls, DynamoDB serves them
/// and decides the table's status.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(engine),
    );
    router
}

/// Call one Amazon DynamoDB operation over its own awsJson1_0 wire.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("dynamodb.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .header("authorization", authorization("dynamodb"))
        .body(Bytes::from(request.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// What `DescribeTable` reports for `name`, with the HTTP status it answered.
async fn described_table(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    dynamodb(router, "DescribeTable", &json!({"TableName": name})).await
}

/// The physical id the stack publishes for its table — the name a client calls DynamoDB with.
async fn published_table_name(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (status, resources) =
        cloudformation(router, "DescribeStackResources", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with body \
         {resources}"
    );
    physical_ids_of(&resources)
        .remove(TABLE_LOGICAL_ID)
        .unwrap_or_else(|| panic!("case {case:?}: no physical id for the table in {resources}"))
}

/// The template a row deploys: one table declared with `properties`, under the name the row
/// gives it, publishing nothing else.
fn template(table_name: &str, properties: Value) -> String {
    let mut declared = json!({
        "TableName": table_name,
        "KeySchema": [{"AttributeName": KEY_ATTRIBUTE, "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": KEY_ATTRIBUTE, "AttributeType": "S"}],
    });
    let object = declared
        .as_object_mut()
        .expect("the declaration is an object");
    for (member, value) in properties
        .as_object()
        .expect("a row's properties are an object")
    {
        object.insert(member.clone(), value.clone());
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            TABLE_LOGICAL_ID: {
                "Type": "AWS::DynamoDB::Table",
                "Properties": declared,
            }
        }
    })
    .to_string()
}

/// The properties a `PROVISIONED` table is declared with at `read`/`write` capacity.
fn provisioned(read: i64, write: i64) -> Value {
    json!({
        "BillingMode": "PROVISIONED",
        "ProvisionedThroughput": {"ReadCapacityUnits": read, "WriteCapacityUnits": write},
    })
}

/// One row of the table: what the stack declares, what the in-place update then declares, and
/// the member of the settled `DescribeTable` answer the change has to show up in.
struct Row {
    case: &'static str,
    /// The table name both of this row's templates declare. Distinct per row: every row
    /// deploys into the one account and Region this suite's engine holds.
    table: &'static str,
    /// The properties the create declares beside the key schema.
    create: fn() -> Value,
    /// The properties the update declares beside the key schema — the same key schema and the
    /// same name, so the change is an in-place one rather than a replacement.
    update: fn() -> Value,
    /// JSON pointers into the `Table` the settled read answers, each with the value the
    /// updated declaration puts there — one per member the row's update moved.
    in_force: Vec<(&'static str, Value)>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            // The recorded case: the read half of a provisioned capacity moved.
            case: "the read capacity raised",
            table: "cfn-ddb-read-capacity",
            create: || provisioned(1, 1),
            update: || provisioned(2, 1),
            in_force: vec![("/ProvisionedThroughput/ReadCapacityUnits", json!(2))],
        },
        Row {
            case: "the write capacity raised",
            table: "cfn-ddb-write-capacity",
            create: || provisioned(1, 1),
            update: || provisioned(1, 3),
            in_force: vec![("/ProvisionedThroughput/WriteCapacityUnits", json!(3))],
        },
        Row {
            case: "both halves of the capacity moved at once",
            table: "cfn-ddb-both-capacities",
            create: || provisioned(2, 2),
            update: || provisioned(4, 5),
            in_force: vec![
                ("/ProvisionedThroughput/ReadCapacityUnits", json!(4)),
                ("/ProvisionedThroughput/WriteCapacityUnits", json!(5)),
            ],
        },
        Row {
            case: "the read capacity lowered",
            table: "cfn-ddb-lowered-capacity",
            create: || provisioned(6, 2),
            update: || provisioned(1, 2),
            in_force: vec![
                ("/ProvisionedThroughput/ReadCapacityUnits", json!(1)),
                ("/ProvisionedThroughput/WriteCapacityUnits", json!(2)),
            ],
        },
        Row {
            case: "the storage class changed",
            table: "cfn-ddb-table-class",
            create: || {
                let mut properties = provisioned(1, 1);
                properties["TableClass"] = json!("STANDARD");
                properties
            },
            update: || {
                let mut properties = provisioned(1, 1);
                properties["TableClass"] = json!("STANDARD_INFREQUENT_ACCESS");
                properties
            },
            in_force: vec![(
                "/TableClassSummary/TableClass",
                json!("STANDARD_INFREQUENT_ACCESS"),
            )],
        },
        Row {
            case: "the warm pair raised",
            table: "cfn-ddb-warm-throughput",
            create: || {
                let mut properties = provisioned(1, 1);
                properties["WarmThroughput"] =
                    json!({"ReadUnitsPerSecond": 12_000, "WriteUnitsPerSecond": 4_000});
                properties
            },
            update: || {
                let mut properties = provisioned(1, 1);
                properties["WarmThroughput"] =
                    json!({"ReadUnitsPerSecond": 24_000, "WriteUnitsPerSecond": 8_000});
                properties
            },
            in_force: vec![
                ("/WarmThroughput/ReadUnitsPerSecond", json!(24_000_i64)),
                ("/WarmThroughput/WriteUnitsPerSecond", json!(8_000_i64)),
            ],
        },
        Row {
            // The one change DynamoDB applies with the call itself, so the table never leaves
            // ACTIVE: the completed update still has to report the table settled, and the
            // declared flag in force.
            case: "only the deletion-protection flag",
            table: "cfn-ddb-deletion-protection",
            create: || {
                let mut properties = provisioned(1, 1);
                properties["DeletionProtectionEnabled"] = json!(false);
                properties
            },
            update: || {
                let mut properties = provisioned(1, 1);
                properties["DeletionProtectionEnabled"] = json!(true);
                properties
            },
            in_force: vec![("/DeletionProtectionEnabled", json!(true))],
        },
    ]
}

/// Every row: the stack reaches `UPDATE_COMPLETE`, and at that FIRST observation DynamoDB
/// reports the table `ACTIVE` with the updated declaration in force, serves an item write,
/// and takes the next control-plane change of the table rather than refusing it as a
/// conflict.
#[tokio::test]
async fn an_in_place_table_update_completes_only_once_dynamodb_settles_the_table() {
    for (index, row) in rows().into_iter().enumerate() {
        let router = build_router();
        let case = row.case;
        let stack_name = format!("ddb-in-place-update-{index}");

        // ---- The create the update is deployed against. ----
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &template(row.table, (row.create)())),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );
        let (stack_status, described) =
            wait_for_stack_status(&router, &stack_name, CREATE_COMPLETE).await;
        assert_eq!(
            stack_status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE} before the update, got \
             {described}"
        );
        let created_name = published_table_name(&router, case, &stack_name).await;
        assert_eq!(
            created_name, row.table,
            "case {case:?}: expected the create to publish the declared table name {:?}, got \
             {created_name:?}",
            row.table
        );

        // ---- The in-place update. ----
        let (status, body) = cloudformation(
            &router,
            "UpdateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &template(row.table, (row.update)())),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
        );
        let (stack_status, described) =
            wait_for_stack_status(&router, &stack_name, UPDATE_COMPLETE).await;
        let reasons = if stack_status == UPDATE_COMPLETE {
            Vec::new()
        } else {
            let (_, events) =
                cloudformation(&router, "DescribeStackEvents", &[("StackName", &stack_name)]).await;
            elements(&events, "ResourceStatusReason")
        };
        assert_eq!(
            stack_status, UPDATE_COMPLETE,
            "case {case:?}: expected the stack to reach {UPDATE_COMPLETE}, got {described} with \
             resource failures {reasons:?}"
        );

        // ---- The table, read the instant that completion was first observed. ----
        let updated_name = published_table_name(&router, case, &stack_name).await;
        assert_eq!(
            updated_name, row.table,
            "case {case:?}: an in-place update keeps the table's identity, expected {:?}, got \
             {updated_name:?}",
            row.table
        );
        let (status, answered) = described_table(&router, &updated_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DynamoDB to resolve the table the completed update \
             published ({updated_name:?}), got {status} with {answered}"
        );
        let table = &answered["Table"];
        assert_eq!(
            table["TableStatus"],
            json!(ACTIVE),
            "case {case:?}: expected the table to be {ACTIVE} the instant the stack reported \
             {UPDATE_COMPLETE}, got {answered}"
        );
        for (pointer, expected) in &row.in_force {
            assert_eq!(
                table.pointer(pointer),
                Some(expected),
                "case {case:?}: expected the settled table to report {pointer} as {expected}, \
                 got {answered}"
            );
        }
        // The identity and the schema an in-place update leaves alone are still what the
        // create declared.
        assert_eq!(
            table["TableArn"],
            json!(format!(
                "arn:aws:dynamodb:{REGION}:{}:table/{}",
                engine::emulator_account_id(),
                row.table
            )),
            "case {case:?}: expected the table's ARN to survive an in-place update, got {answered}"
        );
        assert_eq!(
            table["KeySchema"],
            json!([{"AttributeName": KEY_ATTRIBUTE, "KeyType": "HASH"}]),
            "case {case:?}: expected the key schema to survive an in-place update, got {answered}"
        );

        // A settled table serves the data plane...
        let (status, written) = dynamodb(
            &router,
            "PutItem",
            &json!({
                "TableName": updated_name,
                "Item": {KEY_ATTRIBUTE: {"S": "after-the-update"}},
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the table the completed update published to take an item \
             write, got {status} with {written}"
        );
        // ...and the next control-plane change of it, which is what a table left UPDATING
        // refuses with `ResourceInUseException`.
        let (status, second) = dynamodb(
            &router,
            "UpdateTable",
            &json!({"TableName": updated_name, "DeletionProtectionEnabled": true}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a further UpdateTable on the settled table to be served, \
             got {status} with {second}"
        );
    }
}
