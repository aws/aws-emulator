// SPDX-License-Identifier: CC0-1.0

//! The RESOURCE interface of the AWS SDK a running Python handler imports, end to end over
//! the wire: Lambda and Amazon DynamoDB on ONE engine, a `python3.x` function whose code
//! does `boto3.resource("dynamodb").Table(...)`, and the table read back where a client
//! reads it.
//!
//! The rule pinned here: a managed `python3.x` runtime ships the whole AWS SDK for Python,
//! so a handler can reach a service through its RESOURCE interface as well as its client
//! one — `boto3.resource("dynamodb").Table(name)` carries the table name for the call,
//! every item member it is handed is a plain Python value marshalled to the attribute value
//! that operation's own model carries (a number as text, a blob as the base64 an AWS JSON
//! protocol writes it as, a set, a map and a list as their own types) and read back the same
//! way, and a `Key`/`Attr` condition is built into the expression string the service reads
//! with the placeholders it uses. What the call answers is the service's own answer.
//!
//! The rows of the first table differ along what the rule generalises over: the TYPE and
//! shape of the item value written through the resource — text, a whole number, a fraction,
//! a number far outside what a double holds, a boolean, an absent value, bytes, a list, a
//! nested map, and a set of each scalar type. Each row asserts both halves: the attribute
//! value the TABLE holds, read through DynamoDB's own client API where a client reads it,
//! and the Python value the handler read back through the resource — so a marshalling that
//! writes the wrong wire form, or reads back the wrong Python type, fails here.
//!
//! The further tests walk the rest of the interface: the conditions `Key`/`Attr` build
//! (a key condition, a filter, a refused conditional write), the batch writer's flushing,
//! an update's returned attributes, a delete's old ones, and the table's own attributes as
//! `DescribeTable` reports them.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::synchronous_invoke_run::package;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The `X-Amz-Target` prefix of the DynamoDB service (its `OPERATIONS.md`, "Wire protocol").
const DYNAMODB_TARGET_PREFIX: &str = "DynamoDB_20120810";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module and attribute every package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// The runtime every function below is created with.
const RUNTIME: &str = "python3.12";

/// How long one invocation is given, as the function's own `Timeout`: well past what a run
/// needs, because what is under test is the SDK's interface and not how fast an interpreter
/// gets through a handler.
const FUNCTION_TIMEOUT: u64 = 30;

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// How long a wait for a table to settle is given before the row is a failure.
const BUDGET: Duration = Duration::from_secs(30);

/// How long between two readings of a resource that is settling.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The table's partition and sort key attributes: a sort key is taken so a key condition
/// has a sort key to compare, which is what `Key(...).begins_with(...)` builds.
const PARTITION_KEY: &str = "pk";
/// See [`PARTITION_KEY`].
const SORT_KEY: &str = "sk";

/// Lambda and Amazon DynamoDB over ONE shared engine, as `core/router/src/lib.rs` mounts
/// them: the handler's SDK call is dispatched back into this same router, so the table it
/// writes is the table this suite reads.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One DynamoDB call (`awsJson1_0`, `DynamoDB_20120810.<Operation>`) that must answer 200.
///
/// This is the CLIENT interface: its items are the wire's own attribute values, which is
/// what makes it the reader every row's expected wire form is asserted through.
async fn dynamodb(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("dynamodb.{REGION}.amazonaws.com"))
        .header("authorization", authorization("dynamodb"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{DYNAMODB_TARGET_PREFIX}.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DynamoDB {operation}, got {status} ({parsed})"
    );
    parsed
}

