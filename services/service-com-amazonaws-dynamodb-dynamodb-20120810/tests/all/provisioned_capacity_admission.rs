// SPDX-License-Identifier: CC0-1.0

//! What a PROVISIONED table admits, over every surface that writes it — and the two facts
//! of a table's indexes and of its `RequestItems` map that go with it.
//!
//! Three rules are pinned here, each over a TABLE spanning what it generalises over rather
//! than one recorded request:
//!
//! * the capacity a table is provisioned for is a sustained RATE, so a request arriving
//!   while the table still has capacity is SERVED and charged what it consumed, however
//!   large, and it is the request that arrives on the deficit that draw left which is
//!   refused `ProvisionedThroughputExceededException`. The rows vary the surface the write
//!   came in on — the item API, PartiQL, a transaction — because capacity is a property of
//!   the TABLE and not of the surface;
//! * a GLOBAL secondary index whose build has not finished is no source a read can be
//!   served from, so a `Query` or a `Scan` naming one is refused rather than answered out
//!   of a half-built index;
//! * `RequestItems` is keyed by TABLE, and its key accepts a table's name or its ARN, so two
//!   keys naming one table address one table twice and the request is refused whole.
//!
//! Everything is driven over the wire, so nothing is asserted that a client could not
//! observe, and nothing waits on the wall clock: a status a caller must wait for is POLLED
//! until it holds.

use serde_json::{json, Value};

use crate::emulator::{call, describe_when_active, emulator, error_code, ok, REGION};

/// The sentence a refused write carries, as AWS fixes it for this class.
const THROUGHPUT_EXCEEDED: &str = "The level of configured provisioned throughput for the \
                                   table was exceeded. Consider increasing your provisioning \
                                   level with the UpdateTable API.";

/// The payload bytes of the one large item every capacity row writes: a write costs one unit
/// per 1 KB of the item, so an item of this size costs far more than a table provisioned for
/// one write unit can hold in credit — which is what makes the FIRST write of it the one
/// that drains the budget into deficit.
const LARGE_PAYLOAD_BYTES: usize = 340 * 1024;

/// Create a table provisioned for one read and one write unit and wait for it to settle.
async fn one_unit_table(router: &smithy_server_runtime::MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
        }),
    )
    .await;
    describe_when_active(router, &format!("the one-unit table {name}"), name).await;
}

/// One large item at `key`.
fn large_item(key: &str) -> Value {
    json!({"pk": {"S": key}, "payload": {"S": "x".repeat(LARGE_PAYLOAD_BYTES)}})
}

#[tokio::test]
async fn the_first_write_of_a_provisioned_table_is_served_and_the_next_is_throttled() {
    let router = emulator();

    // Each row is a surface a write arrives on, with the request that writes one large item
    // through it. The rule is the table's, so every surface behaves the same way: the first
    // such write lands and the one after it meets the deficit.
    type Request = fn(&str, &str) -> (&'static str, Value);
    let rows: Vec<(&str, Request)> = vec![
        ("the item API", |table, key| {
            ("PutItem", json!({"TableName": table, "Item": large_item(key)}))
        }),
        ("a PartiQL statement", |table, key| {
            (
                "ExecuteStatement",
                json!({"Statement": format!(
                    "INSERT INTO \"{table}\" VALUE {{'pk': ?, 'payload': ?}}"
                ),
                       "Parameters": [{"S": key}, {"S": "x".repeat(LARGE_PAYLOAD_BYTES)}]}),
            )
        }),
        ("a transaction", |table, key| {
            (
                "TransactWriteItems",
                json!({"TransactItems": [{"Put": {"TableName": table,
                                                  "Item": large_item(key)}}]}),
            )
        }),
        ("a PartiQL transaction", |table, key| {
            (
                "ExecuteTransaction",
                json!({"TransactStatements": [{
                    "Statement": format!(
                        "INSERT INTO \"{table}\" VALUE {{'pk': ?, 'payload': ?}}"
                    ),
                    "Parameters": [{"S": key}, {"S": "x".repeat(LARGE_PAYLOAD_BYTES)}],
                }]}),
            )
        }),
    ];

    for (index, (case, build)) in rows.into_iter().enumerate() {
        let table = format!("Admit{index}");
        one_unit_table(&router, &table).await;

        let (operation, first) = build(&table, "first");
        let (status, body) = call(&router, operation, first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first write of a provisioned table is served, got {status} \
             with {body}"
        );

        let (operation, second) = build(&table, "second");
        let (status, body) = call(&router, operation, second).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ProvisionedThroughputExceededException"),
            "case {case:?}: the write that meets the deficit is throttled, got {status} with \
             {body}"
        );
        assert_eq!(
            crate::emulator::message(&body),
            THROUGHPUT_EXCEEDED,
            "case {case:?}: the refusal carries the provisioning sentence, got {body}"
        );

        // The first write is READABLE and the second stored nothing, so what the capacity
        // decided is what the table holds.
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": table, "Key": {"pk": {"S": "first"}},
                   "ConsistentRead": true}),
        )
        .await;
        assert_eq!(
            read["Item"]["pk"]["S"],
            json!("first"),
            "case {case:?}: the served write is stored, got {read}"
        );
        let (status, missed) = call(
            &router,
            "GetItem",
            json!({"TableName": table, "Key": {"pk": {"S": "second"}},
                   "ConsistentRead": true}),
        )
        .await;
        assert!(
            status != 200 || missed.get("Item").is_none(),
            "case {case:?}: the throttled write stored nothing, got {status} with {missed}"
        );
    }
}

