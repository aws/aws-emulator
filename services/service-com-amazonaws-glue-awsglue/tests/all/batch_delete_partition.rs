// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue batch partition delete, `BatchDeletePartition`.
//!
//! Every call goes over the wire the way a client makes it, through the shared
//! [`crate::wire`] harness: `POST /` with `X-Amz-Target: AWSGlue.<Operation>` and an
//! `awsJson1_1` body, dispatched by the same `MultiServiceRouter` the production front
//! door uses — so what is asserted here is HTTP behaviour and not a direct call into a
//! handler.
//!
//! The rule under test: a batch delete removes, from the table the request names, the
//! stored partition each addressed value list identifies — matched exactly as sent, as a
//! read matches it — so no later read of the catalog serves it, while every partition the
//! batch did not address is left exactly as it was. A value list the table holds no
//! partition for fails ONLY its own item, reported in `Errors` as a partition miss, and a
//! database or table the catalog does not hold fails the whole request with
//! `EntityNotFoundException`.
//!
//! The suites are tables whose rows differ along what the rule generalises over: how many
//! partition keys the table defines, how many partitions one batch addresses (up to the
//! modelled ceiling), how many of them the table holds, the form and casing of a
//! partition value, and the way one item can fail.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::messages::PARTITION_NOT_FOUND;
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::assert_refusal;
use crate::partitions::{batch_create, get_partition, partition_errors, seed_table, values_of};
use crate::wire::{call, emulator, error_message, ok, ERROR_DETAIL_TAG};

/// The largest `PartitionsToDelete` one batch may carry: `BatchDeletePartitionValueList`,
/// length 0..25, from `smithy_query` on `BatchDeletePartition`. This is the DELETE
/// batch's own ceiling; the create batch's is another one
/// (`crate::partitions::MAX_PARTITIONS_PER_BATCH`).
const MAX_DELETES_PER_BATCH: usize = 25;

/// Every per-item failure of a delete is the same one: the addressed value list names no
/// partition of the table, so there is nothing to remove.
const MISS_CODE: &str = "EntityNotFoundException";

/// One value list, as the owned values a request carries.
fn vals(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}

/// `values` as the `&[&str]` the single-partition read is addressed by.
fn as_refs(values: &[String]) -> Vec<&str> {
    values.iter().map(String::as_str).collect()
}

/// The request body of a batch delete: each addressed value list is one
/// `PartitionValueList` carrying its `Values`.
fn request(database: &str, table: &str, values: &[Vec<String>]) -> Value {
    let to_delete: Vec<Value> = values
        .iter()
        .map(|values| json!({ "Values": values }))
        .collect();
    json!({
        "DatabaseName": database,
        "TableName": table,
        "PartitionsToDelete": to_delete,
    })
}

