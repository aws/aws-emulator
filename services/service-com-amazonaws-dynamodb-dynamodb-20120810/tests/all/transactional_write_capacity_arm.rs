// SPDX-License-Identifier: CC0-1.0

//! What a write-only transaction REPORTS it spent in the member that tells write cost from
//! read cost, and what CloudWatch CARRIES for that same transaction.
//!
//! The rule under test: a data-plane operation spends capacity of ONE kind, so every
//! `ConsumedCapacity` it reports carries that kind's own figure equal to its `CapacityUnits`
//! aggregate ("the total number of capacity units consumed by the operation") and omits the
//! other kind's — and so does every `Capacity` line of the per-table and per-index breakdown
//! `INDEXES` asks for. A transaction that only writes therefore reports `WriteCapacityUnits`
//! equal to `CapacityUnits` on the entry of each table it addressed, at twice what the same
//! single-item write reports ("Transactional write requests require two write capacity
//! units"), and that same figure is what the table's `AWS/DynamoDB`
//! `ConsumedWriteCapacityUnits` series gains: the two planes report ONE cost, so a client
//! metering transactional cost from `WriteCapacityUnits` reads what the metric carries.
//!
//! The rows span what the rule generalises over: which action the transaction carries (a put,
//! an update, a delete, a condition check), how many actions it carries, how large the item is
//! on either side of the one-kilobyte write capacity unit, how many tables it addresses,
//! whether a table fans the write out to a secondary index of its own, and which detail level
//! the request asked for (`TOTAL`, `INDEXES`).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::emulator::{call, describe_when_active, emulator_with_cloudwatch, ok, ok_cloudwatch};

/// The namespace Amazon DynamoDB publishes a table's own metrics under.
const NAMESPACE: &str = "AWS/DynamoDB";

/// The metric that counts the write capacity units a table consumed.
const CONSUMED_WRITE: &str = "ConsumedWriteCapacityUnits";

/// A table with no secondary index, in on-demand mode.
const PLAIN: &str = "plain";

/// A second such table, so a transaction can address more than one.
const SECOND: &str = "second";

/// A table that fans every write out to a global secondary index of its own.
const INDEXED: &str = "indexed";

/// That index's name.
const INDEX: &str = "ByOwner";

/// The partition key value the rows that update, delete or condition-check address.
const SEED: &str = "seed";

/// The largest difference two capacity figures may show and still be the same figure: both
/// sides are sums of whole and half units, so anything above rounding is a real divergence.
const TOLERANCE: f64 = 1e-9;

/// One row of the catalog: a transaction, the detail level it asks for, and the tables its
/// answer must account for.
struct Row {
    case: &'static str,
    /// The `ReturnConsumedCapacity` the request asks for.
    detail: &'static str,
    /// The `TransactItems` the request carries.
    actions: fn() -> Vec<Value>,
    /// The tables the transaction addresses, which is one `ConsumedCapacity` entry each.
    tables: &'static [&'static str],
    /// The tables whose entry must itemise the global secondary index the write fanned out
    /// to, for a request that asked for the breakdown.
    fanout: &'static [&'static str],
}

