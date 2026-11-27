// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the PHASE of a vector index's build that decides what `UpdateTable`
//! may do to it, driven over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<Operation>`).
//!
//! RULE: a build allocates the index's resources before it reads anything, so an index whose
//! allocation is still running is not yet a resource a caller can take away: a
//! `VectorIndexUpdates` `Delete` naming it is refused the modelled `ResourceInUseException`
//! ("the operation conflicts with the resource's availability ... you tried to update a
//! resource that was already being updated"), worded
//! `Attempt to change a resource which is still in use: Index creation is in resource
//! allocation phase. Retry deletion during backfilling phase or when the index is active.
//! Table: <TableName> Index: <IndexName>` — fixed text up to `Table:`, with the two names
//! taken from the request — and the refused call writes nothing at all: the index keeps
//! building, still listed, still `CREATING`. From the phase the message names on, the same
//! delete is SERVED: an index reading the items its table already held is cancelled by it,
//! and so is a settled one.
//!
//! The rows span what the rule generalises over: whether the table holds items for the index
//! to read (which decides whether a backfilling phase follows the allocation at all), the
//! index's own name and distance function, and the phase the delete arrives in — allocation,
//! backfill, and settled. The refusal's own wording is driven over two tables and two index
//! names, because the sentence carries both.
//!
//! Sources: `UpdateTable`'s `ResourceInUseException` and
//! `VectorIndexDescription$IndexStatus`/`$Backfilling` in this service's Smithy model
//! (`smithy_query(query="shape",shape="VectorIndexDescription")`); the phases the sentence
//! names are the recorded refusal's own.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// One vector index body over the `emb` attribute, with four dimensions.
fn vector_index(name: &str, distance: &str) -> Value {
    json!({
        "IndexName": name,
        "VectorAttribute": {"AttributeName": "emb"},
        "Dimensions": 4,
        "DistanceFunction": distance,
        "Projection": {"ProjectionType": "ALL"},
    })
}

/// One `UpdateTable` body adding `index` as a vector index of `table`.
fn add(table: &str, index: &str, distance: &str) -> Value {
    json!({"TableName": table, "VectorIndexUpdates": [{"Create": vector_index(index, distance)}]})
}

/// One `UpdateTable` body deleting the vector index `index` of `table`.
fn remove(table: &str, index: &str) -> Value {
    json!({"TableName": table, "VectorIndexUpdates": [{"Delete": {"IndexName": index}}]})
}

/// AWS's sentence for a delete asked of an index still allocating its resources: fixed text up
/// to `Table:`, then the names the request stated.
fn still_allocating(table: &str, index: &str) -> String {
    format!(
        "Attempt to change a resource which is still in use: Index creation is in resource \
         allocation phase. Retry deletion during backfilling phase or when the index is \
         active. Table: {table} Index: {index}"
    )
}

/// Create an on-demand table with a `pk` partition key, wait for it to be `ACTIVE`, and write
/// `items` items carrying the vector attribute — the items an index added later has to read.
async fn on_demand_table(router: &MultiServiceRouter, name: &str, items: usize) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
        }),
    )
    .await;
    describe_when_active(router, name, name).await;
    for item in 0..items {
        ok(
            router,
            "PutItem",
            json!({
                "TableName": name,
                "Item": {
                    "pk": {"S": format!("k{item}")},
                    "emb": {"L": [{"N": "1"}, {"N": "0"}, {"N": "0"}, {"N": "0"}]},
                },
            }),
        )
        .await;
    }
}

/// The description a table reports for one of its vector indexes, or `Value::Null` where it
/// carries no index by that name.
fn vector_index_entry(description: &Value, index: &str) -> Value {
    description["VectorIndexes"]
        .as_array()
        .and_then(|indexes| {
            indexes
                .iter()
                .find(|reported| reported["IndexName"] == json!(index))
                .cloned()
        })
        .unwrap_or(Value::Null)
}