/// Submit one `BatchDeletePartition` that must answer 200, and return its body.
async fn batch_delete(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    values: &[Vec<String>],
) -> Value {
    ok(
        router,
        "BatchDeletePartition",
        &request(database, table, values),
    )
    .await
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
    /// The value lists the batch addresses.
    delete: Vec<Vec<String>>,
    /// The value lists the delete must leave, in the order they were created.
    kept: Vec<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Delete -> read round trip
// ---------------------------------------------------------------------------

/// Whatever a batch delete addresses stops being served by every partition read, and
/// whatever it did not address is still served exactly as it was stored.
///
/// The rows differ along the number of partition keys (1, 2, 3), how many partitions the
/// table holds and how many of them one batch deletes (one of one, two of three, three of
/// five, all twenty-five), and the form and casing of a partition value (a date, a Region,
/// digits, a value with spaces, a non-ASCII value, and two values differing only in case).
#[tokio::test]
async fn a_batch_delete_removes_each_addressed_partition_and_leaves_the_others() {
    let router = emulator();

    // The modelled ceiling as one row: twenty-five partitions addressed by one batch.
    let at_ceiling: Vec<Vec<String>> = (0..MAX_DELETES_PER_BATCH)
        .map(|n| vec![n.to_string()])
        .collect();

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
            case: format!("{MAX_DELETES_PER_BATCH} partitions, the modelled ceiling"),
            keys: vec!["n"],
            created: (0..MAX_DELETES_PER_BATCH)
                .map(|n| json!({ "Values": [n.to_string()] }))
                .collect(),
            delete: at_ceiling.clone(),
            kept: vec![],
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let case = &row.case;
        let database = format!("delete_db_{index}");
        // Mixed case, so the delete addresses its table by a name the catalog folds.
        let table = format!("Events_{index}");
        seed_table(&router, &database, &table, &row.keys).await;
        let created = batch_create(&router, &database, &table, &row.created).await;
        assert!(
            partition_errors(&created).is_empty(),
            "case {case}: the fixture partitions must be created, got Errors {:?}",
            created["Errors"]
        );

        // What a read serves for every partition the delete must leave, BEFORE it runs.
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

        let deleted = batch_delete(&router, &database, &table, &row.delete).await;
        assert!(
            partition_errors(&deleted).is_empty(),
            "case {case}: every addressed partition is stored, so the batch must report no \
             per-item error, got Errors {:?}",
            deleted["Errors"]
        );

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
        // the delete left — a deleted partition is gone for every reader, not only for the
        // single-partition one.
        let addressed: Vec<Vec<String>> = row
            .created
            .iter()
            .map(|input| vals(&values_of(input)))
            .collect();
        let answered = batch_read_values(&router, &database, &table, &addressed).await;
        let expected: Vec<Value> = row.kept.iter().map(|values| json!(values)).collect();
        assert_eq!(
            answered, expected,
            "case {case}: the batch read must answer exactly the partitions the delete left; \
             expected {expected:?}, got {answered:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Per-item failures
// ---------------------------------------------------------------------------

/// One row of the per-item failure table.
struct FailureCase {
    case: &'static str,
    /// The partition keys the addressed table defines.
    keys: Vec<&'static str>,
    /// The `PartitionInput`s the fixture creates.
    created: Vec<Value>,
    /// The value lists the batch addresses.
    delete: Vec<Vec<String>>,
    /// The `PartitionValues` of each `Errors` entry, in order.
    expected_values: Vec<Vec<String>>,
    /// Value lists a read must still serve after the batch.
    readable: Vec<Vec<String>>,
    /// Value lists a read must not serve after the batch.
    missing: Vec<Vec<String>>,
}

/// An item the table holds no partition for fails only its own item: the batch answers
/// 200, names that item in `Errors` as a partition miss, and deletes every other item of
/// the same batch.
///
/// The rows differ along the way one item fails — a value list nothing was created for,
/// the same partition addressed twice, fewer values than the table has partition keys,
/// more values than it has, a value differing from a stored one only in case, an item that
/// names no value at all — and along how many items of the batch succeed beside it.
#[tokio::test]
async fn an_item_the_table_holds_no_partition_for_fails_only_itself() {
    let router = emulator();

    let cases = [
        FailureCase {
            case: "a value list nothing was created for",
            keys: vec!["dt"],
            created: vec![json!({ "Values": ["2026-01-01"] })],
            delete: vec![vals(&["2026-01-09"])],
            expected_values: vec![vals(&["2026-01-09"])],
            readable: vec![vals(&["2026-01-01"])],
            missing: vec![vals(&["2026-01-09"])],
        },
        FailureCase {
            case: "the same partition addressed twice in one batch",
            keys: vec!["dt"],
            created: vec![json!({ "Values": ["2026-01-01"] })],
            delete: vec![vals(&["2026-01-01"]), vals(&["2026-01-01"])],
            expected_values: vec![vals(&["2026-01-01"])],
            readable: vec![],
            missing: vec![vals(&["2026-01-01"])],
        },
        FailureCase {
            case: "one miss beside two deletes",
            keys: vec!["dt"],
            created: vec![
                json!({ "Values": ["2026-01-01"] }),
                json!({ "Values": ["2026-01-02"] }),
                json!({ "Values": ["2026-01-03"] }),
            ],
            delete: vec![
                vals(&["2026-01-01"]),
                vals(&["2026-01-09"]),
                vals(&["2026-01-02"]),
            ],
            expected_values: vec![vals(&["2026-01-09"])],
            readable: vec![vals(&["2026-01-03"])],
            missing: vec![vals(&["2026-01-01"]), vals(&["2026-01-02"])],
        },
        FailureCase {
            case: "fewer values than the table has partition keys",
            keys: vec!["dt", "region"],
            created: vec![json!({ "Values": ["2026-01-01", "us-east-1"] })],
            delete: vec![vals(&["2026-01-01"])],
            expected_values: vec![vals(&["2026-01-01"])],
            readable: vec![vals(&["2026-01-01", "us-east-1"])],
            missing: vec![],
        },
        FailureCase {
            case: "more values than the table has partition keys",
            keys: vec!["dt"],
            created: vec![json!({ "Values": ["2026-01-01"] })],
            delete: vec![vals(&["2026-01-01", "us-east-1"])],
            expected_values: vec![vals(&["2026-01-01", "us-east-1"])],
            readable: vec![vals(&["2026-01-01"])],
            missing: vec![],
        },
        FailureCase {
            case: "a value differing from a stored one only in case",
            keys: vec!["k"],
            created: vec![json!({ "Values": ["Monday"] })],
            delete: vec![vals(&["monday"])],
            expected_values: vec![vals(&["monday"])],
            readable: vec![vals(&["Monday"])],
            missing: vec![vals(&["monday"])],
        },
        FailureCase {
            case: "an item that names no value at all",
            keys: vec!["dt"],
            created: vec![json!({ "Values": ["2026-01-01"] })],
            delete: vec![vals(&[])],
            expected_values: vec![vals(&[])],
            readable: vec![vals(&["2026-01-01"])],
            missing: vec![],
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let case = row.case;
        let database = format!("failing_delete_db_{index}");
        let table = format!("failing_delete_table_{index}");
        seed_table(&router, &database, &table, &row.keys).await;
        let created = batch_create(&router, &database, &table, &row.created).await;
        assert!(
            partition_errors(&created).is_empty(),
            "case {case}: the fixture partitions must be created, got Errors {:?}",
            created["Errors"]
        );

        let body = batch_delete(&router, &database, &table, &row.delete).await;
        let reported = partition_errors(&body);
        assert_eq!(
            reported.len(),
            row.expected_values.len(),
            "case {case}: expected {} per-item errors, got {:?}",
            row.expected_values.len(),
            body["Errors"]
        );
        for (position, (entry, expected_values)) in
            reported.iter().zip(row.expected_values.iter()).enumerate()
        {
            let detail = &entry["ErrorDetail"];
            assert_eq!(
                detail["ErrorCode"].as_str(),
                Some(MISS_CODE),
                "case {case}: error {position} must carry ErrorCode {MISS_CODE}, got {entry}"
            );
            assert_eq!(
                detail["ErrorMessage"].as_str(),
                Some(PARTITION_NOT_FOUND),
                "case {case}: error {position} must carry the partition miss wording, got {entry}"
            );
            assert_eq!(
                detail["__type"].as_str(),
                Some(ERROR_DETAIL_TAG),
                "case {case}: error {position} must carry the ErrorDetail type tag, got {entry}"
            );
            assert_eq!(
                entry["PartitionValues"],
                json!(expected_values),
                "case {case}: error {position} must name the values its item addressed, got \
                 {entry}"
            );
        }

        for values in &row.readable {
            let (status, read) = get_partition(&router, &database, &table, &as_refs(values)).await;
            assert_eq!(
                status, 200,
                "case {case}: the batch deleted no partition at {values:?}, so a read must still \
                 serve it; got {status} with {read}"
            );
        }
        for values in &row.missing {
            let (status, read) = get_partition(&router, &database, &table, &as_refs(values)).await;
            assert_eq!(
                status, 400,
                "case {case}: no partition must be served at {values:?}; got {status} with {read}"
            );
        }
    }
}

/// An empty `PartitionsToDelete` is a valid input (`BatchDeletePartitionValueList`,
/// length 0..25, from `smithy_query` on `BatchDeletePartition`): it deletes nothing and
/// reports no per-item error.
#[tokio::test]
async fn an_empty_batch_deletes_nothing_and_reports_no_errors() {
    let router = emulator();
    seed_table(&router, "empty_delete_db", "empty_delete_table", &["dt"]).await;
    batch_create(
        &router,
        "empty_delete_db",
        "empty_delete_table",
        &[json!({ "Values": ["2026-01-01"] })],
    )
    .await;

    let body = batch_delete(&router, "empty_delete_db", "empty_delete_table", &[]).await;
    assert!(
        partition_errors(&body).is_empty(),
        "an empty batch reports no error, got {body}"
    );
    let (status, read) = get_partition(
        &router,
        "empty_delete_db",
        "empty_delete_table",
        &["2026-01-01"],
    )
    .await;
    assert_eq!(
        status, 200,
        "an empty batch deletes no partition, so the stored one must still be served; got \
         {status} with {read}"
    );
}

// ---------------------------------------------------------------------------
// Whole-request refusals
// ---------------------------------------------------------------------------

/// A database or table the catalog does not hold fails the WHOLE batch — there is no
/// table to delete any partition from — with `EntityNotFoundException` and the wording
/// this service answers for that resource.
#[tokio::test]
async fn a_batch_naming_no_stored_table_is_refused_whole() {
    let router = emulator();
    seed_table(&router, "present_delete_db", "present_delete_table", &["dt"]).await;

    let cases = [
        (
            "no such database",
            "ghost_db",
            "present_delete_table",
            "Database ghost_db not found.",
        ),
        (
            "no such database, mixed case",
            "Ghost_DB",
            "present_delete_table",
            "Database ghost_db not found.",
        ),
        (
            "no such table",
            "present_delete_db",
            "ghost_table",
            "Table ghost_table not found.",
        ),
        (
            "no such table, mixed case",
            "present_delete_db",
            "Ghost_Table",
            "Table ghost_table not found.",
        ),
    ];

    for (case, database, table, expected_message) in cases {
        assert_refusal(
            &router,
            case,
            "BatchDeletePartition",
            &request(database, table, &[vals(&["2026-01-01"])]),
            "EntityNotFoundException",
            expected_message,
        )
        .await;
    }
}

/// A batch addressing more partitions than the model admits
/// (`BatchDeletePartitionValueList`, length 0..25, the delete batch's own ceiling) is
/// refused before any of them is
/// deleted: the request is past the modelled ceiling, so nothing it names is removed.
#[tokio::test]
async fn a_batch_past_the_modelled_ceiling_deletes_nothing() {
    let router = emulator();
    seed_table(&router, "over_delete_db", "over_delete_table", &["n"]).await;
    let inputs: Vec<Value> = (0..=MAX_DELETES_PER_BATCH)
        .map(|n| json!({ "Values": [n.to_string()] }))
        .collect();
    let created = batch_create(&router, "over_delete_db", "over_delete_table", &inputs).await;
    assert!(
        partition_errors(&created).is_empty(),
        "the fixture partitions must be created, got Errors {:?}",
        created["Errors"]
    );

    let over: Vec<Vec<String>> = (0..=MAX_DELETES_PER_BATCH)
        .map(|n| vec![n.to_string()])
        .collect();
    let (status, body) = call(
        &router,
        "BatchDeletePartition",
        &request("over_delete_db", "over_delete_table", &over),
    )
    .await;
    assert_eq!(
        status, 400,
        "a batch of {} value lists is past the ceiling of {MAX_DELETES_PER_BATCH} and must be \
         refused, got {status} with {body}",
        over.len()
    );
    for values in &over {
        let (status, read) = get_partition(
            &router,
            "over_delete_db",
            "over_delete_table",
            &as_refs(values),
        )
        .await;
        assert_eq!(
            status, 200,
            "a refused batch must delete nothing, so {values:?} must still be served; got \
             {status} with {read}"
        );
    }
}