/// The catalog: every action a `TransactWriteItem` may carry, over one and over several
/// tables, at item sizes on either side of a capacity unit, at both detail levels.
const ROWS: &[Row] = &[
    Row {
        case: "a put of a small item, TOTAL",
        detail: "TOTAL",
        actions: || vec![put(PLAIN, "p0", 0)],
        tables: &[PLAIN],
        fanout: &[],
    },
    Row {
        case: "a put of a multi-kilobyte item, TOTAL",
        detail: "TOTAL",
        actions: || vec![put(PLAIN, "p1", 3_000)],
        tables: &[PLAIN],
        fanout: &[],
    },
    Row {
        case: "two puts into one table, TOTAL",
        detail: "TOTAL",
        actions: || vec![put(PLAIN, "p2", 0), put(PLAIN, "p3", 1_500)],
        tables: &[PLAIN],
        fanout: &[],
    },
    Row {
        case: "three puts of growing items, TOTAL",
        detail: "TOTAL",
        actions: || {
            vec![
                put(PLAIN, "p4", 0),
                put(PLAIN, "p5", 900),
                put(PLAIN, "p6", 2_500),
            ]
        },
        tables: &[PLAIN],
        fanout: &[],
    },
    Row {
        case: "an update of a stored item, TOTAL",
        detail: "TOTAL",
        actions: || vec![update(PLAIN)],
        tables: &[PLAIN],
        fanout: &[],
    },
    Row {
        case: "a delete of a stored item, TOTAL",
        detail: "TOTAL",
        actions: || vec![delete(PLAIN)],
        tables: &[PLAIN],
        fanout: &[],
    },
    Row {
        case: "a condition check beside a put, TOTAL",
        detail: "TOTAL",
        actions: || vec![condition_check(PLAIN), put(PLAIN, "p7", 0)],
        tables: &[PLAIN],
        fanout: &[],
    },
    Row {
        case: "a put into each of two tables, TOTAL",
        detail: "TOTAL",
        actions: || vec![put(PLAIN, "p8", 0), put(SECOND, "p9", 2_000)],
        tables: &[PLAIN, SECOND],
        fanout: &[],
    },
    Row {
        case: "a put of a small item into an indexed table, INDEXES",
        detail: "INDEXES",
        actions: || vec![put(INDEXED, "i0", 0)],
        tables: &[INDEXED],
        fanout: &[INDEXED],
    },
    Row {
        case: "a put of a multi-kilobyte item into an indexed table, INDEXES",
        detail: "INDEXES",
        actions: || vec![put(INDEXED, "i1", 3_000)],
        tables: &[INDEXED],
        fanout: &[INDEXED],
    },
    Row {
        case: "a put into an indexed table beside an update of a plain one, INDEXES",
        detail: "INDEXES",
        actions: || vec![put(INDEXED, "i2", 900), update(PLAIN)],
        tables: &[INDEXED, PLAIN],
        fanout: &[INDEXED],
    },
    Row {
        case: "a put into an indexed table, TOTAL",
        detail: "TOTAL",
        actions: || vec![put(INDEXED, "i3", 0)],
        tables: &[INDEXED],
        fanout: &[],
    },
];

#[tokio::test]
async fn a_write_only_transaction_reports_its_cost_in_the_write_arm_in_both_planes() {
    // One emulator for every row: each row reads its tables' series before and after its own
    // transaction, so what IT counted is the delta whatever the rows before it counted.
    let router = emulator_with_cloudwatch();
    create_tables(&router).await;
    for row in ROWS {
        let case = row.case;
        // The item the update, delete and condition-check rows stand on, put back for each
        // row: the row that deletes it leaves the key empty for the rows after it.
        seed_item(&router).await;

        // Read each table's series BEFORE the transaction: the tables were created and
        // seeded with requests of their own, so what this transaction counted is the delta.
        let mut before: Vec<(&str, f64)> = Vec::new();
        for table in row.tables {
            before.push((table, published_write_units(&router, table).await));
        }

        let body = json!({
            "TransactItems": (row.actions)(),
            "ReturnConsumedCapacity": row.detail,
        });
        let (status, answered) = call(&router, "TransactWriteItems", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected TransactWriteItems {body} to answer 200, got {status} \
             {answered:?}"
        );

        let entries = answered["ConsumedCapacity"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a ConsumedCapacity list, got {answered}")
            });
        let mut named: Vec<&str> = entries
            .iter()
            .map(|entry| entry["TableName"].as_str().unwrap_or_default())
            .collect();
        named.sort_unstable();
        let mut addressed: Vec<&str> = row.tables.to_vec();
        addressed.sort_unstable();
        assert_eq!(
            named, addressed,
            "case {case:?}: expected one entry per table the transaction addressed, got \
             {answered}"
        );

        for entry in &entries {
            let table = entry["TableName"].as_str().unwrap_or_default();
            let reported = write_arm(case, entry);
            assert_detail(case, row, table, entry);

            // The metric plane carries the same cost: the table's series gained exactly the
            // units the entry named.
            let was = before
                .iter()
                .find(|(name, _)| *name == table)
                .map_or(0.0, |(_, units)| *units);
            let published = published_write_units(&router, table).await;
            assert!(
                (published - was - reported).abs() < TOLERANCE,
                "case {case:?}: expected the {CONSUMED_WRITE} series of {table:?} to gain the \
                 {reported} units the response reported, from {was} to {}, got {published}",
                was + reported
            );
        }
    }
}

