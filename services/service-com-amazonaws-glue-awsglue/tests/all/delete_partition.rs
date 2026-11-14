// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the single-partition delete, `DeletePartition`.
//!
//! Every call goes over the wire the way a client makes it, through the shared
//! [`crate::wire`] harness: `POST /` with `X-Amz-Target: AWSGlue.<Operation>` and an
//! `awsJson1_1` body, dispatched by the same `MultiServiceRouter` the production front
//! door uses — so what is asserted here is HTTP behaviour and not a direct call into a
//! handler.
//!
//! The rule under test: a delete removes, from the table the request names, the ONE stored
//! partition its `PartitionValues` address — matched exactly as sent, as a read matches
//! them — so no later read of the catalog serves it, while every other partition of that
//! table and of every other table is left exactly as it was; a value list the addressed
//! table holds no partition for, and a database or table the catalog does not hold, is
//! refused with `EntityNotFoundException` and the wording this service answers for that
//! resource, and nothing is removed.
//!
//! The suites are tables whose rows differ along what the rule generalises over: how many
//! partition keys the table defines, how many partitions the table holds and which of them
//! the delete addresses, the form and casing of a partition value, and the way one delete
//! can fail.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::messages::PARTITION_NOT_FOUND;
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::assert_refusal;
use crate::partitions::{batch_create, get_partition, partition_errors, seed_table, values_of};
use crate::wire::{call, emulator, error_message, ok};

/// One value list, as the owned values a request carries.
fn vals(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}

/// `values` as the `&[&str]` the single-partition read is addressed by.
fn as_refs(values: &[String]) -> Vec<&str> {
    values.iter().map(String::as_str).collect()
}

/// The request body of a single delete: the table it addresses plus the values that
/// define the partition.
fn request(database: &str, table: &str, values: &[String]) -> Value {
    json!({
        "DatabaseName": database,
        "TableName": table,
        "PartitionValues": values,
    })
}

/// Submit one `DeletePartition`, returning its HTTP status and body.
async fn delete_partition(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    values: &[String],
) -> (u16, Value) {
    call(
        router,
        "DeletePartition",
        &request(database, table, values),
    )
    .await
}

/// Submit one `DeletePartition` that must answer 200, asserting the answer carries no
/// member: `DeletePartitionResponse` has none, so nothing of the removed definition comes
/// back.
async fn delete_ok(
    router: &MultiServiceRouter,
    case: &str,
    database: &str,
    table: &str,
    values: &[String],
) {
    let body = ok(
        router,
        "DeletePartition",
        &request(database, table, values),
    )
    .await;
    let empty = match &body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    };
    assert!(
        empty,
        "case {case}: a delete of {values:?} answers no member at all, got {body}"
    );
}