/// RULE: the delete of an index still allocating its resources is refused, with both names the
/// request stated, and the index is left exactly as it was.
#[tokio::test]
async fn a_delete_in_the_allocation_phase_is_refused_and_writes_nothing() {
    let router = emulator();
    // Rows differ in whether the table holds items for the index to read — so in whether a
    // backfilling phase follows the allocation at all — and in the names and distance function
    // the request carries, which the sentence reproduces.
    for (case, table, index, distance, items) in [
        (
            "an index over pre-existing items",
            "VecPhaseFilled",
            "late-euc",
            "EUCLIDEAN",
            2usize,
        ),
        (
            "an index over no items at all",
            "VecPhaseEmpty",
            "early-cos",
            "COSINE",
            0,
        ),
    ] {
        on_demand_table(&router, table, items).await;
        let created = ok(&router, "UpdateTable", add(table, index, distance)).await;
        assert_eq!(
            vector_index_entry(&created["TableDescription"], index)["IndexStatus"],
            json!("CREATING"),
            "case {case:?}: the created index must report CREATING for this test to be about \
             that, got {created}"
        );

        let (status, body) = call(&router, "UpdateTable", remove(table, index)).await;
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body).as_str()),
            (
                400,
                "ResourceInUseException",
                still_allocating(table, index).as_str()
            ),
            "case {case:?}: expected the delete refused while the index allocates, got {status} \
             with {body}"
        );

        // Nothing of the refused call landed: the index is still listed, still CREATING, and
        // nothing is owed its removal.
        let described = ok(&router, "DescribeTable", json!({"TableName": table})).await;
        let entry = vector_index_entry(&described["Table"], index);
        assert_eq!(
            entry["IndexStatus"],
            json!("CREATING"),
            "case {case:?}: the refused delete must leave the index building, got {described}"
        );
        assert_eq!(
            entry["IndexName"],
            json!(index),
            "case {case:?}: the refused delete must leave the index listed, got {described}"
        );
    }
}

/// RULE, the other side of the phase: once the allocation is over the same delete is served —
/// an index still READING the items its table held is cancelled by it, and a settled index is
/// put on its way out — and a settled index's description carries no `Backfilling` member at
/// all.
#[tokio::test]
async fn a_delete_past_the_allocation_phase_is_served() {
    let router = emulator();

    // An index over pre-existing items: the table settles while the index keeps reading them,
    // which is the backfilling phase the refusal above tells the caller to retry in.
    let table = "VecPhaseBackfilling";
    let index = "backfilling-idx";
    on_demand_table(&router, table, 2).await;
    ok(&router, "UpdateTable", add(table, index, "COSINE")).await;
    describe_when_active(&router, table, table).await;
    let described = ok(&router, "DescribeTable", json!({"TableName": table})).await;
    let entry = vector_index_entry(&described["Table"], index);
    assert_eq!(
        (entry["IndexStatus"].clone(), entry["Backfilling"].clone()),
        (json!("CREATING"), json!(true)),
        "case \"an index reading the items the table held\": expected it still building and \
         backfilling, got {described}"
    );
    let answered = ok(&router, "UpdateTable", remove(table, index)).await;
    assert_eq!(
        vector_index_entry(&answered["TableDescription"], index)["IndexStatus"],
        json!("DELETING"),
        "case \"an index reading the items the table held\": the cancelled build must be \
         reported on its way out, got {answered}"
    );

    // And an index with nothing to read, deleted once it has settled: a settled index reports
    // no `Backfilling` at all, "after all items have been processed".
    let table = "VecPhaseSettled";
    let index = "settled-idx";
    on_demand_table(&router, table, 0).await;
    ok(&router, "UpdateTable", add(table, index, "EUCLIDEAN")).await;
    describe_when_active(&router, table, table).await;
    let described = ok(&router, "DescribeTable", json!({"TableName": table})).await;
    let entry = vector_index_entry(&described["Table"], index);
    assert_eq!(
        entry["IndexStatus"],
        json!("ACTIVE"),
        "case \"an index with nothing to read\": expected it settled, got {described}"
    );
    assert!(
        entry.get("Backfilling").is_none(),
        "case \"an index with nothing to read\": a settled index reports no Backfilling, got \
         {described}"
    );
    let answered = ok(&router, "UpdateTable", remove(table, index)).await;
    assert_eq!(
        vector_index_entry(&answered["TableDescription"], index)["IndexStatus"],
        json!("DELETING"),
        "case \"an index with nothing to read\": a settled index is deletable, got {answered}"
    );
}