/// The doubling, read off the same member: the SAME item written once through the item API and
/// once through a single-element transaction, so the transaction's `WriteCapacityUnits` is
/// twice the put's — at item sizes on either side of a capacity unit, so the doubled figure is
/// not one number.
#[tokio::test]
async fn a_transaction_reports_twice_the_single_item_write_in_the_same_member() {
    let router = emulator_with_cloudwatch();
    create_tables(&router).await;

    for padding in [0_usize, 900, 2_500, 5_000] {
        let case = format!("a {padding}-byte payload written both ways");
        let written = item(&format!("w{padding}"), padding);
        let single = ok(
            &router,
            "PutItem",
            json!({
                "TableName": PLAIN,
                "Item": written.clone(),
                "ReturnConsumedCapacity": "TOTAL",
            }),
        )
        .await;
        let single_units = write_arm(&case, &single["ConsumedCapacity"]);

        let transactional = ok(
            &router,
            "TransactWriteItems",
            json!({
                "TransactItems": [{"Put": {"TableName": SECOND, "Item": written}}],
                "ReturnConsumedCapacity": "TOTAL",
            }),
        )
        .await;
        let entries = transactional["ConsumedCapacity"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a ConsumedCapacity list, got {transactional}")
            });
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: a transaction over one table reports one entry, got {transactional}"
        );
        let transactional_units = write_arm(&case, &entries[0]);
        assert!(
            (transactional_units - 2.0 * single_units).abs() < TOLERANCE,
            "case {case:?}: expected the transaction to report twice the {single_units} write \
             capacity units the same put reported, got {transactional_units}"
        );
    }
}

/// The write arm of one `ConsumedCapacity` or `Capacity` object, asserted and returned: the
/// aggregate is positive, `WriteCapacityUnits` equals it, and `ReadCapacityUnits` is absent
/// because a write consumed no read capacity.
fn write_arm(case: &str, consumed: &Value) -> f64 {
    let aggregate = consumed["CapacityUnits"].as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected a CapacityUnits aggregate, got {consumed}")
    });
    assert!(
        aggregate > 0.0,
        "case {case:?}: a transaction that wrote consumed capacity, got {consumed}"
    );
    assert_eq!(
        consumed["WriteCapacityUnits"].as_f64(),
        Some(aggregate),
        "case {case:?}: expected WriteCapacityUnits to equal CapacityUnits ({aggregate}), got \
         {consumed}"
    );
    assert_eq!(
        consumed.get("ReadCapacityUnits"),
        None,
        "case {case:?}: a write consumes no read capacity, so ReadCapacityUnits is absent, got \
         {consumed}"
    );
    aggregate
}

/// The breakdown the detail level asks for: `TOTAL` reports "only the aggregate
/// ConsumedCapacity for the operation", while `INDEXES` adds the table's own line and one per
/// index the write reached — each of them reporting its figure in the write arm too, and the
/// lines together making up the aggregate.
fn assert_detail(case: &str, row: &Row, table: &str, entry: &Value) {
    if row.detail == "TOTAL" {
        for line in ["Table", "LocalSecondaryIndexes", "GlobalSecondaryIndexes"] {
            assert_eq!(
                entry.get(line),
                None,
                "case {case:?}: TOTAL reports only the aggregate, so {table:?} carries no \
                 {line}, got {entry}"
            );
        }
        return;
    }
    let own = entry
        .get("Table")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Table line for {table:?}, got {entry}"));
    let mut lines = write_arm(case, own);
    for map in ["LocalSecondaryIndexes", "GlobalSecondaryIndexes"] {
        let Some(indexes) = entry.get(map).and_then(Value::as_object) else {
            continue;
        };
        for (index, capacity) in indexes {
            lines += write_arm(&format!("{case}: the {map} line for {index}"), capacity);
        }
    }
    let aggregate = entry["CapacityUnits"].as_f64().unwrap_or_default();
    assert!(
        (lines - aggregate).abs() < TOLERANCE,
        "case {case:?}: expected the lines of {table:?} to make up its {aggregate} aggregate, \
         got {lines} in {entry}"
    );
    if row.fanout.contains(&table) {
        let index = &entry["GlobalSecondaryIndexes"][INDEX];
        assert!(
            index.is_object(),
            "case {case:?}: expected {table:?} to itemise the {INDEX} index the write fanned \
             out to, got {entry}"
        );
    }
}

