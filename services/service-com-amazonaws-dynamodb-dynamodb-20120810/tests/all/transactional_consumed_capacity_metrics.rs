// SPDX-License-Identifier: CC0-1.0

//! The consumption a request REPORTS and the consumption CloudWatch CARRIES for it are the
//! same figure, whichever surface the request came in through.
//!
//! The rule under test: every request a table serves counts the capacity units it reports as
//! its `ConsumedCapacity` into that table's `AWS/DynamoDB` consumption series for the half of
//! the throughput it spent — so the `Sum` over a window equals the units the responses in that
//! window named. The transactional surfaces are part of that rule and not an exception to it:
//! "transactional write requests require two write capacity units", so a transaction counts
//! twice what the same single-item write counts, and a repeat under a client request token
//! applies nothing and counts the READ capacity it reports instead.
//!
//! The catalog spans what the rule generalises over — which API the write was made with (the
//! item, batched, transactional and statement surfaces), which kind of write it was (a put, an
//! update, a delete), how many writes one request carries and how large the item is, so the
//! unit boundary falls on either side — rather than one operation against one table.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::emulator::{call, describe_when_active, emulator_with_cloudwatch, ok, ok_cloudwatch};

/// The namespace Amazon DynamoDB publishes a table's own metrics under.
const NAMESPACE: &str = "AWS/DynamoDB";

/// The metric that counts write capacity units.
const CONSUMED_WRITE: &str = "ConsumedWriteCapacityUnits";

/// The metric that counts read capacity units.
const CONSUMED_READ: &str = "ConsumedReadCapacityUnits";

/// The metric that reports how long a served request took.
const LATENCY: &str = "SuccessfulRequestLatency";

/// The partition key value the rows that update or delete address.
const SEEDED_KEY: &str = "seed";

/// The largest difference two capacity figures may show and still be the same figure: both
/// sides are sums of whole and half units, so anything above rounding is a real divergence.
const TOLERANCE: f64 = 1e-9;

/// One row of the catalog: a surface, and the requests this suite makes of it.
struct SurfaceRow {
    case: &'static str,
    /// The API served.
    operation: &'static str,
    /// The request bodies sent, one per call.
    bodies: fn(&str) -> Vec<Value>,
}

