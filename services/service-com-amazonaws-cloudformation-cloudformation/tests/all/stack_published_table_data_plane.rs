// SPDX-License-Identifier: CC0-1.0

//! The hand-off a client deploying a table by template depends on: the name a stack's OUTPUT
//! carries for a declared `AWS::DynamoDB::Table` is a name the DynamoDB DATA PLANE serves.
//!
//! RULE: a stack reports `CREATE_COMPLETE` only once every declared resource has been created
//! in the service that owns it, and the physical id a declared resource reports — the value
//! `Ref` resolves to, which is what an `Output` built on it carries and what
//! `DescribeStackResources` records — is the identifier that service registered. For an
//! `AWS::DynamoDB::Table` that identifier is the table's name ("Ref returns the table name"),
//! so the item calls a client makes against a completed stack's `TableName` output are SERVED:
//! `DeleteItem` on a key holding no item is a success, and "the operation tried to access a
//! nonexistent table or index" is the refusal only a name no table answers to earns.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the table
//! or leaves CloudFormation to name it, the key schema (a partition key alone, and one with a
//! sort key beside it), and the capacity mode — and each row's template declares the table
//! beside the bucket and the function an event-driven estate carries, each published as an
//! output of its own, so the table is one resource of several rather than the only one.
//!
//! Sibling suites hold the halves this one stands on: `dynamodb_table_resources.rs` pins the
//! attributes such a table publishes, and `dynamodb_table_creation_window.rs` that the table
//! is out of its `CREATING` window when the stack reports it created. What this one adds is
//! the client's own sequence — read the output, then call the data plane with it — and the
//! refusal a name no stack published still earns. WHICH sentence each DynamoDB surface words
//! that refusal with is the DynamoDB crate's own contract
//! (`validation_message_wording.rs` there), so this suite holds only the text they share.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::dynamodb_table_resources::{describe_table, dynamodb, failure_reasons};
use crate::stack_lifecycle::{
    acknowledging_iam, call, create_stack, describe_stack, member, output_value, router, xml_blocks,
    xml_text,
};

/// The logical ids the templates below declare, which are what `DescribeStackResources`
/// records each physical id against.
const TABLE_LOGICAL_ID: &str = "LabelsTable";
const BUCKET_LOGICAL_ID: &str = "Images";
const FUNCTION_LOGICAL_ID: &str = "Handler";

/// The attribute every row's table is keyed by.
const PARTITION_KEY: &str = "image_name";

/// The sort key the row that declares one carries.
const SORT_KEY: &str = "captured_at";

/// How long a create is given to settle, and how often the suite looks again. The wait is on
/// the CONDITION — the stack leaving `CREATE_IN_PROGRESS` — so nothing here sleeps on the
/// emulator's behalf, and the deadline is far past the spans a stack of four resources owes.
const DEPLOY_DEADLINE: Duration = Duration::from_secs(120);
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// The `TableStatus` a table serves item calls in — "you can perform read and write
/// operations only on an ACTIVE table".
const ACTIVE: &str = "ACTIVE";

/// The fixed text an item call naming a table no table answers to is refused with.
const RESOURCE_NOT_FOUND_TEXT: &str = "Requested resource not found";

/// The handler the declared function carries. It is never invoked by this suite: what is
/// under test is the TABLE the same stack declares, and the function is here because the
/// estate a client deploys around such a table carries one.
const HANDLER_SOURCE: &str = "def handler(event, context):\n    return {}\n";

/// One row: the table's declaration, and the name the template states for it when it states
/// one.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The `TableName` property, or `None` for a table CloudFormation names itself.
    declared_name: Option<&'static str>,
    /// The `AWS::DynamoDB::Table` properties beside the name.
    properties: Value,
    /// Whether the key schema carries a sort key, which is what an item's key must carry.
    sorted: bool,
}