#[tokio::test]
async fn an_on_demand_table_admits_every_write_the_provisioned_one_turned_down() {
    let router = emulator();
    let table = "SwitchedToOnDemand";
    one_unit_table(&router, table).await;

    // Drain the budget: the first write lands, the second is refused.
    ok(
        &router,
        "PutItem",
        json!({"TableName": table, "Item": large_item("first")}),
    )
    .await;
    let (status, refused) = call(
        &router,
        "PutItem",
        json!({"TableName": table, "Item": large_item("second")}),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ProvisionedThroughputExceededException"),
        "case the drained table: expected the throttle, got {status} with {refused}"
    );

    // On-demand is "provisioned for no capacity to exceed", so the same write is served
    // once the table is switched — which is what tells a capacity ceiling from a defect of
    // the request.
    ok(
        &router,
        "UpdateTable",
        json!({"TableName": table, "BillingMode": "PAY_PER_REQUEST"}),
    )
    .await;
    describe_when_active(&router, "the switched table", table).await;
    for key in ["second", "third", "fourth"] {
        let (status, body) = call(
            &router,
            "PutItem",
            json!({"TableName": table, "Item": large_item(key)}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case on-demand write {key:?}: expected the write to be served, got {status} with \
             {body}"
        );
    }
}

#[tokio::test]
async fn a_global_index_still_being_built_serves_no_read() {
    let router = emulator();
    let table = "Backfilling";
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": table,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(&router, "the base table", table).await;
    for at in 0..3 {
        ok(
            &router,
            "PutItem",
            json!({"TableName": table,
                   "Item": {"pk": {"S": format!("k{at}")}, "sk": {"S": format!("s{at}")}}}),
        )
        .await;
    }

    // The index is added to a table that already holds items, so its build is in flight when
    // the update is answered: a read of it is refused the way a read naming an index no
    // table of this account has is, and the index is still THERE — its description reports
    // the status it is in, which is what tells a client to wait. The status is POLLED rather
    // than assumed, so the row does not depend on how fast the host runs.
    ok(
        &router,
        "UpdateTable",
        json!({
            "TableName": table,
            "AttributeDefinitions": [{"AttributeName": "sk", "AttributeType": "S"}],
            "GlobalSecondaryIndexUpdates": [{"Create": {
                "IndexName": "by-sk",
                "KeySchema": [{"AttributeName": "sk", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
            }}],
        }),
    )
    .await;
    crate::emulator::table_when(&router, "the index being built", REGION, table, |t| {
        t["GlobalSecondaryIndexes"][0]["IndexStatus"] == json!("CREATING")
    })
    .await;
    for (case, operation, request) in [
        (
            "a Query of an index still being built",
            "Query",
            json!({"TableName": table, "IndexName": "by-sk",
                   "KeyConditionExpression": "sk = :s",
                   "ExpressionAttributeValues": {":s": {"S": "s0"}}}),
        ),
        (
            "a Scan of an index still being built",
            "Scan",
            json!({"TableName": table, "IndexName": "by-sk"}),
        ),
    ] {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the read to be refused, got {status} with {body}"
        );
        assert_eq!(
            crate::emulator::message(&body),
            "The table does not have the specified index: by-sk",
            "case {case:?}: expected the no-such-index sentence, got {body}"
        );
    }

    // Once the build has settled the same reads are served, so the refusal is about the
    // build and nothing else. The status is POLLED until it holds.
    let described = crate::emulator::table_when(&router, "the built index", REGION, table, |t| {
        t["GlobalSecondaryIndexes"][0]["IndexStatus"] == json!("ACTIVE")
    })
    .await;
    assert_eq!(
        described["GlobalSecondaryIndexes"][0]["IndexName"],
        json!("by-sk"),
        "case the built index: the table reports the index it built, got {described}"
    );
    let walked = ok(
        &router,
        "Scan",
        json!({"TableName": table, "IndexName": "by-sk"}),
    )
    .await;
    assert_eq!(
        walked["Count"],
        json!(3),
        "case the built index: the settled index serves the table's items, got {walked}"
    );
}

#[tokio::test]
async fn request_items_naming_one_table_twice_is_refused() {
    let router = emulator();
    let table = "KeyedTwice";
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": table,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    let described = describe_when_active(&router, "the twice-keyed table", table).await;
    let arn = described["TableArn"]
        .as_str()
        .expect("the description names the table's ARN")
        .to_string();

    // One write the request DOES ask for once, so the table is not empty for a reason of
    // its own: the rows below must leave exactly this item behind.
    ok(
        &router,
        "PutItem",
        json!({"TableName": table, "Item": {"pk": {"S": "control"}}}),
    )
    .await;

    // Each row names ONE table under two of the spellings its key accepts, in a different
    // order and with a different number of operations on each side: the map is keyed by
    // TABLE, so every one of them addresses one table twice and is refused whole.
    let put = |key: &str| json!({"PutRequest": {"Item": {"pk": {"S": key}}}});
    let rows: Vec<(&str, Value)> = vec![
        (
            "the name first, then the ARN",
            json!({table: [put("name-00"), put("name-01")],
                   arn.clone(): [put("arn-00"), put("arn-01"), put("arn-02")]}),
        ),
        (
            "the ARN first, then the name",
            json!({arn.clone(): [put("arn-10")],
                   table: [put("name-10"), put("name-11")]}),
        ),
        (
            "a delete under one spelling and a put under the other",
            json!({table: [{"DeleteRequest": {"Key": {"pk": {"S": "control"}}}}],
                   arn.clone(): [put("arn-20")]}),
        ),
    ];
    for (case, request_items) in rows {
        let (status, body) = call(
            &router,
            "BatchWriteItem",
            json!({"RequestItems": request_items}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: one table addressed twice is refused, got {status} with {body}"
        );
        assert_eq!(
            crate::emulator::message(&body),
            "Each table name or ARN can be used only once per request",
            "case {case:?}: expected the once-per-request sentence, got {body}"
        );
    }

    // A refused batch wrote nothing and removed nothing: the one item the control write
    // stored is still all the table holds.
    let walked = ok(
        &router,
        "Scan",
        json!({"TableName": table, "ConsistentRead": true,
               "ProjectionExpression": "pk"}),
    )
    .await;
    let mut held: Vec<String> = walked["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item["pk"]["S"].as_str().map(str::to_string))
        .collect();
    held.sort();
    assert_eq!(
        held,
        vec!["control".to_string()],
        "case a refused batch: nothing of it was applied, the table holds {held:?}"
    );

    // And the SAME operations under ONE spelling are served, so the refusal is about the
    // map's keys and not about the writes it carried.
    let served = ok(
        &router,
        "BatchWriteItem",
        json!({"RequestItems": {table: [put("name-00"), put("arn-00")]}}),
    )
    .await;
    assert_eq!(
        served["UnprocessedItems"],
        json!({}),
        "case one spelling: the same writes land whole, got {served}"
    );
}
