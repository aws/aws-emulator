// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a table serves and refuses while one of its VECTOR indexes is
//! still being built, and for the capacity mode such an index requires.
//!
//! The rules pinned here:
//!
//! 1. an index change asked of a table that is already building one is past the limit on
//!    how many of a table's indexes may be built at once, so it is answered
//!    `LimitExceededException` — the code a client backs off and retries one index at a
//!    time on — rather than the in-use code it would abort on;
//! 2. a `Delete` of the index that is itself being built is held to the PHASE that build is
//!    in: while a vector index is still having its resources allocated there is nothing
//!    allocated to take away, so the delete is refused `ResourceInUseException` and the
//!    index keeps building; from its backfill on — and once it has settled — the delete is
//!    served, CANCELS the build and reports the index `DELETING` from there, which is how a
//!    caller undoes an index it mis-specified;
//! 3. a vector index exists only on an on-demand table, so an `UpdateTable` that would
//!    leave a table carrying one in the provisioned capacity mode is refused — whether it
//!    adds the index or switches the mode — while one that removes the last of them in the
//!    same call is served.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, and each rule is asserted over a
//! SET of inputs — which change the second call asks for, which phase the index is in,
//! which member states the capacity mode — rather than one example.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// One vector index body over the `emb` attribute, with four dimensions.
fn vector_index(name: &str) -> Value {
    json!({
        "IndexName": name,
        "VectorAttribute": {"AttributeName": "emb"},
        "Dimensions": 4,
        "DistanceFunction": "COSINE",
        "Projection": {"ProjectionType": "ALL"},
    })
}

/// One `UpdateTable` body adding `index` as a vector index.
fn add_vector_index(table: &str, index: &str) -> Value {
    json!({
        "TableName": table,
        "VectorIndexUpdates": [{"Create": vector_index(index)}],
    })
}

/// One `UpdateTable` body removing the vector index `index`.
fn remove_vector_index(table: &str, index: &str) -> Value {
    json!({
        "TableName": table,
        "VectorIndexUpdates": [{"Delete": {"IndexName": index}}],
    })
}

/// Create an on-demand table with a `pk` partition key, wait for it to be `ACTIVE`, and
/// write `items` items to it — the items a vector index added later has to read.
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