/// The row whose table CloudFormation names, keyed by a partition key alone.
fn generated_name_row() -> Row {
    Row {
        case: "a table CloudFormation names, keyed by a partition key alone",
        stack: "published-table-generated-name",
        declared_name: None,
        properties: json!({
            "AttributeDefinitions": [
                {"AttributeName": PARTITION_KEY, "AttributeType": "S"},
            ],
            "KeySchema": [{"AttributeName": PARTITION_KEY, "KeyType": "HASH"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
        sorted: false,
    }
}

/// The row whose table the template names, with a sort key and provisioned capacity.
fn declared_name_row() -> Row {
    Row {
        case: "a table the template names, with a sort key and provisioned capacity",
        stack: "published-table-declared-name",
        declared_name: Some("published-image-labels"),
        properties: json!({
            "AttributeDefinitions": [
                {"AttributeName": PARTITION_KEY, "AttributeType": "S"},
                {"AttributeName": SORT_KEY, "AttributeType": "N"},
            ],
            "KeySchema": [
                {"AttributeName": PARTITION_KEY, "KeyType": "HASH"},
                {"AttributeName": SORT_KEY, "KeyType": "RANGE"},
            ],
            "ProvisionedThroughput": {
                "ReadCapacityUnits": 3,
                "WriteCapacityUnits": 2,
            },
        }),
        sorted: true,
    }
}

/// The template one row deploys: the table it declares, the bucket the objects of such an
/// estate land in, and the function and role that process them — with the three identifiers
/// the estate publishes as outputs.
fn template(row: &Row) -> String {
    let mut table = row.properties.clone();
    if let Some(name) = row.declared_name {
        table["TableName"] = json!(name);
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            TABLE_LOGICAL_ID: {"Type": "AWS::DynamoDB::Table", "Properties": table},
            BUCKET_LOGICAL_ID: {"Type": "AWS::S3::Bucket"},
            "HandlerRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": {"Service": "lambda.amazonaws.com"},
                            "Action": "sts:AssumeRole",
                        }],
                    },
                },
            },
            FUNCTION_LOGICAL_ID: {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": {"Fn::GetAtt": ["HandlerRole", "Arn"]},
                    "Timeout": 30,
                    "Environment": {"Variables": {"TABLE": {"Ref": TABLE_LOGICAL_ID}}},
                    "Code": {"ZipFile": HANDLER_SOURCE},
                },
            },
        },
        "Outputs": {
            "BucketName": {"Value": {"Ref": BUCKET_LOGICAL_ID}},
            "TableName": {"Value": {"Ref": TABLE_LOGICAL_ID}},
            "FunctionName": {"Value": {"Ref": FUNCTION_LOGICAL_ID}},
        },
    })
    .to_string()
}