/// The catalog: the item API's three write verbs, the batched surface, the transactional
/// surface over a put, an update, a delete and a two-element transaction, and the three
/// statement surfaces — over items on both sides of the one-kilobyte capacity unit.
const SURFACE_ROWS: &[SurfaceRow] = &[
    SurfaceRow {
        case: "a single-item put of a small item",
        operation: "PutItem",
        bodies: |table| vec![json!({"TableName": table, "Item": item("p0", 0)})],
    },
    SurfaceRow {
        case: "a single-item put of a multi-kilobyte item",
        operation: "PutItem",
        bodies: |table| vec![json!({"TableName": table, "Item": item("p1", 3_000)})],
    },
    SurfaceRow {
        case: "three single-item puts of growing items",
        operation: "PutItem",
        bodies: |table| {
            [0_usize, 900, 2_500]
                .into_iter()
                .enumerate()
                .map(|(index, padding)| {
                    json!({"TableName": table, "Item": item(&format!("p{index}"), padding)})
                })
                .collect()
        },
    },
    SurfaceRow {
        case: "a single-item update",
        operation: "UpdateItem",
        bodies: |table| {
            vec![json!({
                "TableName": table,
                "Key": {"pk": {"S": SEEDED_KEY}},
                "UpdateExpression": "SET n = :n",
                "ExpressionAttributeValues": {":n": {"N": "1"}},
            })]
        },
    },
    SurfaceRow {
        case: "a single-item delete",
        operation: "DeleteItem",
        bodies: |table| vec![json!({"TableName": table, "Key": {"pk": {"S": SEEDED_KEY}}})],
    },
    SurfaceRow {
        case: "a batched write of two puts",
        operation: "BatchWriteItem",
        bodies: |table| {
            vec![json!({
                "RequestItems": {
                    table: [
                        {"PutRequest": {"Item": item("b0", 0)}},
                        {"PutRequest": {"Item": item("b1", 2_000)}},
                    ],
                },
            })]
        },
    },
    SurfaceRow {
        case: "a transactional put of a small item",
        operation: "TransactWriteItems",
        bodies: |table| {
            vec![json!({
                "TransactItems": [{"Put": {"TableName": table, "Item": item("t0", 0)}}],
            })]
        },
    },
    SurfaceRow {
        case: "a transactional put of a multi-kilobyte item",
        operation: "TransactWriteItems",
        bodies: |table| {
            vec![json!({
                "TransactItems": [{"Put": {"TableName": table, "Item": item("t1", 3_000)}}],
            })]
        },
    },
    SurfaceRow {
        case: "a transaction of two puts into one table",
        operation: "TransactWriteItems",
        bodies: |table| {
            vec![json!({
                "TransactItems": [
                    {"Put": {"TableName": table, "Item": item("t2", 0)}},
                    {"Put": {"TableName": table, "Item": item("t3", 1_500)}},
                ],
            })]
        },
    },
    SurfaceRow {
        case: "three transactional puts, one call each",
        operation: "TransactWriteItems",
        bodies: |table| {
            (0..3)
                .map(|index| {
                    json!({
                        "TransactItems": [{
                            "Put": {
                                "TableName": table,
                                "Item": item(&format!("t{index}"), index * 900),
                            },
                        }],
                    })
                })
                .collect()
        },
    },
    SurfaceRow {
        case: "a transactional update",
        operation: "TransactWriteItems",
        bodies: |table| {
            vec![json!({
                "TransactItems": [{
                    "Update": {
                        "TableName": table,
                        "Key": {"pk": {"S": SEEDED_KEY}},
                        "UpdateExpression": "SET n = :n",
                        "ExpressionAttributeValues": {":n": {"N": "2"}},
                    },
                }],
            })]
        },
    },
    SurfaceRow {
        case: "a transactional delete",
        operation: "TransactWriteItems",
        bodies: |table| {
            vec![json!({
                "TransactItems": [{
                    "Delete": {"TableName": table, "Key": {"pk": {"S": SEEDED_KEY}}},
                }],
            })]
        },
    },
    SurfaceRow {
        case: "a statement surface insert",
        operation: "ExecuteStatement",
        bodies: |table| {
            vec![json!({"Statement": insert(table, "s0")})]
        },
    },
    SurfaceRow {
        case: "a transactional statement insert",
        operation: "ExecuteTransaction",
        bodies: |table| {
            vec![json!({
                "TransactStatements": [
                    {"Statement": insert(table, "x0")},
                    {"Statement": insert(table, "x1")},
                ],
            })]
        },
    },
    SurfaceRow {
        case: "a batched statement insert",
        operation: "BatchExecuteStatement",
        bodies: |table| {
            vec![json!({
                "Statements": [
                    {"Statement": insert(table, "y0")},
                    {"Statement": insert(table, "y1")},
                ],
            })]
        },
    },
];

#[tokio::test]
async fn every_write_surface_counts_what_it_reported_into_the_table_series() {
    for row in SURFACE_ROWS {
        let case = row.case;
        let router = emulator_with_cloudwatch();
        let table = "reported";
        create_table(&router, table).await;
        seed_item(&router, table).await;

        let before = series_sum(&router, table, CONSUMED_WRITE).await;
        let read_before = series_sum(&router, table, CONSUMED_READ).await;
        let mut reported = 0.0_f64;
        for mut body in (row.bodies)(table) {
            body["ReturnConsumedCapacity"] = json!("TOTAL");
            let (status, answered) = call(&router, row.operation, body.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {} {body} to answer 200, got {status} {answered:?}",
                row.operation
            );
            reported += reported_units(&answered);
        }
        assert!(
            reported > 0.0,
            "case {case:?}: expected {} to report the capacity it consumed, got {reported}",
            row.operation
        );

        let after = series_sum(&router, table, CONSUMED_WRITE).await;
        assert!(
            (after - before - reported).abs() < TOLERANCE,
            "case {case:?}: expected the {CONSUMED_WRITE} series of {table:?} to gain the \
             {reported} units the responses reported, from {before} to {}, got {after}",
            before + reported
        );
        // A write spends no read capacity, so the other half of the throughput does not move.
        let read_after = series_sum(&router, table, CONSUMED_READ).await;
        assert!(
            (read_after - read_before).abs() < TOLERANCE,
            "case {case:?}: expected a write to spend no read capacity, which moved from \
             {read_before} to {read_after}"
        );
    }
}