/// The status a table's description reports for the vector index `index`, or `None` when
/// the table carries no index by that name.
fn vector_index_status(description: &Value, index: &str) -> Option<String> {
    description["VectorIndexes"]
        .as_array()?
        .iter()
        .find(|reported| reported["IndexName"] == json!(index))
        .map(|reported| {
            reported["IndexStatus"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
}

/// AWS's fixed sentence for the limit on how many of a table's indexes may be created or
/// deleted at once.
const ONLINE_INDEX_LIMIT_MESSAGE: &str =
    "Subscriber limit exceeded: Only 1 online index can be created or deleted simultaneously \
     per table";

#[tokio::test]
async fn an_index_change_while_one_is_building_is_refused_as_the_limit_it_exceeds() {
    let router = emulator();
    let table = "VectorBuildingBlocksChanges";
    // A vector index declared WITH the table, so the rows below have a settled index of the
    // table to address, and items for the index added afterwards to read, so that index is
    // building for the span this test drives: what the table answers while it builds is
    // what is under test.
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": table,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "VectorIndexes": [vector_index("vidx-settled")],
        }),
    )
    .await;
    describe_when_active(&router, table, table).await;
    for item in 0..2 {
        ok(
            &router,
            "PutItem",
            json!({
                "TableName": table,
                "Item": {
                    "pk": {"S": format!("k{item}")},
                    "emb": {"L": [{"N": "1"}, {"N": "0"}, {"N": "0"}, {"N": "0"}]},
                },
            }),
        )
        .await;
    }
    ok(&router, "UpdateTable", add_vector_index(table, "vidx-a")).await;

    // Rows differ in the change the SECOND call asks for: another vector index, the delete
    // of the table's OTHER vector index (the settled one, not the one building), and a
    // global secondary index — every one of them a second index build on a table already
    // building one.
    let cases: [(&str, Value); 3] = [
        (
            "a second vector index",
            add_vector_index(table, "vidx-b"),
        ),
        (
            "the delete of another vector index",
            remove_vector_index(table, "vidx-settled"),
        ),
        (
            "a global secondary index",
            json!({
                "TableName": table,
                "AttributeDefinitions": [{"AttributeName": "gsk", "AttributeType": "S"}],
                "GlobalSecondaryIndexUpdates": [{"Create": {
                    "IndexName": "gsi-b",
                    "KeySchema": [{"AttributeName": "gsk", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                }}],
            }),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = call(&router, "UpdateTable", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the change to be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "LimitExceededException",
            "case {case:?}: expected the per-table index limit, got {body}"
        );
        assert_eq!(
            message(&body),
            ONLINE_INDEX_LIMIT_MESSAGE,
            "case {case:?}: expected the limit's own sentence, got {body}"
        );
    }

    // ...and the index the table is building is still being built, so the refusals wrote
    // nothing.
    let described = ok(&router, "DescribeTable", json!({"TableName": table})).await;
    let reported = vector_index_status(&described["Table"], "vidx-a");
    assert_eq!(
        reported.as_deref(),
        Some("CREATING"),
        "the refused changes must leave the building index as it was, got {described}"
    );

    // The limit is about the INDEX being built and not about the table's own update: once
    // the table reports itself settled, an index it is still building keeps the limit in
    // force — which is exactly what a client that waits for the table and then adds its
    // next index meets.
    describe_when_active(&router, table, table).await;
    let described = ok(&router, "DescribeTable", json!({"TableName": table})).await;
    assert_eq!(
        vector_index_status(&described["Table"], "vidx-a").as_deref(),
        Some("CREATING"),
        "an index added over pre-existing items is still being built when the table's own \
         update has settled, got {described}"
    );
    let (status, body) = call(&router, "UpdateTable", add_vector_index(table, "vidx-c")).await;
    assert_eq!(
        status, 400,
        "expected the settled table to refuse a second index build, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceededException",
        "expected the per-table index limit while the first index builds, got {body}"
    );
}

#[tokio::test]
async fn deleting_the_index_being_built_is_served_once_it_is_reading_the_table() {
    let router = emulator();
    let table = "VectorCancelBackfillingBuild";
    // The table holds items, so the added index keeps reading them after the table's own
    // update has settled — the phase a cancellation is served in like any other.
    on_demand_table(&router, table, 2).await;
    ok(&router, "UpdateTable", add_vector_index(table, "vidx-a")).await;
    describe_when_active(&router, table, table).await;
    let described = ok(&router, "DescribeTable", json!({"TableName": table})).await;
    assert_eq!(
        vector_index_status(&described["Table"], "vidx-a").as_deref(),
        Some("CREATING"),
        "the added index must still be building for this test to be about that, got \
         {described}"
    );

    let answered = ok(&router, "UpdateTable", remove_vector_index(table, "vidx-a")).await;
    assert_eq!(
        vector_index_status(&answered["TableDescription"], "vidx-a").as_deref(),
        Some("DELETING"),
        "the cancelled build must be reported as the deletion it now is, got {answered}"
    );
    // And the index serves nothing from that moment on: a search through the name it was
    // created under is answered as a name no index of the table has.
    let (status, body) = call(
        &router,
        "SearchVectors",
        json!({
            "TableName": table,
            "IndexName": "vidx-a",
            "SearchVector": [{"N": "1"}, {"N": "0"}, {"N": "0"}, {"N": "0"}],
            "TopK": 1,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a cancelled index serves no search, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "a name no index of the table has is the request's own mistake, got {body}"
    );
}

#[tokio::test]
async fn a_building_secondary_index_follows_the_same_build_phases() {
    let router = emulator();
    let table = "SecondaryIndexBuildPhases";
    // The lifecycle belongs to the INDEX and not to the kind of index being built, so a
    // global secondary index added to a table that already holds items is cancelled the
    // same way a vector index is: the delete is served while the index is still being
    // built, the index reports `DELETING`, and the table is left carrying none of it.
    on_demand_table(&router, table, 2).await;
    let add = json!({
        "TableName": table,
        "AttributeDefinitions": [{"AttributeName": "gsk", "AttributeType": "S"}],
        "GlobalSecondaryIndexUpdates": [{"Create": {
            "IndexName": "gsi-a",
            "KeySchema": [{"AttributeName": "gsk", "KeyType": "HASH"}],
            "Projection": {"ProjectionType": "ALL"},
        }}],
    });
    let remove = json!({
        "TableName": table,
        "GlobalSecondaryIndexUpdates": [{"Delete": {"IndexName": "gsi-a"}}],
    });
    ok(&router, "UpdateTable", add).await;

    // Straight after the create, while the index is still being built, the delete of that
    // very index is served and reports it on its way out.
    let answered = ok(&router, "UpdateTable", remove).await;
    assert_eq!(
        gsi_status(&answered["TableDescription"], "gsi-a").as_deref(),
        Some("DELETING"),
        "the cancelled build must be reported as the deletion it now is, got {answered}"
    );

    // Once the table's own update has settled the index is gone altogether.
    describe_when_active(&router, table, table).await;
    let described = ok(&router, "DescribeTable", json!({"TableName": table})).await;
    let reported: Vec<&str> = described["Table"]["GlobalSecondaryIndexes"]
        .as_array()
        .map(|indexes| {
            indexes
                .iter()
                .filter_map(|index| index["IndexName"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        reported.is_empty(),
        "the cancelled build must leave the table carrying no such index, got {described}"
    );
}

/// The status a table's description reports for the GLOBAL secondary index `index`, or
/// `None` when the table carries no index by that name.
fn gsi_status(description: &Value, index: &str) -> Option<String> {
    description["GlobalSecondaryIndexes"]
        .as_array()?
        .iter()
        .find(|reported| reported["IndexName"] == json!(index))
        .map(|reported| {
            reported["IndexStatus"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
}

/// AWS's fixed sentence for a delete asked of a vector index whose creation is still
/// allocating its resources: fixed text up to `Table:`, then the two names the request
/// stated.
fn allocation_phase_message(table: &str, index: &str) -> String {
    format!(
        "Attempt to change a resource which is still in use: Index creation is in resource \
         allocation phase. Retry deletion during backfilling phase or when the index is \
         active. Table: {table} Index: {index}"
    )
}

#[tokio::test]
async fn deleting_a_vector_index_still_allocating_its_resources_is_refused() {
    let router = emulator();
    // Rows differ in whether the table holds items for the index to read, which decides only
    // how long the build runs after its resources are allocated: a delete that arrives while
    // that allocation is still running is refused either way.
    for (case, table, items) in [
        ("an index over pre-existing items", "VectorAllocFilled", 2usize),
        ("an index over no items at all", "VectorAllocEmpty", 0),
    ] {
        on_demand_table(&router, table, items).await;
        ok(&router, "UpdateTable", add_vector_index(table, "vidx-a")).await;

        let (status, body) = call(&router, "UpdateTable", remove_vector_index(table, "vidx-a"))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body).as_str()),
            (
                400,
                "ResourceInUseException",
                allocation_phase_message(table, "vidx-a").as_str()
            ),
            "case {case:?}: expected the delete refused while the index allocates its \
             resources, got {status} with {body}"
        );

        // The refused delete wrote nothing: the index is still there, still being built.
        let described = ok(&router, "DescribeTable", json!({"TableName": table})).await;
        assert_eq!(
            vector_index_status(&described["Table"], "vidx-a").as_deref(),
            Some("CREATING"),
            "case {case:?}: the refused delete must leave the index building, got {described}"
        );
    }
}

#[tokio::test]
async fn a_settled_vector_index_is_deletable() {
    let router = emulator();
    let table = "VectorDeleteSettled";
    // No items, so the build is done when its resources are: the refusal above is about
    // the PHASE and not about the index, so the same delete is served once it has settled.
    on_demand_table(&router, table, 0).await;
    ok(&router, "UpdateTable", add_vector_index(table, "vidx-a")).await;
    describe_when_active(&router, table, table).await;
    let described = ok(&router, "DescribeTable", json!({"TableName": table})).await;
    assert_eq!(
        vector_index_status(&described["Table"], "vidx-a").as_deref(),
        Some("ACTIVE"),
        "an index with nothing to read settles with the table's own update, got {described}"
    );
    let answered = ok(&router, "UpdateTable", remove_vector_index(table, "vidx-a")).await;
    assert_eq!(
        vector_index_status(&answered["TableDescription"], "vidx-a").as_deref(),
        Some("DELETING"),
        "a settled index is deletable, and the accepted delete is reported on it, got \
         {answered}"
    );
}

/// AWS's fixed sentence for a vector index on a table that is not on-demand.
const ON_DEMAND_ONLY_MESSAGE: &str =
    "One or more parameter values were invalid: Vector indexes are only supported for \
     PAY_PER_REQUEST tables";

#[tokio::test]
async fn an_update_cannot_leave_a_table_with_a_vector_index_provisioned() {
    let router = emulator();
    // Rows differ in HOW the update would leave the table provisioned while carrying a
    // vector index: by adding the index to a provisioned table (through either member that
    // states the capacity mode), and by switching a table that already carries one.
    let cases: [(&str, &str, bool, Value); 4] = [
        (
            "adding one to a provisioned table",
            "VectorModeAddProvisioned",
            false,
            json!({"VectorIndexUpdates": [{"Create": vector_index("vidx")}]}),
        ),
        (
            "adding one while switching to PROVISIONED",
            "VectorModeAddAndSwitch",
            false,
            json!({
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {"ReadCapacityUnits": 2, "WriteCapacityUnits": 3},
                "VectorIndexUpdates": [{"Create": vector_index("vidx")}],
            }),
        ),
        (
            "switching a table that carries one, through BillingMode",
            "VectorModeSwitchBilling",
            true,
            json!({
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
            }),
        ),
        (
            "switching a table that carries one, through TableThroughputMode",
            "VectorModeSwitchThroughputMode",
            true,
            json!({
                "TableThroughputMode": "PROVISIONED",
                "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
            }),
        ),
    ];

    for (case, table, carries_index, update) in cases {
        let mut create = json!({
            "TableName": table,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
        });
        if carries_index {
            create["BillingMode"] = json!("PAY_PER_REQUEST");
            create["VectorIndexes"] = json!([vector_index("vidx")]);
        } else {
            create["BillingMode"] = json!("PROVISIONED");
            create["ProvisionedThroughput"] =
                json!({"ReadCapacityUnits": 1, "WriteCapacityUnits": 1});
        }
        ok(&router, "CreateTable", create).await;
        describe_when_active(&router, case, table).await;

        let mut request = update.clone();
        request["TableName"] = json!(table);
        let (status, body) = call(&router, "UpdateTable", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: the capacity mode a vector index needs is a parameter rule, got \
             {body}"
        );
        assert_eq!(
            message(&body),
            ON_DEMAND_ONLY_MESSAGE,
            "case {case:?}: expected the capacity-mode sentence, got {body}"
        );
    }
}

#[tokio::test]
async fn an_update_that_removes_the_last_vector_index_may_switch_the_mode() {
    let router = emulator();
    let table = "VectorModeSwitchAfterRemoval";
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": table,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "VectorIndexes": [vector_index("vidx")],
        }),
    )
    .await;
    describe_when_active(&router, table, table).await;

    // The table is left carrying no vector index, so the mode it is left in is one it may
    // hold: the rule is about what the table CARRIES after the update.
    let answered = ok(
        &router,
        "UpdateTable",
        json!({
            "TableName": table,
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
            "VectorIndexUpdates": [{"Delete": {"IndexName": "vidx"}}],
        }),
    )
    .await;
    assert_eq!(
        vector_index_status(&answered["TableDescription"], "vidx").as_deref(),
        Some("DELETING"),
        "the removed index must be reported on its way out, got {answered}"
    );
    assert_eq!(
        answered["TableDescription"]["BillingModeSummary"]["BillingMode"],
        json!("PROVISIONED"),
        "the mode the update stated must be in force, got {answered}"
    );
}