/// The `DescribeStacks` body at the first poll that reports the stack out of
/// `CREATE_IN_PROGRESS`.
async fn described_once_settled(case: &str, router: &MultiServiceRouter, stack: &str) -> String {
    let deadline = Instant::now() + DEPLOY_DEADLINE;
    loop {
        let (status, described) = describe_stack(router, stack).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the stack to be readable, got {status} {described}"
        );
        if xml_text(&described, "StackStatus").as_deref() != Some("CREATE_IN_PROGRESS") {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the create to settle, last read {described}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The physical id `DescribeStackResources` records for one logical resource of a stack.
async fn physical_id(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> Option<String> {
    let (status, body) = call(
        router,
        "DescribeStackResources",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeStackResources to answer, got {status} {body}"
    );
    xml_blocks(&body, "member")
        .into_iter()
        .find(|resource| xml_text(resource, "LogicalResourceId").as_deref() == Some(logical_id))
        .and_then(|resource| xml_text(&resource, "PhysicalResourceId"))
}

/// The key of one item of a row's table: the partition key, with the sort key beside it where
/// the declaration carries one.
fn item_key(row: &Row, name: &str) -> Value {
    if row.sorted {
        return json!({PARTITION_KEY: {"S": name}, SORT_KEY: {"N": "1761465600"}});
    }
    json!({PARTITION_KEY: {"S": name}})
}

/// The error code an answer carries (`__type`, as an AWS JSON answer names the error shape).
fn error_code(answered: &Value) -> String {
    answered["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Deploy one row's estate and answer the table name it published, having asserted that the
/// stack completed, that every identifier it publishes travels back, and that the published
/// name is the physical id the stack recorded for the declared table.
async fn published_table_name(router: &MultiServiceRouter, row: &Row) -> String {
    let case = row.case;
    let (status, body) =
        create_stack(router, row.stack, &template(row), &acknowledging_iam(&[])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateStack to be accepted, got {status} {body}"
    );

    let described = described_once_settled(case, router, row.stack).await;
    let stack_status = xml_text(&described, "StackStatus");
    assert_eq!(
        stack_status.as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {stack_status:?} because {:?}",
        failure_reasons(router, row.stack).await
    );

    // Every identifier the estate publishes travels back as a non-empty output, which is what
    // a client reads to address the resources it just deployed.
    for output in ["BucketName", "TableName", "FunctionName"] {
        let value = output_value(&described, output).unwrap_or_default();
        assert!(
            !value.is_empty(),
            "case {case:?}: expected the stack to publish {output}, got {value:?} in {described}"
        );
    }
    let name = output_value(&described, "TableName").unwrap_or_default();
    if let Some(declared) = row.declared_name {
        assert_eq!(
            name, declared,
            "case {case:?}: expected the output to carry the name the template declared"
        );
    }

    // The output IS the physical id the stack recorded for the resource, which is the name the
    // owning service registered.
    let recorded = physical_id(router, case, row.stack, TABLE_LOGICAL_ID).await;
    assert_eq!(
        recorded.as_deref(),
        Some(name.as_str()),
        "case {case:?}: expected the recorded physical id to be the published name {name:?}, \
         got {recorded:?}"
    );
    name
}

/// What DynamoDB itself holds for the published name: a table of that name, reported ACTIVE
/// and listed among the Region's tables.
async fn dynamodb_holds_the_table(router: &MultiServiceRouter, row: &Row, name: &str) {
    let case = row.case;
    let table = describe_table(router, name).await.unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected DynamoDB to hold the table {name:?} the stack published, \
             got no table at all"
        )
    });
    assert_eq!(
        table["TableName"],
        json!(name),
        "case {case:?}: expected DescribeTable to answer for the published name, got {table}"
    );
    assert_eq!(
        table["TableStatus"],
        json!(ACTIVE),
        "case {case:?}: expected the published table to be {ACTIVE}, got {table}"
    );
    let (status, listed) = dynamodb(router, "ListTables", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListTables to answer, got {status} {listed}"
    );
    let names: Vec<String> = listed["TableNames"]
        .as_array()
        .map(|held| {
            held.iter()
                .map(|held| held.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        names.iter().any(|held| held == name),
        "case {case:?}: expected the Region's tables to name {name:?}, got {names:?}"
    );
}

/// The item calls the stack published the table for, all SERVED: a delete of a key no item
/// sits under is a success, and a write is read back and removable.
async fn the_data_plane_serves_the_published_name(
    router: &MultiServiceRouter,
    row: &Row,
    name: &str,
) {
    let case = row.case;
    let absent = item_key(row, "never-written.jpg");
    let (status, deleted) = dynamodb(
        router,
        "DeleteItem",
        &json!({"TableName": name, "Key": absent}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected a delete of a key holding no item to be served, got {status} \
         {deleted}"
    );
    assert_eq!(
        deleted.get("Attributes"),
        None,
        "case {case:?}: a delete that removed nothing reports no attributes, got {deleted}"
    );

    let key = item_key(row, "alpha.jpg");
    let mut item = key.clone();
    item["labels"] = json!({"L": [{"S": case}]});
    let (status, written) =
        dynamodb(router, "PutItem", &json!({"TableName": name, "Item": item})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the write to be served, got {status} {written}"
    );
    let (status, read) = dynamodb(
        router,
        "GetItem",
        &json!({"TableName": name, "Key": key.clone(), "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the read to be served, got {status} {read}"
    );
    assert_eq!(
        read["Item"]["labels"],
        json!({"L": [{"S": case}]}),
        "case {case:?}: expected the item just written, got {read}"
    );
    let (status, removed) = dynamodb(
        router,
        "DeleteItem",
        &json!({"TableName": name, "Key": key.clone()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the delete to be served, got {status} {removed}"
    );
    let (status, gone) = dynamodb(
        router,
        "GetItem",
        &json!({"TableName": name, "Key": key, "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        (status, gone.get("Item")),
        (200, None),
        "case {case:?}: expected the deleted key to hold no item, got {status} {gone}"
    );
}

/// The other half of the rule: the refusal belongs to a name NO stack published.
async fn an_unpublished_name_is_refused(router: &MultiServiceRouter, row: &Row, name: &str) {
    let case = row.case;
    let unpublished = format!("{name}-never-declared");
    for operation in ["DeleteItem", "GetItem"] {
        let (status, refused) = dynamodb(
            router,
            operation,
            &json!({"TableName": &unpublished, "Key": item_key(row, "alpha.jpg")}),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected {operation} on {unpublished:?} to be refused as a missing \
             table, got {status} {refused}"
        );
        let message = refused["message"]
            .as_str()
            .or(refused["Message"].as_str())
            .unwrap_or_default();
        assert!(
            message.starts_with(RESOURCE_NOT_FOUND_TEXT),
            "case {case:?}: expected the missing-table refusal's own text, got {refused}"
        );
    }
}

/// One row, end to end. Each phase is awaited through its own heap-allocated future, so the
/// test's own frame stays small whatever the request paths beneath it cost.
async fn a_published_table_serves_its_item_calls(row: &Row) {
    let router = router();
    let name = Box::pin(published_table_name(&router, row)).await;
    Box::pin(dynamodb_holds_the_table(&router, row, &name)).await;
    Box::pin(the_data_plane_serves_the_published_name(&router, row, &name)).await;
    Box::pin(an_unpublished_name_is_refused(&router, row, &name)).await;
}

#[tokio::test]
async fn a_table_cloudformation_named_is_served_under_the_name_the_stack_published() {
    Box::pin(a_published_table_serves_its_item_calls(&generated_name_row())).await;
}

#[tokio::test]
async fn a_table_the_template_named_is_served_under_the_name_the_stack_published() {
    Box::pin(a_published_table_serves_its_item_calls(&declared_name_row())).await;
}