/// One Lambda `restJson1` call, as its status, headers and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// The table every handler below writes, waited out to `ACTIVE` — "you can perform read and
/// write operations only on an ACTIVE table".
async fn given_table(router: &MultiServiceRouter, case: &str, name: &str) {
    dynamodb(
        router,
        case,
        "CreateTable",
        &json!({
            "TableName": name,
            "KeySchema": [
                { "AttributeName": PARTITION_KEY, "KeyType": "HASH" },
                { "AttributeName": SORT_KEY, "KeyType": "RANGE" },
            ],
            "AttributeDefinitions": [
                { "AttributeName": PARTITION_KEY, "AttributeType": "S" },
                { "AttributeName": SORT_KEY, "AttributeType": "S" },
            ],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    let deadline = Instant::now() + BUDGET;
    loop {
        let described = dynamodb(router, case, "DescribeTable", &json!({ "TableName": name })).await;
        if described["Table"]["TableStatus"] == json!("ACTIVE") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the table never became ACTIVE: {described}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Create a function running [`HANDLER_SOURCE`] against `table` and wait out the
/// provisioning window.
async fn given_function(router: &MultiServiceRouter, case: &str, name: &str, table: &str) {
    let (status, _headers, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT,
            "Environment": { "Variables": { "TABLE": table } },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// `Invoke` the function synchronously with `event`, asserting it ran to completion and
/// answering the payload the run produced.
async fn invoke(router: &MultiServiceRouter, case: &str, name: &str, event: &Value) -> Value {
    let (status, headers, payload) = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        event.to_string().as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Invoke, got {status} ({payload})"
    );
    let signal = headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    assert_eq!(
        signal, None,
        "case {case:?}: the handler must run to completion, got the function error \
         {signal:?} with {payload}"
    );
    payload
}

/// The handler every test below runs: it does the one piece of work the event names
/// through the DynamoDB RESOURCE interface, and reports what it read back.
///
/// Every value it writes is a plain Python one and every value it reports is what the
/// resource handed back, so the report is evidence of the marshalling in both directions.
const HANDLER_SOURCE: &str = r##"
import base64
import os
from decimal import Decimal

import boto3
import botocore.exceptions
from boto3.dynamodb.conditions import Attr, Key
from boto3.dynamodb.types import Binary

TABLE_NAME = os.environ["TABLE"]
RESOURCE = boto3.resource("dynamodb")
TABLE = RESOURCE.Table(TABLE_NAME)

# One value of each type the resource interface carries, named by the event. A number is a
# whole number or a Decimal, binary is bytes, and a set is a Python set.
VALUES = {
    "text": "a value",
    "whole_number": 7,
    "fraction": Decimal("3.5"),
    "very_large_number": Decimal("-12345678901234567890123"),
    "boolean": True,
    "absent": None,
    "binary": b"\x00\x01\xfe",
    "list": ["a", 1, Decimal("2.5")],
    "map": {"inner": 1, "deep": {"flag": False}},
    "text_set": {"b", "a"},
    "number_set": {1, Decimal("2.5")},
    "binary_set": {b"\x01", b"\x02"},
}


def report(value):
    """What a value read back IS: its Python type, and its text."""
    if isinstance(value, Binary):
        return {"type": "Binary", "value": base64.b64encode(bytes(value)).decode("ascii")}
    if isinstance(value, bool):
        return {"type": "bool", "value": value}
    if value is None:
        return {"type": "null", "value": None}
    if isinstance(value, set):
        return {"type": "set", "value": sorted(report(member)["value"] for member in value)}
    if isinstance(value, list):
        return {"type": "list", "value": [report(member) for member in value]}
    if isinstance(value, dict):
        return {"type": "map", "value": {key: report(held) for key, held in value.items()}}
    return {"type": type(value).__name__, "value": str(value)}


def round_trip(event):
    key = event["key"]
    TABLE.put_item(Item={"pk": key, "sk": "only", "attr": VALUES[event["kind"]]})
    item = TABLE.get_item(Key={"pk": key, "sk": "only"})["Item"]
    return {
        "ok": True,
        "attr": report(item["attr"]),
        "pk": report(item["pk"]),
        "members": sorted(item),
    }


def query(event):
    for index, sort_key in enumerate(event["sort_keys"]):
        TABLE.put_item(Item={"pk": event["key"], "sk": sort_key, "rank": index})
    answered = TABLE.query(
        KeyConditionExpression=Key("pk").eq(event["key"])
        & Key("sk").begins_with(event["prefix"]),
    )
    return {
        "ok": True,
        "found": [item["sk"] for item in answered["Items"]],
        "ranks": [report(item["rank"]) for item in answered["Items"]],
        "count": int(answered["Count"]),
    }


def scan(event):
    for sort_key, value in event["items"]:
        TABLE.put_item(Item={"pk": event["key"], "sk": sort_key, "attr": value})
    answered = TABLE.scan(FilterExpression=Attr("attr").eq(event["match"]))
    return {"ok": True, "found": sorted(item["sk"] for item in answered["Items"])}


def conditional(event):
    key = event["key"]
    TABLE.put_item(Item={"pk": key, "sk": "only", "attr": "first"})
    try:
        TABLE.put_item(
            Item={"pk": key, "sk": "only", "attr": "second"},
            ConditionExpression=Attr("pk").not_exists(),
        )
    except botocore.exceptions.ClientError as error:
        held = TABLE.get_item(Key={"pk": key, "sk": "only"})["Item"]
        return {
            "ok": False,
            "code": error.response["Error"]["Code"],
            "sentence": str(error),
            "held": report(held["attr"]),
        }
    return {"ok": True}


def batch(event):
    with TABLE.batch_writer() as writer:
        for index in range(event["count"]):
            writer.put_item(
                Item={"pk": event["key"], "sk": "item-%03d" % index, "rank": index}
            )
    return {"ok": True, "written": event["count"]}


def update(event):
    key = event["key"]
    TABLE.put_item(Item={"pk": key, "sk": "only", "attr": 1})
    answered = TABLE.update_item(
        Key={"pk": key, "sk": "only"},
        UpdateExpression="SET #a = :v",
        ExpressionAttributeNames={"#a": "attr"},
        ExpressionAttributeValues={":v": Decimal("9.5")},
        ReturnValues="ALL_NEW",
    )
    return {
        "ok": True,
        "attributes": {
            name: report(held) for name, held in answered["Attributes"].items()
        },
    }


def remove(event):
    key = event["key"]
    TABLE.put_item(Item={"pk": key, "sk": "only", "attr": "doomed"})
    answered = TABLE.delete_item(Key={"pk": key, "sk": "only"}, ReturnValues="ALL_OLD")
    after = TABLE.get_item(Key={"pk": key, "sk": "only"})
    return {
        "ok": True,
        "old": report(answered["Attributes"]["attr"]),
        "found_after": "Item" in after,
    }


def describe(event):
    return {
        "ok": True,
        "table_name": TABLE.table_name,
        "table_status": TABLE.table_status,
        "table_arn": TABLE.table_arn,
        "key_schema": TABLE.key_schema,
        "named": [table.name for table in RESOURCE.tables.all()],
    }


ACTIONS = {
    "round_trip": round_trip,
    "query": query,
    "scan": scan,
    "conditional": conditional,
    "batch": batch,
    "update": update,
    "remove": remove,
    "describe": describe,
}


def lambda_handler(event, context):
    return ACTIONS[event["action"]](event)
"##;

/// A set is unordered, so a set attribute value's members are compared as a SET: every
/// `SS`/`NS`/`BS` list is sorted before the two are compared, and nothing else is touched.
fn normalized(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(name, held)| {
                    let mut normalised = normalized(held);
                    if matches!(name.as_str(), "SS" | "NS" | "BS") {
                        if let Some(members) = normalised.as_array_mut() {
                            members.sort_by_key(ToString::to_string);
                        }
                    }
                    (name.clone(), normalised)
                })
                .collect(),
        ),
        Value::Array(members) => Value::Array(members.iter().map(normalized).collect()),
        other => other.clone(),
    }
}

/// One row of the marshalling table: a plain Python value written through the resource, the
/// attribute value the table must hold for it, and what reading it back must answer.
struct Row {
    case: &'static str,
    /// The value the handler writes, by the name its own catalogue holds it under.
    kind: &'static str,
    /// The attribute value `GetItem` must report for it, through the CLIENT interface.
    wire: fn() -> Value,
    /// The Python type and text the resource must hand the handler back.
    read_back: fn() -> Value,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "text: a string attribute",
            kind: "text",
            wire: || json!({ "S": "a value" }),
            read_back: || json!({ "type": "str", "value": "a value" }),
        },
        Row {
            // A number travels as TEXT and comes back as a Decimal, never a float: that is
            // what the resource interface's number type is.
            case: "a whole number: carried as text, read back as a Decimal",
            kind: "whole_number",
            wire: || json!({ "N": "7" }),
            read_back: || json!({ "type": "Decimal", "value": "7" }),
        },
        Row {
            case: "a fraction: a Decimal keeps its own digits",
            kind: "fraction",
            wire: || json!({ "N": "3.5" }),
            read_back: || json!({ "type": "Decimal", "value": "3.5" }),
        },
        Row {
            // Far outside what a double holds, so a round trip through one would lose
            // digits; the number's own text is what travels.
            case: "a number no double holds: every digit survives the round trip",
            kind: "very_large_number",
            wire: || json!({ "N": "-12345678901234567890123" }),
            read_back: || json!({ "type": "Decimal", "value": "-12345678901234567890123" }),
        },
        Row {
            case: "a boolean",
            kind: "boolean",
            wire: || json!({ "BOOL": true }),
            read_back: || json!({ "type": "bool", "value": true }),
        },
        Row {
            case: "an absent value: None is the null attribute value",
            kind: "absent",
            wire: || json!({ "NULL": true }),
            read_back: || json!({ "type": "null", "value": null }),
        },
        Row {
            // A blob travels as the base64 text an AWS JSON protocol carries it as, and is
            // read back as the SDK's own binary type.
            case: "binary: bytes out, base64 on the wire, Binary back",
            kind: "binary",
            wire: || json!({ "B": "AAH+" }),
            read_back: || json!({ "type": "Binary", "value": "AAH+" }),
        },
        Row {
            case: "a list: each element marshalled in its own right",
            kind: "list",
            wire: || json!({ "L": [{ "S": "a" }, { "N": "1" }, { "N": "2.5" }] }),
            read_back: || {
                json!({
                    "type": "list",
                    "value": [
                        { "type": "str", "value": "a" },
                        { "type": "Decimal", "value": "1" },
                        { "type": "Decimal", "value": "2.5" },
                    ],
                })
            },
        },
        Row {
            case: "a nested map: a document all the way down",
            kind: "map",
            wire: || {
                json!({
                    "M": {
                        "inner": { "N": "1" },
                        "deep": { "M": { "flag": { "BOOL": false } } },
                    },
                })
            },
            read_back: || {
                json!({
                    "type": "map",
                    "value": {
                        "inner": { "type": "Decimal", "value": "1" },
                        "deep": {
                            "type": "map",
                            "value": { "flag": { "type": "bool", "value": false } },
                        },
                    },
                })
            },
        },
        Row {
            case: "a set of strings",
            kind: "text_set",
            wire: || json!({ "SS": ["a", "b"] }),
            read_back: || json!({ "type": "set", "value": ["a", "b"] }),
        },
        Row {
            case: "a set of numbers",
            kind: "number_set",
            wire: || json!({ "NS": ["1", "2.5"] }),
            read_back: || json!({ "type": "set", "value": ["1", "2.5"] }),
        },
        Row {
            case: "a set of blobs",
            kind: "binary_set",
            wire: || json!({ "BS": ["AQ==", "Ag=="] }),
            read_back: || json!({ "type": "set", "value": ["AQ==", "Ag=="] }),
        },
    ]
}

/// RULE: an item written through the DynamoDB resource interface is marshalled from plain
/// Python values to the attribute values that operation's model carries, and read back to
/// plain Python values — asserted on both sides for one value of every type.
/// Every part of the interface, one after another: the suite runs as ONE test because the
/// handlers it invokes take the emulator's own run slots, which every suite in this target
/// shares, and a sequence of invocations is what a function serves.
#[tokio::test]
async fn the_dynamodb_resource_interface_serves_a_running_handler() {
    the_resource_interface_marshals_every_item_value_both_ways().await;
    a_key_or_attr_condition_is_built_into_the_expression_the_service_reads().await;
    the_batch_writer_sends_every_item_it_buffered().await;
    the_attributes_an_update_or_a_delete_returns_are_read_back_too().await;
    a_tables_attributes_are_the_ones_describe_table_reports().await;
    a_service_with_no_resource_classes_is_refused_rather_than_served().await;
}

async fn the_resource_interface_marshals_every_item_value_both_ways() {
    let router = router();
    let table = "resource-marshalling";
    let case = "the marshalling table";
    given_table(&router, case, table).await;
    given_function(&router, case, "resource-marshaller", table).await;

    for row in rows() {
        let case = row.case;
        let key = format!("row-{}", row.kind);
        let payload = invoke(
            &router,
            case,
            "resource-marshaller",
            &json!({ "action": "round_trip", "kind": row.kind, "key": key }),
        )
        .await;

        // What the handler read back through the RESOURCE: a plain Python value.
        assert_eq!(
            payload["attr"],
            (row.read_back)(),
            "case {case:?}: expected the resource to read back {}, got {payload}",
            (row.read_back)()
        );
        assert_eq!(
            payload["pk"],
            json!({ "type": "str", "value": key }),
            "case {case:?}: expected the key to come back as the string it was written as, \
             got {payload}"
        );
        assert_eq!(
            payload["members"],
            json!(["attr", PARTITION_KEY, SORT_KEY]),
            "case {case:?}: expected the item to carry exactly its key and its attribute, \
             got {payload}"
        );

        // And what the TABLE holds, read through the client interface where a client reads
        // it: the wire's own attribute value.
        let read = dynamodb(
            &router,
            case,
            "GetItem",
            &json!({
                "TableName": table,
                "Key": { PARTITION_KEY: { "S": key }, SORT_KEY: { "S": "only" } },
                "ConsistentRead": true,
            }),
        )
        .await;
        let held = normalized(&read["Item"]["attr"]);
        let expected = normalized(&(row.wire)());
        assert_eq!(
            held, expected,
            "case {case:?}: expected the table to hold {expected} for it, got {held} \
             (whole item {})",
            read["Item"]
        );
    }
}

/// RULE: a `Key`/`Attr` condition handed to a condition member is built into the expression
/// string the service reads, with the placeholders it uses carried beside it — so the items
/// the service selects are the ones the condition names.
async fn a_key_or_attr_condition_is_built_into_the_expression_the_service_reads() {
    let router = router();
    let table = "resource-conditions";
    let case = "conditions";
    given_table(&router, case, table).await;
    given_function(&router, case, "resource-conditions", table).await;

    // A key condition: the partition key by equality and the sort key by prefix, which is
    // the pair a query is selected by.
    let queried = invoke(
        &router,
        "a key condition over a partition key and a sort-key prefix",
        "resource-conditions",
        &json!({
            "action": "query",
            "key": "conditions",
            "sort_keys": ["west-1", "west-2", "east-1"],
            "prefix": "west",
        }),
    )
    .await;
    assert_eq!(
        queried["found"],
        json!(["west-1", "west-2"]),
        "case {:?}: expected only the sort keys under the prefix, got {queried}",
        "a key condition over a partition key and a sort-key prefix"
    );
    assert_eq!(
        queried["count"],
        json!(2),
        "case {:?}: expected the service's own Count of 2, got {queried}",
        "a key condition over a partition key and a sort-key prefix"
    );
    assert_eq!(
        queried["ranks"],
        json!([
            { "type": "Decimal", "value": "0" },
            { "type": "Decimal", "value": "1" },
        ]),
        "case {:?}: expected each selected item's own number back as a Decimal, got {queried}",
        "a key condition over a partition key and a sort-key prefix"
    );

    // A filter: an attribute no key names, compared against a plain Python value, which the
    // builder has to carry as an attribute value of its own.
    let scanned = invoke(
        &router,
        "a filter on a non-key attribute",
        "resource-conditions",
        &json!({
            "action": "scan",
            "key": "filtered",
            "items": [["one", "keep"], ["two", "drop"], ["three", "keep"]],
            "match": "keep",
        }),
    )
    .await;
    assert_eq!(
        scanned["found"],
        json!(["one", "three"]),
        "case {:?}: expected only the items the filter admits, got {scanned}",
        "a filter on a non-key attribute"
    );

    // A conditional write the service refuses: the condition is the service's to evaluate,
    // so what the handler sees is that service's own modelled refusal, and the item is
    // unchanged.
    let case = "a conditional write the service refuses";
    let refused = invoke(
        &router,
        case,
        "resource-conditions",
        &json!({ "action": "conditional", "key": "guarded" }),
    )
    .await;
    assert_eq!(
        refused["ok"],
        json!(false),
        "case {case:?}: expected the second write to be refused, got {refused}"
    );
    assert_eq!(
        refused["code"],
        json!("ConditionalCheckFailedException"),
        "case {case:?}: expected the service's own error code, got {refused}"
    );
    assert_eq!(
        refused["held"],
        json!({ "type": "str", "value": "first" }),
        "case {case:?}: expected the refused write to have changed nothing, got {refused}"
    );
    let sentence = refused["sentence"].as_str().unwrap_or_default();
    assert!(
        sentence.starts_with(
            "An error occurred (ConditionalCheckFailedException) when calling the PutItem \
             operation: "
        ),
        "case {case:?}: expected the sentence botocore renders for the refusal, got \
         {sentence:?}"
    );
}

/// RULE: the batch writer sends what it buffers through `BatchWriteItem`, flushing a full
/// batch as it fills and the rest when it is closed — so every item a handler wrote through
/// it is in the table, however many there were.
async fn the_batch_writer_sends_every_item_it_buffered() {
    let router = router();
    let table = "resource-batches";
    let case = "the batch writer";
    given_table(&router, case, table).await;
    given_function(&router, case, "resource-batches", table).await;

    // Three counts around the 25-item batch the operation takes at most: under it, exactly
    // it, and over it, so the flush on filling and the flush on closing are both exercised.
    for count in [3_usize, 25, 30] {
        let case = "the batch writer sends every item it buffered";
        let key = format!("batch-{count}");
        let written = invoke(
            &router,
            case,
            "resource-batches",
            &json!({ "action": "batch", "key": key, "count": count }),
        )
        .await;
        assert_eq!(
            written["written"],
            json!(count),
            "case {case:?} ({count} items): expected the handler to report its own count, \
             got {written}"
        );
        let queried = dynamodb(
            &router,
            case,
            "Query",
            &json!({
                "TableName": table,
                "KeyConditionExpression": "#p = :p",
                "ExpressionAttributeNames": { "#p": PARTITION_KEY },
                "ExpressionAttributeValues": { ":p": { "S": key } },
                "ConsistentRead": true,
            }),
        )
        .await;
        let held = queried["Items"].as_array().map_or(0, Vec::len);
        assert_eq!(
            held, count,
            "case {case:?} ({count} items): expected the table to hold every buffered item, \
             got {held} ({queried})"
        );
        let ranks: Vec<String> = queried["Items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|item| item["rank"]["N"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            ranks.len(),
            count,
            "case {case:?} ({count} items): every item's number must be on the wire as a \
             number, got {queried}"
        );
    }
}

/// RULE: an operation's answer is read back through the same marshalling as its input, so
/// the attributes an update or a delete returns are plain Python values too.
async fn the_attributes_an_update_or_a_delete_returns_are_read_back_too() {
    let router = router();
    let table = "resource-returned";
    let case = "returned attributes";
    given_table(&router, case, table).await;
    given_function(&router, case, "resource-returned", table).await;

    let case = "an update's ALL_NEW attributes";
    let updated = invoke(
        &router,
        case,
        "resource-returned",
        &json!({ "action": "update", "key": "updated" }),
    )
    .await;
    assert_eq!(
        updated["attributes"]["attr"],
        json!({ "type": "Decimal", "value": "9.5" }),
        "case {case:?}: expected the new value back as a Decimal, got {updated}"
    );
    assert_eq!(
        updated["attributes"][PARTITION_KEY],
        json!({ "type": "str", "value": "updated" }),
        "case {case:?}: expected the key among the returned attributes, got {updated}"
    );

    let case = "a delete's ALL_OLD attributes";
    let deleted = invoke(
        &router,
        case,
        "resource-returned",
        &json!({ "action": "remove", "key": "doomed" }),
    )
    .await;
    assert_eq!(
        deleted["old"],
        json!({ "type": "str", "value": "doomed" }),
        "case {case:?}: expected the deleted item's own value back, got {deleted}"
    );
    assert_eq!(
        deleted["found_after"],
        json!(false),
        "case {case:?}: expected the item to be gone, got {deleted}"
    );
    let read = dynamodb(
        &router,
        case,
        "GetItem",
        &json!({
            "TableName": table,
            "Key": { PARTITION_KEY: { "S": "doomed" }, SORT_KEY: { "S": "only" } },
            "ConsistentRead": true,
        }),
    )
    .await;
    assert!(
        read["Item"].is_null(),
        "case {case:?}: expected the table to hold no item for it, got {read}"
    );
}

/// RULE: a resource's own attributes are what the service reports for it — a `Table`'s are
/// `DescribeTable`'s, read when one is first asked for, and the collection of tables is
/// `ListTables`'s.
async fn a_tables_attributes_are_the_ones_describe_table_reports() {
    let router = router();
    let table = "resource-described";
    let case = "a table's own attributes";
    given_table(&router, case, table).await;
    given_function(&router, case, "resource-described", table).await;

    let described = dynamodb(&router, case, "DescribeTable", &json!({ "TableName": table })).await;
    let reported = invoke(
        &router,
        case,
        "resource-described",
        &json!({ "action": "describe" }),
    )
    .await;
    assert_eq!(
        reported["table_name"],
        json!(table),
        "case {case:?}: expected the table's own name, got {reported}"
    );
    assert_eq!(
        reported["table_status"], described["Table"]["TableStatus"],
        "case {case:?}: expected the status DescribeTable reports ({}), got {reported}",
        described["Table"]["TableStatus"]
    );
    assert_eq!(
        reported["table_arn"], described["Table"]["TableArn"],
        "case {case:?}: expected the ARN DescribeTable reports ({}), got {reported}",
        described["Table"]["TableArn"]
    );
    assert_eq!(
        reported["key_schema"], described["Table"]["KeySchema"],
        "case {case:?}: expected the key schema DescribeTable reports ({}), got {reported}",
        described["Table"]["KeySchema"]
    );
    assert_eq!(
        reported["named"],
        json!([table]),
        "case {case:?}: expected the collection to name the tables ListTables reports, \
         got {reported}"
    );
}

/// A service this runtime's SDK supplies no resource classes for is REFUSED, and the
/// refusal says which service and what to build instead — nothing is answered for an
/// interface that would serve nothing.
async fn a_service_with_no_resource_classes_is_refused_rather_than_served() {
    let router = router();
    let case = "a resource for a service this runtime supplies no resource classes for";
    let source = r#"
import boto3
import botocore.exceptions


def lambda_handler(event, context):
    try:
        boto3.resource(event["service"])
    except botocore.exceptions.BotoCoreError as error:
        return {"ok": False, "refusal": str(error)}
    return {"ok": True}
"#;
    let (status, _headers, created) = lambda(
        &router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": "resource-refused",
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;

    for service in ["ec2", "sqs"] {
        let payload = invoke(
            &router,
            case,
            "resource-refused",
            &json!({ "service": service }),
        )
        .await;
        assert_eq!(
            payload["ok"],
            json!(false),
            "case {case:?} ({service}): expected the resource to be refused, got {payload}"
        );
        let refusal = payload["refusal"].as_str().unwrap_or_default();
        assert!(
            refusal.contains(service)
                && refusal.contains("dynamodb")
                && refusal.contains("s3"),
            "case {case:?} ({service}): expected the refusal to name the service asked for \
             and the ones there are classes for, got {refusal:?}"
        );
    }
}