/// The transactional cost, read out of the series rather than out of the response: twin
/// tables written with the SAME items, one through the item API and one through a
/// single-element transaction, so the series of the transactional table carries exactly twice
/// what the other one does — and each table's latency samples are booked under the one
/// operation it was written with.
#[tokio::test]
async fn a_transaction_counts_twice_what_the_same_single_item_write_counts() {
    let router = emulator_with_cloudwatch();
    let plain = "plain";
    let transactional = "transactional";
    for table in [plain, transactional] {
        create_table(&router, table).await;
    }

    // Three item sizes, so the figure doubled is not one number: either side of the
    // one-kilobyte write capacity unit, and several kilobytes.
    let paddings = [0_usize, 900, 2_500];
    let case = "twin tables written with the same items";
    for (index, padding) in paddings.into_iter().enumerate() {
        let written = item(&format!("w{index}"), padding);
        let (status, answered) = call(
            &router,
            "PutItem",
            json!({
                "TableName": plain,
                "Item": written.clone(),
                "ReturnConsumedCapacity": "TOTAL",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutItem to answer 200, got {status} {answered:?}"
        );
        let plain_units = reported_units(&answered);
        let (status, answered) = call(
            &router,
            "TransactWriteItems",
            json!({
                "TransactItems": [{"Put": {"TableName": transactional, "Item": written}}],
                "ReturnConsumedCapacity": "TOTAL",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected TransactWriteItems to answer 200, got {status} {answered:?}"
        );
        let transactional_units = reported_units(&answered);
        assert!(
            (transactional_units - 2.0 * plain_units).abs() < TOLERANCE,
            "case {case:?}: expected the transactional write of a {padding}-byte payload to \
             report twice the {plain_units} units the same put reported, got \
             {transactional_units}"
        );
    }

    let plain_sum = series_sum(&router, plain, CONSUMED_WRITE).await;
    let transactional_sum = series_sum(&router, transactional, CONSUMED_WRITE).await;
    assert!(
        plain_sum > 0.0,
        "case {case:?}: expected the {CONSUMED_WRITE} series of {plain:?} to carry the puts' \
         units, got {plain_sum}"
    );
    assert!(
        (transactional_sum - 2.0 * plain_sum).abs() < TOLERANCE,
        "case {case:?}: expected the {CONSUMED_WRITE} series of {transactional:?} to carry \
         twice the {plain_sum} units of {plain:?}, got {transactional_sum}"
    );

    // Each table's requests are booked under the operation they were made with, and the table
    // written with the other operation has no series for it at all.
    for (table, served, unserved) in [
        (plain, "PutItem", "TransactWriteItems"),
        (transactional, "TransactWriteItems", "PutItem"),
    ] {
        let booked = latency_samples(&router, table, served).await;
        assert!(
            (booked - paddings.len() as f64).abs() < TOLERANCE,
            "case {case:?}: expected {table:?} to book {} {served} samples, got {booked}",
            paddings.len()
        );
        let absent = latency_datapoints(&router, table, unserved).await;
        assert!(
            absent.is_empty(),
            "case {case:?}: expected no {unserved} series on {table:?}, got {absent:?}"
        );
    }
}

/// A repeat under the same client request token applies nothing, and is billed as the read it
/// was: the read series gains the figure it reports and the write series stays where the first
/// call left it.
#[tokio::test]
async fn a_repeat_under_a_client_request_token_counts_the_read_it_reports() {
    let router = emulator_with_cloudwatch();
    let table = "idempotent";
    create_table(&router, table).await;
    let case = "a repeat under the same client request token";

    let request = json!({
        "TransactItems": [{"Put": {"TableName": table, "Item": item("r0", 1_500)}}],
        "ClientRequestToken": "token-of-this-transaction",
        "ReturnConsumedCapacity": "TOTAL",
    });
    let applied = ok(&router, "TransactWriteItems", request.clone()).await;
    let written = series_sum(&router, table, CONSUMED_WRITE).await;
    assert!(
        (written - reported_units(&applied)).abs() < TOLERANCE,
        "case {case:?}: expected the first call to count the {} units it reported, got {written}",
        reported_units(&applied)
    );
    let read_before = series_sum(&router, table, CONSUMED_READ).await;

    let repeated = ok(&router, "TransactWriteItems", request).await;
    let repeated_units = reported_units(&repeated);
    assert!(
        repeated_units > 0.0,
        "case {case:?}: expected the repeat to report the read capacity it consumed, got \
         {repeated_units} in {repeated:?}"
    );
    let read_after = series_sum(&router, table, CONSUMED_READ).await;
    assert!(
        (read_after - read_before - repeated_units).abs() < TOLERANCE,
        "case {case:?}: expected the {CONSUMED_READ} series to gain the {repeated_units} units \
         the repeat reported, from {read_before}, got {read_after}"
    );
    let write_after = series_sum(&router, table, CONSUMED_WRITE).await;
    assert!(
        (write_after - written).abs() < TOLERANCE,
        "case {case:?}: expected the repeat to apply nothing and so spend no write capacity, \
         which moved from {written} to {write_after}"
    );
}

/// One item under `key`, padded to put its write capacity on either side of a unit boundary:
/// `padding` bytes of payload beside the key.
fn item(key: &str, padding: usize) -> Value {
    let mut written = json!({"pk": {"S": key}});
    if padding > 0 {
        written["payload"] = json!({"S": "x".repeat(padding)});
    }
    written
}

/// A PartiQL insert of one small item under `key`, for the statement surfaces.
fn insert(table: &str, key: &str) -> String {
    format!("INSERT INTO \"{table}\" VALUE {{'pk': '{key}'}}")
}

/// Every `CapacityUnits` figure a response carries, summed: with `ReturnConsumedCapacity` of
/// `TOTAL` each entry carries one aggregate for the table it is about
/// (`ConsumedCapacity$CapacityUnits`), and these suites address one table per request, so this
/// is what the request reported consuming.
fn reported_units(answered: &Value) -> f64 {
    match answered {
        Value::Object(members) => members
            .iter()
            .map(|(name, value)| {
                if name == "CapacityUnits" {
                    value.as_f64().unwrap_or_default()
                } else {
                    reported_units(value)
                }
            })
            .sum(),
        Value::Array(entries) => entries.iter().map(reported_units).sum(),
        _ => 0.0,
    }
}

/// Create one on-demand table with a single-attribute key, ready to serve.
async fn create_table(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    // A table is served once it is ACTIVE: the waiter loop a client runs after a create.
    describe_when_active(router, name, name).await;
}

/// Write the item the rows that update or delete address. It lands BEFORE the sums a row
/// asserts are read, so what it consumed is outside the delta under test.
async fn seed_item(router: &MultiServiceRouter, table: &str) {
    ok(
        router,
        "PutItem",
        json!({"TableName": table, "Item": {"pk": {"S": SEEDED_KEY}, "n": {"N": "0"}}}),
    )
    .await;
}

/// The `Sum` CloudWatch reports for one of a table's throughput series, over a window wide
/// enough to hold everything this suite published.
async fn series_sum(router: &MultiServiceRouter, table: &str, metric: &str) -> f64 {
    datapoints(
        router,
        metric,
        json!([{"Name": "TableName", "Value": table}]),
        "Sum",
    )
    .await
    .iter()
    .filter_map(|point| point["Sum"].as_f64())
    .sum()
}

/// How many latency samples the `{TableName, Operation}` series holds.
async fn latency_samples(router: &MultiServiceRouter, table: &str, operation: &str) -> f64 {
    latency_datapoints(router, table, operation)
        .await
        .iter()
        .filter_map(|point| point["SampleCount"].as_f64())
        .sum()
}

/// Every datapoint of the latency series of `operation` on `table`.
async fn latency_datapoints(
    router: &MultiServiceRouter,
    table: &str,
    operation: &str,
) -> Vec<Value> {
    datapoints(
        router,
        LATENCY,
        json!([
            {"Name": "TableName", "Value": table},
            {"Name": "Operation", "Value": operation},
        ]),
        "SampleCount",
    )
    .await
}

/// Every datapoint CloudWatch reports for `metric` under `dimensions`, over a window wide
/// enough to hold everything this suite published.
async fn datapoints(
    router: &MultiServiceRouter,
    metric: &str,
    dimensions: Value,
    statistic: &str,
) -> Vec<Value> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64();
    let answered = ok_cloudwatch(
        router,
        "GetMetricStatistics",
        json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": dimensions,
            "StartTime": now - 3600.0,
            "EndTime": now + 3600.0,
            "Period": 60,
            "Statistics": [statistic],
        }),
    )
    .await;
    answered["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}