/// Create the three tables the rows address, and wait for them to be usable. Every create is
/// made BEFORE the first wait, so the three tables settle together rather than one after the
/// other.
async fn create_tables(router: &MultiServiceRouter) {
    for table in [PLAIN, SECOND] {
        ok(
            router,
            "CreateTable",
            json!({
                "TableName": table,
                "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                "BillingMode": "PAY_PER_REQUEST",
            }),
        )
        .await;
    }
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": INDEXED,
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "owner", "AttributeType": "S"},
            ],
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            // Provisioned well above what the rows spend on it: the capacity a table HOLDS
            // is another rule's ([`provisioned_capacity_metrics`]), and a throttle is not
            // what these rows are about.
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 500, "WriteCapacityUnits": 500},
            "GlobalSecondaryIndexes": [{
                "IndexName": INDEX,
                "KeySchema": [{"AttributeName": "owner", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
                "ProvisionedThroughput": {"ReadCapacityUnits": 500, "WriteCapacityUnits": 500},
            }],
        }),
    )
    .await;
    for table in [PLAIN, SECOND, INDEXED] {
        describe_when_active(router, table, table).await;
    }
}

/// Put the item the update, delete and condition-check rows address. It lands BEFORE the sums
/// a row asserts are read, so what it consumed is outside the delta under test.
async fn seed_item(router: &MultiServiceRouter) {
    ok(
        router,
        "PutItem",
        json!({"TableName": PLAIN, "Item": item(SEED, 0)}),
    )
    .await;
}

/// One `Put` action of an item under `key`, padded by `padding` bytes of payload.
fn put(table: &str, key: &str, padding: usize) -> Value {
    json!({"Put": {"TableName": table, "Item": item(key, padding)}})
}

/// One `Update` action over the seeded item, which writes one attribute.
fn update(table: &str) -> Value {
    json!({
        "Update": {
            "TableName": table,
            "Key": {"pk": {"S": SEED}},
            "UpdateExpression": "SET n = :n",
            "ExpressionAttributeValues": {":n": {"N": "1"}},
        },
    })
}

/// One `Delete` action over the seeded item.
fn delete(table: &str) -> Value {
    json!({"Delete": {"TableName": table, "Key": {"pk": {"S": SEED}}}})
}

/// One `ConditionCheck` action over the seeded item, whose condition holds.
fn condition_check(table: &str) -> Value {
    json!({
        "ConditionCheck": {
            "TableName": table,
            "Key": {"pk": {"S": SEED}},
            "ConditionExpression": "attribute_exists(pk)",
        },
    })
}

/// One item under `key`, with the attribute the global secondary index is keyed on and
/// `padding` bytes of payload beside it, so a row can put the item's cost on either side of a
/// write capacity unit.
fn item(key: &str, padding: usize) -> Value {
    let mut written = json!({"pk": {"S": key}, "owner": {"S": "team"}});
    if padding > 0 {
        written["payload"] = json!({"S": "x".repeat(padding)});
    }
    written
}

/// The write capacity CloudWatch carries for `table`: the `Sum` of the series dimensioned by
/// the table alone plus the one its global secondary index publishes under
/// `{TableName, GlobalSecondaryIndexName}` — an index is provisioned for throughput of its
/// own, so its units are a series of its own, and a table with no index simply has no such
/// series for the read to find.
async fn published_write_units(router: &MultiServiceRouter, table: &str) -> f64 {
    let own = series_sum(router, json!([{"Name": "TableName", "Value": table}])).await;
    let index = series_sum(
        router,
        json!([
            {"Name": "TableName", "Value": table},
            {"Name": "GlobalSecondaryIndexName", "Value": INDEX},
        ]),
    )
    .await;
    own + index
}

/// The `Sum` CloudWatch reports for the write consumption series under `dimensions`, over a
/// window wide enough to hold everything a suite published.
async fn series_sum(router: &MultiServiceRouter, dimensions: Value) -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64();
    let answered = ok_cloudwatch(
        router,
        "GetMetricStatistics",
        json!({
            "Namespace": NAMESPACE,
            "MetricName": CONSUMED_WRITE,
            "Dimensions": dimensions,
            "StartTime": now - 3600.0,
            "EndTime": now + 3600.0,
            "Period": 60,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    answered["Datapoints"]
        .as_array()
        .map(|points| {
            points
                .iter()
                .filter_map(|point| point["Sum"].as_f64())
                .sum()
        })
        .unwrap_or_default()
}