/// The `Values` of each partition a batch read answers, in the order it answered them.
async fn batch_read_values(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    values: &[Vec<String>],
) -> Vec<Value> {
    let to_get: Vec<Value> = values
        .iter()
        .map(|values| json!({ "Values": values }))
        .collect();
    let body = ok(
        router,
        "BatchGetPartition",
        &json!({
            "DatabaseName": database,
            "TableName": table,
            "PartitionsToGet": to_get,
        }),
    )
    .await;
    body["Partitions"]
        .as_array()
        .map(|partitions| {
            partitions
                .iter()
                .map(|partition| partition["Values"].clone())
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the delete table.
struct DeleteCase {
    case: String,
    /// The partition keys the addressed table defines.
    keys: Vec<&'static str>,
    /// The `PartitionInput`s the fixture creates, in order.
    created: Vec<Value>,
    /// The value lists the test deletes, one request each.
    delete: Vec<Vec<String>>,
    /// The value lists the deletes must leave, in the order they were created.
    kept: Vec<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Delete -> read round trip
// ---------------------------------------------------------------------------

/// Whatever a delete addresses stops being served by every partition read, and whatever it
/// did not address is still served exactly as it was stored.
///
/// The rows differ along the number of partition keys (1, 2, 3), how many partitions the
/// table holds and how many of them are deleted (its only one, two of three, three of
/// five, all of ten), and the form and casing of a partition value (a date, a Region,
/// digits, a value with spaces, a non-ASCII value, and two values differing only in case).
#[tokio::test]
async fn a_delete_removes_the_addressed_partition_and_leaves_the_others() {
    let router = emulator();

    let many: Vec<Value> = (0..10).map(|n| json!({ "Values": [n.to_string()] })).collect();

    let cases: Vec<DeleteCase> = vec![
        DeleteCase {
            case: "one key, the table's only partition".to_string(),
            keys: vec!["dt"],
            created: vec![json!({ "Values": ["2026-01-01"] })],
            delete: vec![vals(&["2026-01-01"])],
            kept: vec![],
        },
        DeleteCase {
            case: "two keys, two of three partitions".to_string(),
            keys: vec!["dt", "region"],
            created: vec![
                json!({
                    "Values": ["2026-01-01", "us-east-1"],
                    "StorageDescriptor": {
                        "Location": "s3://bucket/events/dt=2026-01-01/region=us-east-1/",
                        "Columns": [{ "Name": "id", "Type": "string" }]
                    },
                    "Parameters": { "classification": "csv" }
                }),
                json!({ "Values": ["2026-01-02", "us-east-1"] }),
                json!({ "Values": ["2026-01-02", "eu-west-1"] }),
            ],
            delete: vec![
                vals(&["2026-01-01", "us-east-1"]),
                vals(&["2026-01-02", "eu-west-1"]),
            ],
            kept: vec![vals(&["2026-01-02", "us-east-1"])],
        },
        DeleteCase {
            case: "three keys, the one partition".to_string(),
            keys: vec!["year", "month", "day"],
            created: vec![json!({
                "Values": ["2026", "01", "02"],
                "LastAccessTime": 1_767_225_600,
                "LastAnalyzedTime": 1_767_312_000
            })],
            delete: vec![vals(&["2026", "01", "02"])],
            kept: vec![],
        },
        DeleteCase {
            case: "one key, three of five partitions spanning value forms".to_string(),
            keys: vec!["k"],
            created: vec![
                json!({ "Values": ["a"] }),
                // A partition value is table DATA, so these two are different
                // partitions and only the one addressed is deleted.
                json!({ "Values": ["A"] }),
                json!({ "Values": ["a value with spaces"] }),
                json!({ "Values": ["100"] }),
                json!({ "Values": ["ünïcode=yes"] }),
            ],
            delete: vec![
                vals(&["A"]),
                vals(&["a value with spaces"]),
                vals(&["ünïcode=yes"]),
            ],
            kept: vec![vals(&["a"]), vals(&["100"])],
        },
        DeleteCase {
            case: "one key, every partition of ten, one delete each".to_string(),
            keys: vec!["n"],
            created: many.clone(),
            delete: (0..10).map(|n| vec![n.to_string()]).collect(),
            kept: vec![],
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let case = &row.case;
        let database = format!("single_delete_db_{index}");
        // Mixed case, so the delete addresses its table by a name the catalog folds.
        let table = format!("Events_{index}");
        seed_table(&router, &database, &table, &row.keys).await;
        let created = batch_create(&router, &database, &table, &row.created).await;
        assert!(
            partition_errors(&created).is_empty(),
            "case {case}: the fixture partitions must be created, got Errors {:?}",
            created["Errors"]
        );

        // What a read serves for every partition the deletes must leave, BEFORE they run.
        let mut before = Vec::new();
        for values in &row.kept {
            let (status, body) = get_partition(&router, &database, &table, &as_refs(values)).await;
            assert_eq!(
                status, 200,
                "case {case}: the fixture partition {values:?} must be readable before the \
                 delete, got {status} with {body}"
            );
            before.push(body["Partition"].clone());
        }

        for values in &row.delete {
            delete_ok(&router, case, &database, &table, values).await;
        }

        for values in &row.delete {
            let (status, body) = get_partition(&router, &database, &table, &as_refs(values)).await;
            assert_eq!(
                status, 400,
                "case {case}: the deleted partition {values:?} must no longer be served, got \
                 {status} with {body}"
            );
            assert_eq!(
                error_message(&body),
                PARTITION_NOT_FOUND,
                "case {case}: a read of the deleted {values:?} must answer the partition miss \
                 wording, got {:?}",
                error_message(&body)
            );
        }
        for (values, expected) in row.kept.iter().zip(before.iter()) {
            let (status, body) = get_partition(&router, &database, &table, &as_refs(values)).await;
            assert_eq!(
                status, 200,
                "case {case}: the delete did not address {values:?}, so a read must still serve \
                 it; got {status} with {body}"
            );
            assert_eq!(
                &body["Partition"], expected,
                "case {case}: the delete must leave {values:?} exactly as it was; expected \
                 {expected}, got {}",
                body["Partition"]
            );
        }

        // The batch read consults the same record, so it answers exactly the partitions
        // the deletes left — a deleted partition is gone for every reader, not only for
        // the single-partition one.
        let addressed: Vec<Vec<String>> = row
            .created
            .iter()
            .map(|input| vals(&values_of(input)))
            .collect();
        let answered = batch_read_values(&router, &database, &table, &addressed).await;
        let expected: Vec<Value> = row.kept.iter().map(|values| json!(values)).collect();
        assert_eq!(
            answered, expected,
            "case {case}: the batch read must answer exactly the partitions the deletes left; \
             expected {expected:?}, got {answered:?}"
        );
    }
}

/// A delete addresses a partition WITHIN the one table the request names: the same value
/// list in another table of the same database, and in a table of another database, is
/// untouched.
#[tokio::test]
async fn a_delete_touches_only_the_table_it_names() {
    let router = emulator();
    let shared = vals(&["2026-01-01"]);

    seed_table(&router, "scoped_db_a", "events", &["dt"]).await;
    // A second table of the SAME database, so the database is created once.
    ok(
        &router,
        "CreateTable",
        &json!({
            "DatabaseName": "scoped_db_a",
            "TableInput": {
                "Name": "other_events",
                "PartitionKeys": [{ "Name": "dt", "Type": "string" }]
            }
        }),
    )
    .await;
    seed_table(&router, "scoped_db_b", "events", &["dt"]).await;

    for (database, table) in [
        ("scoped_db_a", "events"),
        ("scoped_db_a", "other_events"),
        ("scoped_db_b", "events"),
    ] {
        let created = batch_create(
            &router,
            database,
            table,
            &[json!({ "Values": ["2026-01-01"] })],
        )
        .await;
        assert!(
            partition_errors(&created).is_empty(),
            "the fixture partition of {database}.{table} must be created, got Errors {:?}",
            created["Errors"]
        );
    }

    delete_ok(
        &router,
        "the addressed table",
        "scoped_db_a",
        "events",
        &shared,
    )
    .await;

    let (status, body) = get_partition(&router, "scoped_db_a", "events", &["2026-01-01"]).await;
    assert_eq!(
        status, 400,
        "the addressed partition must be gone, got {status} with {body}"
    );
    for (database, table) in [("scoped_db_a", "other_events"), ("scoped_db_b", "events")] {
        let (status, body) = get_partition(&router, database, table, &["2026-01-01"]).await;
        assert_eq!(
            status, 200,
            "the delete named another table, so {database}.{table} must still serve its \
             partition; got {status} with {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// One row of the partition-miss table.
struct MissCase {
    case: &'static str,
    /// The partition keys the addressed table defines.
    keys: Vec<&'static str>,
    /// The `PartitionInput`s the fixture creates.
    created: Vec<Value>,
    /// Value lists deleted (and which must succeed) before the refused delete.
    delete_first: Vec<Vec<String>>,
    /// The value list the refused delete addresses.
    delete: Vec<String>,
    /// Value lists a read must still serve after the refusal.
    readable: Vec<Vec<String>>,
}

/// A value list the addressed table holds no partition for is refused with
/// `EntityNotFoundException` and the partition-miss wording, and removes nothing — so a
/// delete is not idempotent: the second delete of the same partition is that same refusal.
///
/// The rows differ along the way the value list names no stored partition — never created,
/// already deleted, fewer values than the table has partition keys, more values than it
/// has, a value differing from a stored one only in case, no value at all — and along how
/// many partitions the table holds beside it.
#[tokio::test]
async fn a_delete_of_a_partition_the_table_does_not_hold_is_refused() {
    let router = emulator();

    let cases = [
        MissCase {
            case: "a value list nothing was created for",
            keys: vec!["dt"],
            created: vec![json!({ "Values": ["2026-01-01"] })],
            delete_first: vec![],
            delete: vals(&["2026-01-09"]),
            readable: vec![vals(&["2026-01-01"])],
        },
        MissCase {
            case: "the same partition deleted twice",
            keys: vec!["dt"],
            created: vec![
                json!({ "Values": ["2026-01-01"] }),
                json!({ "Values": ["2026-01-02"] }),
            ],
            delete_first: vec![vals(&["2026-01-01"])],
            delete: vals(&["2026-01-01"]),
            readable: vec![vals(&["2026-01-02"])],
        },
        MissCase {
            case: "fewer values than the table has partition keys",
            keys: vec!["dt", "region"],
            created: vec![json!({ "Values": ["2026-01-01", "us-east-1"] })],
            delete_first: vec![],
            delete: vals(&["2026-01-01"]),
            readable: vec![vals(&["2026-01-01", "us-east-1"])],
        },
        MissCase {
            case: "more values than the table has partition keys",
            keys: vec!["dt"],
            created: vec![json!({ "Values": ["2026-01-01"] })],
            delete_first: vec![],
            delete: vals(&["2026-01-01", "us-east-1"]),
            readable: vec![vals(&["2026-01-01"])],
        },
        MissCase {
            case: "a value differing from a stored one only in case",
            keys: vec!["k"],
            created: vec![json!({ "Values": ["Monday"] })],
            delete_first: vec![],
            delete: vals(&["monday"]),
            readable: vec![vals(&["Monday"])],
        },
        MissCase {
            case: "no value at all",
            keys: vec!["dt"],
            created: vec![json!({ "Values": ["2026-01-01"] })],
            delete_first: vec![],
            delete: vals(&[]),
            readable: vec![vals(&["2026-01-01"])],
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let case = row.case;
        let database = format!("missing_delete_db_{index}");
        let table = format!("missing_delete_table_{index}");
        seed_table(&router, &database, &table, &row.keys).await;
        let created = batch_create(&router, &database, &table, &row.created).await;
        assert!(
            partition_errors(&created).is_empty(),
            "case {case}: the fixture partitions must be created, got Errors {:?}",
            created["Errors"]
        );
        for values in &row.delete_first {
            delete_ok(&router, case, &database, &table, values).await;
        }

        assert_refusal(
            &router,
            case,
            "DeletePartition",
            &request(&database, &table, &row.delete),
            "EntityNotFoundException",
            PARTITION_NOT_FOUND,
        )
        .await;

        for values in &row.readable {
            let (status, read) = get_partition(&router, &database, &table, &as_refs(values)).await;
            assert_eq!(
                status, 200,
                "case {case}: the refused delete removed nothing, so {values:?} must still be \
                 served; got {status} with {read}"
            );
        }
    }
}

/// A database or table the catalog does not hold is refused with
/// `EntityNotFoundException` and the wording this service answers for that resource —
/// there is no table to delete a partition from — and nothing of the stored catalog
/// changes.
#[tokio::test]
async fn a_delete_naming_no_stored_table_is_refused() {
    let router = emulator();
    seed_table(&router, "present_single_db", "present_single_table", &["dt"]).await;
    let created = batch_create(
        &router,
        "present_single_db",
        "present_single_table",
        &[json!({ "Values": ["2026-01-01"] })],
    )
    .await;
    assert!(
        partition_errors(&created).is_empty(),
        "the fixture partition must be created, got Errors {:?}",
        created["Errors"]
    );

    let cases = [
        (
            "no such database",
            "ghost_db",
            "present_single_table",
            "Database ghost_db not found.",
        ),
        (
            "no such database, mixed case",
            "Ghost_DB",
            "present_single_table",
            "Database ghost_db not found.",
        ),
        (
            "no such table",
            "present_single_db",
            "ghost_table",
            "Table ghost_table not found.",
        ),
        (
            "no such table, mixed case",
            "present_single_db",
            "Ghost_Table",
            "Table ghost_table not found.",
        ),
    ];

    for (case, database, table, expected_message) in cases {
        assert_refusal(
            &router,
            case,
            "DeletePartition",
            &request(database, table, &vals(&["2026-01-01"])),
            "EntityNotFoundException",
            expected_message,
        )
        .await;
    }

    let (status, body) = get_partition(
        &router,
        "present_single_db",
        "present_single_table",
        &["2026-01-01"],
    )
    .await;
    assert_eq!(
        status, 200,
        "every refused delete named another resource, so the stored partition must still be \
         served; got {status} with {body}"
    );
}

/// A delete reaches the catalog the request names: the default catalog is the caller's
/// account, and a `CatalogId` naming it addresses the same partition the create wrote,
/// while a catalog the store does not hold holds no database to delete from.
#[tokio::test]
async fn a_delete_addresses_the_catalog_the_request_names() {
    let router = emulator();
    seed_table(&router, "catalog_delete_db", "catalog_delete_table", &["dt"]).await;
    let created = batch_create(
        &router,
        "catalog_delete_db",
        "catalog_delete_table",
        &[json!({ "Values": ["2026-01-01"] })],
    )
    .await;
    assert!(
        partition_errors(&created).is_empty(),
        "the fixture partition must be created, got Errors {:?}",
        created["Errors"]
    );

    // A catalog id no catalog of the store holds: the database lookup fails there, so the
    // delete is refused and the partition of the default catalog is untouched.
    let mut other_catalog = request(
        "catalog_delete_db",
        "catalog_delete_table",
        &vals(&["2026-01-01"]),
    );
    other_catalog["CatalogId"] = json!("210987654321");
    assert_refusal(
        &router,
        "another catalog",
        "DeletePartition",
        &other_catalog,
        "EntityNotFoundException",
        "Database catalog_delete_db not found.",
    )
    .await;
    let (status, body) = get_partition(
        &router,
        "catalog_delete_db",
        "catalog_delete_table",
        &["2026-01-01"],
    )
    .await;
    assert_eq!(
        status, 200,
        "the refused delete named another catalog, so the stored partition must still be \
         served; got {status} with {body}"
    );

    // The caller's own account id names the catalog the create defaulted to, so the delete
    // addresses exactly the partition that create wrote.
    let mut own_catalog = request(
        "catalog_delete_db",
        "catalog_delete_table",
        &vals(&["2026-01-01"]),
    );
    own_catalog["CatalogId"] = json!(crate::wire::ACCOUNT_ID);
    let (status, body) = call(&router, "DeletePartition", &own_catalog).await;
    assert_eq!(
        status, 200,
        "the caller's account id names the catalog the create defaulted to, so the delete must \
         succeed; got {status} with {body}"
    );
    let (status, body) = get_partition(
        &router,
        "catalog_delete_db",
        "catalog_delete_table",
        &["2026-01-01"],
    )
    .await;
    assert_eq!(
        status, 400,
        "the deleted partition must no longer be served, got {status} with {body}"
    );
}

/// The partition a delete removed can be created again at the same values, which is what
/// AWS documents a value change as: "delete and recreate the partition"
/// (`UpdatePartitionRequest$PartitionInput`, from `smithy_query`).
#[tokio::test]
async fn a_deleted_partition_can_be_created_again() {
    let router = emulator();
    seed_table(&router, "recreate_db", "recreate_table", &["dt"]).await;
    let created = batch_create(
        &router,
        "recreate_db",
        "recreate_table",
        &[json!({ "Values": ["2026-01-01"] })],
    )
    .await;
    assert!(
        partition_errors(&created).is_empty(),
        "the fixture partition must be created, got Errors {:?}",
        created["Errors"]
    );

    delete_ok(
        &router,
        "recreate",
        "recreate_db",
        "recreate_table",
        &vals(&["2026-01-01"]),
    )
    .await;

    let (status, body) = call(
        &router,
        "CreatePartition",
        &json!({
            "DatabaseName": "recreate_db",
            "TableName": "recreate_table",
            "PartitionInput": {
                "Values": ["2026-01-01"],
                "Parameters": { "classification": "parquet" }
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the values are free again after the delete, so the create must succeed; got {status} \
         with {body}"
    );

    let (status, read) = get_partition(&router, "recreate_db", "recreate_table", &["2026-01-01"]).await;
    assert_eq!(
        status, 200,
        "the recreated partition must be served; got {status} with {read}"
    );
    assert_eq!(
        read["Partition"]["Parameters"]["classification"].as_str(),
        Some("parquet"),
        "the recreated partition must carry what the second create sent, got {read}"
    );

    // A second delete of the recreated partition succeeds too: the record the delete reads
    // is the one the create wrote.
    let (status, body) = delete_partition(
        &router,
        "recreate_db",
        "recreate_table",
        &vals(&["2026-01-01"]),
    )
    .await;
    assert_eq!(
        status, 200,
        "the recreated partition is stored, so deleting it must succeed; got {status} with {body}"
    );
    let (status, read) = get_partition(&router, "recreate_db", "recreate_table", &["2026-01-01"]).await;
    assert_eq!(
        status, 400,
        "the partition must be gone again after the second delete; got {status} with {read}"
    );
}
