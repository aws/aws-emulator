// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue partition-index slice: the `PartitionIndexes` a
//! `CreateTable` registers and the `GetPartitionIndexes` that reports them.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler. Only the suite that
//! positions an index's DUE INSTANT reaches into the store, for the same reason
//! [`crate::owed_status_materialisation`] does: a wire call is served at whatever instant it
//! reaches the service, so the side of the due instant a read lands on is chosen in the
//! store rather than raced for against the wall clock.
//!
//! The rule under test: a partition index is a resource OF one table — "the partition
//! indexes associated with a table" (`GetPartitionIndexes`) — so a read is addressed through
//! a stored table (an absent database or table is `EntityNotFoundException`) and reports the
//! indexes THAT table was registered with, each one carrying the keys it indexes with the
//! type the table's own partition key declares, in `IndexName` order, and the status its
//! registration's due instant says. An index may only name keys the table partitions by, an
//! index name addresses one index of a table, and a table holds at most
//! `MAX_INDEXES_PER_TABLE` of them; a request that breaks any of those is refused and stores
//! nothing at all.
//!
//! The rows differ along what the rule generalises over: how many indexes a table is
//! registered with (none, one, two, the maximum), how many keys one index names and in which
//! order, which of the table's partition keys are indexed and what types they declare, which
//! part of a read's address is missing, which part of a submitted index is invalid, and where
//! a continuation token sits in the index order.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::KeySchemaElement;
use service_com_amazonaws_glue_awsglue::service::partition_indexes::{
    messages as index_messages, PartitionIndexEntry, PartitionIndexStore, TableIndexKey,
    INDEX_STATUS_ACTIVE, INDEX_STATUS_CREATING, MAX_INDEXES_PER_TABLE,
};
use service_com_amazonaws_glue_awsglue::state::{fold_name, messages, ComAmazonawsGlueAwsglueState};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::{already_past, far_future};
use crate::wire::{call, emulator, emulator_on, error_code, error_message, ok, ACCOUNT_ID};

/// How many times a positioned record is read: more than once, so a read that changed what
/// the next read reports is caught.
const READS: usize = 3;

/// ONE index as a read reports it: its `IndexName`, its `Keys` as (name, type) pairs in the
/// order reported, and its `IndexStatus`.
pub(crate) type ReportedIndex = (String, Vec<(String, String)>, String);

/// ONE index a row expects: its `IndexName` and its keys as (name, type) pairs.
type ExpectedIndex = (&'static str, Vec<(&'static str, &'static str)>);

/// One row of the registration table: the case, the table's `PartitionKeys`, the submitted
/// `PartitionIndexes`, and the indexes the read must report, in report order.
type RegistrationCase = (&'static str, Value, Value, Vec<ExpectedIndex>);

/// One row of the status table: the case, where the due instant sits when the reads are made,
/// and the status the index must report there.
type Position = (&'static str, fn() -> SystemTime, &'static str);

/// One row of the refusal table: the case, the table's `PartitionKeys`, the submitted
/// `PartitionIndexes`, the modelled error expected, and the message it must carry, built from
/// the table the row creates.
type RefusalCase = (&'static str, Value, Value, &'static str, fn(&str) -> String);

/// Create `database` holding `table`, whose definition declares `partition_keys` and which is
/// registered with `indexes`, and answer the create's HTTP status and body.
///
/// Shared with [`crate::partition_index_registration`], which registers indexes on the table
/// this creates rather than with it.
pub(crate) async fn create_indexed_table(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    partition_keys: &Value,
    indexes: Option<&Value>,
) -> (u16, Value) {
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database } }),
    )
    .await;
    let mut body = json!({
        "DatabaseName": database,
        "TableInput": {
            "Name": table,
            "StorageDescriptor": {
                "Location": "s3://lake/events/",
                "Columns": [ { "Name": "payload", "Type": "string" } ]
            },
            "PartitionKeys": partition_keys
        }
    });
    if let Some(indexes) = indexes {
        body["PartitionIndexes"] = indexes.clone();
    }
    call(router, "CreateTable", &body).await
}

/// One index read, answering the HTTP status and body.
pub(crate) async fn get_indexes(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({
        "CatalogId": ACCOUNT_ID,
        "DatabaseName": database,
        "TableName": table
    });
    if let Some(token) = next_token {
        body["NextToken"] = json!(token);
    }
    call(router, "GetPartitionIndexes", &body).await
}

/// The reported list as `(IndexName, [(key name, key type)], IndexStatus)` triples, in the
/// order the answer carries them.
pub(crate) fn reported(body: &Value) -> Vec<ReportedIndex> {
    body["PartitionIndexDescriptorList"]
        .as_array()
        .unwrap_or_else(|| panic!("the answer must carry a PartitionIndexDescriptorList: {body}"))
        .iter()
        .map(|descriptor| {
            let keys = descriptor["Keys"]
                .as_array()
                .unwrap_or_else(|| panic!("a descriptor must carry its Keys: {descriptor}"))
                .iter()
                .map(|key| {
                    (
                        key["Name"].as_str().unwrap_or_default().to_string(),
                        key["Type"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            (
                descriptor["IndexName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                keys,
                descriptor["IndexStatus"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// The index names a read reports, in the order it reports them.
pub(crate) fn reported_names(body: &Value) -> Vec<String> {
    reported(body).into_iter().map(|(name, _, _)| name).collect()
}

// ---------------------------------------------------------------------------
// The indexes a create registers are the indexes the read reports
// ---------------------------------------------------------------------------

/// A table registered with partition indexes reports exactly those indexes, each with the
/// keys it names in the order it named them and the type the table's own partition key
/// declares, in `IndexName` order — and the table itself is unchanged by any of it.
///
/// The rows differ along how many indexes the table carries (one, two, the maximum), how many
/// keys one index names, the order those keys are named in against the partition-key order,
/// which of the table's partition keys are indexed, and what type each declares — including a
/// partition key that declares none.
#[tokio::test]
async fn a_table_reports_the_partition_indexes_its_create_registered() {
    // (case, the table's PartitionKeys, the submitted PartitionIndexes, the expected
    //  (IndexName, [(key, type)]) pairs in the order they must be reported)
    let cases: Vec<RegistrationCase> = vec![
        (
            "one index on the single partition key",
            json!([ { "Name": "dt", "Type": "string" } ]),
            json!([ { "IndexName": "dt_idx", "Keys": [ "dt" ] } ]),
            vec![("dt_idx", vec![("dt", "string")])],
        ),
        (
            "one index on two keys, in the partition-key order",
            json!([ { "Name": "year", "Type": "int" }, { "Name": "month", "Type": "int" } ]),
            json!([ { "IndexName": "ym_idx", "Keys": [ "year", "month" ] } ]),
            vec![("ym_idx", vec![("year", "int"), ("month", "int")])],
        ),
        (
            "one index on two keys, in the reverse of the partition-key order",
            json!([ { "Name": "year", "Type": "int" }, { "Name": "month", "Type": "int" } ]),
            json!([ { "IndexName": "my_idx", "Keys": [ "month", "year" ] } ]),
            vec![("my_idx", vec![("month", "int"), ("year", "int")])],
        ),
        (
            "two indexes, registered in the reverse of the name order",
            json!([
                { "Name": "region", "Type": "string" },
                { "Name": "dt", "Type": "date" }
            ]),
            json!([
                { "IndexName": "region_dt_idx", "Keys": [ "region", "dt" ] },
                { "IndexName": "dt_idx", "Keys": [ "dt" ] }
            ]),
            vec![
                ("dt_idx", vec![("dt", "date")]),
                ("region_dt_idx", vec![("region", "string"), ("dt", "date")]),
            ],
        ),
        (
            "the maximum number of indexes a table holds",
            json!([
                { "Name": "region", "Type": "string" },
                { "Name": "dt", "Type": "date" },
                { "Name": "hour", "Type": "int" }
            ]),
            json!([
                { "IndexName": "c_idx", "Keys": [ "hour" ] },
                { "IndexName": "a_idx", "Keys": [ "region", "dt", "hour" ] },
                { "IndexName": "b_idx", "Keys": [ "dt", "region" ] }
            ]),
            vec![
                (
                    "a_idx",
                    vec![("region", "string"), ("dt", "date"), ("hour", "int")],
                ),
                ("b_idx", vec![("dt", "date"), ("region", "string")]),
                ("c_idx", vec![("hour", "int")]),
            ],
        ),
        (
            "an index on a partition key that declares no type",
            json!([ { "Name": "untyped" } ]),
            json!([ { "IndexName": "untyped_idx", "Keys": [ "untyped" ] } ]),
            vec![("untyped_idx", vec![("untyped", "")])],
        ),
    ];

    assert_eq!(
        MAX_INDEXES_PER_TABLE, 3,
        "the maximum row above registers three indexes, which is the per-table maximum"
    );

    for (index, (case, partition_keys, indexes, expected)) in cases.iter().enumerate() {
        let router = emulator();
        let database = format!("index_db_{index}");
        let table = format!("index_table_{index}");
        let (status, body) =
            create_indexed_table(&router, &database, &table, partition_keys, Some(indexes)).await;
        assert_eq!(
            status, 200,
            "case {case}: the create must be served, got {status} with {body}"
        );

        let (status, read) = get_indexes(&router, &database, &table, None).await;
        assert_eq!(
            status, 200,
            "case {case}: the registered indexes must be readable, got {status} with {read}"
        );
        let expected_list: Vec<ReportedIndex> = expected
            .iter()
            .map(|(name, keys)| {
                (
                    (*name).to_string(),
                    keys.iter()
                        .map(|(key, key_type)| ((*key).to_string(), (*key_type).to_string()))
                        .collect(),
                    // The create put every index in the status the service builds one in; the
                    // span before the clock owes it ACTIVE is what makes this read observe
                    // it.
                    INDEX_STATUS_CREATING.to_string(),
                )
            })
            .collect();
        assert_eq!(
            reported(&read),
            expected_list,
            "case {case}: expected {expected_list:?}, got {:?} in {read}",
            reported(&read)
        );
        assert!(
            read["NextToken"].is_null(),
            "case {case}: a table's whole index list fits one page, so no token may be handed \
             back, got {read}"
        );
        for descriptor in read["PartitionIndexDescriptorList"]
            .as_array()
            .unwrap_or(&Vec::new())
        {
            assert!(
                descriptor["BackfillErrors"].is_null(),
                "case {case}: an index registered with its table backfills nothing, so no \
                 BackfillErrors may be reported, got {descriptor}"
            );
        }

        // The table the indexes belong to is exactly the table the create stored.
        let table_read = ok(
            &router,
            "GetTable",
            &json!({ "DatabaseName": database, "Name": table }),
        )
        .await;
        assert_eq!(
            table_read["Table"]["PartitionKeys"], *partition_keys,
            "case {case}: registering indexes must leave the table's partition keys as sent, \
             got {table_read}"
        );
    }
}

// ---------------------------------------------------------------------------
// A table nothing indexed holds no index
// ---------------------------------------------------------------------------

/// A table no index was registered on reports an EMPTY list, not an absent one and not an
/// error: the table exists, and it holds no index.
///
/// The rows differ along why the table holds none: a create that named no `PartitionIndexes`
/// at all, one that named an empty list, and a table that partitions by nothing.
#[tokio::test]
async fn a_table_no_index_was_registered_on_reports_an_empty_list() {
    // (case, the table's PartitionKeys, the submitted PartitionIndexes)
    let cases: Vec<(&str, Value, Option<Value>)> = vec![
        (
            "a partitioned table whose create named no PartitionIndexes",
            json!([ { "Name": "dt", "Type": "string" } ]),
            None,
        ),
        (
            "a partitioned table whose create named an empty PartitionIndexes",
            json!([ { "Name": "dt", "Type": "string" } ]),
            Some(json!([])),
        ),
        ("a table that partitions by nothing", json!([]), None),
    ];

    for (index, (case, partition_keys, indexes)) in cases.iter().enumerate() {
        let router = emulator();
        let database = format!("empty_db_{index}");
        let table = format!("empty_table_{index}");
        let (status, body) = create_indexed_table(
            &router,
            &database,
            &table,
            partition_keys,
            indexes.as_ref(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the create must be served, got {status} with {body}"
        );

        let (status, read) = get_indexes(&router, &database, &table, None).await;
        assert_eq!(
            status, 200,
            "case {case}: a stored table must answer its index list, got {status} with {read}"
        );
        assert_eq!(
            read["PartitionIndexDescriptorList"],
            json!([]),
            "case {case}: the list must be present and empty, got {read}"
        );
        assert!(
            read["NextToken"].is_null(),
            "case {case}: an empty list ends the listing, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// The read is addressed through a stored table
// ---------------------------------------------------------------------------

/// A read whose address does not resolve to a stored table is `EntityNotFoundException`,
/// naming the entity that is missing.
///
/// The rows differ along which part of the address is missing — the database, the table, and
/// a table whose name belongs to another database — and are made both with and without a
/// continuation token, which cannot turn a miss into an answer.
#[tokio::test]
async fn a_read_through_a_table_the_catalog_does_not_hold_is_not_found() {
    let router = emulator();
    create_indexed_table(
        &router,
        "held_db",
        "events",
        &json!([ { "Name": "dt", "Type": "string" } ]),
        Some(&json!([ { "IndexName": "dt_idx", "Keys": [ "dt" ] } ])),
    )
    .await;
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": "other_db" } }),
    )
    .await;

    // (case, the database, the table, the message the refusal must carry)
    let misses: Vec<(&str, &str, &str, String)> = vec![
        (
            "a database the catalog does not hold",
            "no_such_db",
            "events",
            messages::database_not_found("no_such_db"),
        ),
        (
            "a table the database does not hold",
            "held_db",
            "no_such_table",
            messages::table_not_found("no_such_table"),
        ),
        (
            "a table held by another database",
            "other_db",
            "events",
            messages::table_not_found("events"),
        ),
    ];

    for (case, database, table, message) in &misses {
        for token in [None, Some("dt_idx")] {
            let (status, body) = get_indexes(&router, database, table, token).await;
            assert_eq!(
                status, 400,
                "case {case} (token {token:?}): an index read needs a stored table, got {status} \
                 with {body}"
            );
            assert!(
                error_code(&body).ends_with("EntityNotFoundException"),
                "case {case} (token {token:?}): expected EntityNotFoundException, got {} in \
                 {body}",
                error_code(&body)
            );
            assert_eq!(
                error_message(&body),
                *message,
                "case {case} (token {token:?}): expected {message:?}, got {:?}",
                error_message(&body)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What a table cannot be registered with
// ---------------------------------------------------------------------------

/// A create whose `PartitionIndexes` the table cannot carry is REFUSED, and stores nothing at
/// all: neither the table nor any of its indexes.
///
/// The rows differ along what makes the submission impossible — a key that is a column but
/// not a partition key, a key that is no column of the table, a table that partitions by
/// nothing, one bad key among good ones, a long key list, a name used twice, a key named
/// twice, and one index more than a table holds — and so along which modelled error answers
/// it.
#[tokio::test]
async fn a_create_naming_indexes_a_table_cannot_carry_is_refused() {
    // (case, the table's PartitionKeys, the submitted PartitionIndexes, the expected error,
    //  the message it must carry)
    let cases: Vec<RefusalCase> = vec![
        (
            "a key that is a column of the table but not a partition key",
            json!([ { "Name": "dt", "Type": "string" } ]),
            json!([ { "IndexName": "payload_idx", "Keys": [ "payload" ] } ]),
            "InvalidInputException",
            // The expected sentence is spelled out here, not taken from the builder the
            // handler uses, so this row pins the wording and not just its substitution.
            |table| {
                format!(
                    "Index key payload of partition index payload_idx is not a partition key of \
                     table {table}"
                )
            },
        ),
        (
            "a key that is no column of the table at all",
            json!([ { "Name": "dt", "Type": "string" } ]),
            json!([ { "IndexName": "ghost_idx", "Keys": [ "nowhere" ] } ]),
            "InvalidInputException",
            |table| index_messages::key_is_not_a_partition_key(table, "ghost_idx", "nowhere"),
        ),
        (
            "an index on a table that partitions by nothing",
            json!([]),
            json!([ { "IndexName": "dt_idx", "Keys": [ "dt" ] } ]),
            "InvalidInputException",
            |table| index_messages::key_is_not_a_partition_key(table, "dt_idx", "dt"),
        ),
        (
            "one key of two that is not a partition key",
            json!([ { "Name": "year", "Type": "int" } ]),
            json!([ { "IndexName": "ym_idx", "Keys": [ "year", "month" ] } ]),
            "InvalidInputException",
            |table| index_messages::key_is_not_a_partition_key(table, "ym_idx", "month"),
        ),
        (
            "one index name used twice",
            json!([ { "Name": "year", "Type": "int" }, { "Name": "month", "Type": "int" } ]),
            json!([
                { "IndexName": "dup_idx", "Keys": [ "year" ] },
                { "IndexName": "dup_idx", "Keys": [ "month" ] }
            ]),
            "InvalidInputException",
            // Spelled out, as on the first row: this pins the wording of a repeated name.
            |table| format!("Partition index dup_idx is named more than once for table {table}"),
        ),
        (
            "one key named twice inside one index",
            json!([ { "Name": "year", "Type": "int" } ]),
            json!([ { "IndexName": "yy_idx", "Keys": [ "year", "year" ] } ]),
            "InvalidInputException",
            // Spelled out, as on the first row: this pins the wording of a repeated key.
            |table| {
                format!(
                    "Index key year is named more than once in partition index yy_idx of table \
                     {table}"
                )
            },
        ),
        (
            "a key list far longer than the table's partition keys, refused at its first key",
            json!([ { "Name": "dt", "Type": "string" } ]),
            json!([ {
                "IndexName": "long_idx",
                "Keys": (0..256).map(|key| format!("ghost_{key}")).collect::<Vec<String>>()
            } ]),
            "InvalidInputException",
            |table| index_messages::key_is_not_a_partition_key(table, "long_idx", "ghost_0"),
        ),
        (
            "one index more than a table holds",
            json!([
                { "Name": "a", "Type": "string" },
                { "Name": "b", "Type": "string" },
                { "Name": "c", "Type": "string" },
                { "Name": "d", "Type": "string" }
            ]),
            json!([
                { "IndexName": "a_idx", "Keys": [ "a" ] },
                { "IndexName": "b_idx", "Keys": [ "b" ] },
                { "IndexName": "c_idx", "Keys": [ "c" ] },
                { "IndexName": "d_idx", "Keys": [ "d" ] }
            ]),
            "ResourceNumberLimitExceededException",
            // Spelled out, as on the first row: this pins the wording of the limit, with the
            // limit itself and the number submitted in it.
            |table| {
                format!(
                    "Table {table} cannot have more than 3 partition indexes, and 4 were submitted"
                )
            },
        ),
    ];

    for (index, (case, partition_keys, indexes, expected, expected_message)) in
        cases.iter().enumerate()
    {
        let router = emulator();
        let database = format!("refused_db_{index}");
        let table = format!("refused_table_{index}");
        let (status, body) =
            create_indexed_table(&router, &database, &table, partition_keys, Some(indexes)).await;
        assert_eq!(
            status, 400,
            "case {case}: the create must be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with(expected),
            "case {case}: expected {expected}, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            expected_message(&table),
            "case {case}: expected {:?}, got {:?}",
            expected_message(&table),
            error_message(&body)
        );

        // Nothing was stored: the table is a miss, and so is its index list.
        let (status, read) = call(
            &router,
            "GetTable",
            &json!({ "DatabaseName": database, "Name": table }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: the refused create must store no table, got {status} with {read}"
        );
        let (status, read) = get_indexes(&router, &database, &table, None).await;
        assert_eq!(
            status, 400,
            "case {case}: the refused create must register no index, got {status} with {read}"
        );
        assert!(
            error_code(&read).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException from the read, got {}",
            error_code(&read)
        );
    }
}

// ---------------------------------------------------------------------------
// The indexes of one table are the indexes of THAT table
// ---------------------------------------------------------------------------

/// Each table answers its own indexes: two tables of one database, and two tables of the same
/// name in different databases, each report what their own create registered — and a table
/// deleted and created again under the same name reports none, because the indexes belonged
/// to the table that was dropped.
#[tokio::test]
async fn indexes_belong_to_the_table_they_were_registered_on() {
    let router = emulator();
    create_indexed_table(
        &router,
        "first_db",
        "events",
        &json!([ { "Name": "dt", "Type": "string" } ]),
        Some(&json!([ { "IndexName": "dt_idx", "Keys": [ "dt" ] } ])),
    )
    .await;
    // A second table of the SAME database, with its own index.
    ok(
        &router,
        "CreateTable",
        &json!({
            "DatabaseName": "first_db",
            "TableInput": {
                "Name": "sessions",
                "PartitionKeys": [ { "Name": "region", "Type": "string" } ]
            },
            "PartitionIndexes": [ { "IndexName": "region_idx", "Keys": [ "region" ] } ]
        }),
    )
    .await;
    // A table of the same NAME in another database, with no index at all.
    create_indexed_table(
        &router,
        "second_db",
        "events",
        &json!([ { "Name": "dt", "Type": "string" } ]),
        None,
    )
    .await;

    // (case, the database, the table, the index names it must report)
    let expectations: [(&str, &str, &str, Vec<&str>); 3] = [
        ("the indexed table", "first_db", "events", vec!["dt_idx"]),
        (
            "its neighbour in the same database",
            "first_db",
            "sessions",
            vec!["region_idx"],
        ),
        (
            "the same table name in another database",
            "second_db",
            "events",
            vec![],
        ),
    ];
    for (case, database, table, names) in &expectations {
        let (status, read) = get_indexes(&router, database, table, None).await;
        assert_eq!(
            status, 200,
            "case {case}: the read must be served, got {status} with {read}"
        );
        let expected: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            reported_names(&read),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            reported_names(&read)
        );
    }

    // The table the indexes belonged to is dropped and created again under the same name: the
    // new table is a different table, so it carries no index.
    ok(
        &router,
        "DeleteTable",
        &json!({ "DatabaseName": "first_db", "Name": "events" }),
    )
    .await;
    ok(
        &router,
        "CreateTable",
        &json!({
            "DatabaseName": "first_db",
            "TableInput": {
                "Name": "events",
                "PartitionKeys": [ { "Name": "dt", "Type": "string" } ]
            }
        }),
    )
    .await;
    let (status, read) = get_indexes(&router, "first_db", "events", None).await;
    assert_eq!(
        status, 200,
        "case the recreated table: the read must be served, got {status} with {read}"
    );
    assert_eq!(
        read["PartitionIndexDescriptorList"],
        json!([]),
        "case the recreated table: the dropped table's indexes must not be reported for it, got \
         {read}"
    );
}

// ---------------------------------------------------------------------------
// An update of the database around the table changes nothing about its indexes
// ---------------------------------------------------------------------------

/// A database update leaves the indexes of the tables it holds exactly where they are: an
/// index belongs to its TABLE, so moving the database around it — including renaming it,
/// which moves its tables to the new name — changes neither the table nor its indexes.
///
/// The rows differ along whether the update renames the database, and so along the name the
/// indexes are reachable under afterwards.
#[tokio::test]
async fn an_update_of_the_database_keeps_its_tables_indexes() {
    // (case, the name the update submits, the database the indexes read back under)
    let cases: [(&str, &str, &str); 2] = [
        ("an update that renames the database", "renamed_db", "renamed_db"),
        ("an update that keeps the name", "updated_db", "updated_db"),
    ];

    for (case, submitted, reachable) in &cases {
        let router = emulator();
        create_indexed_table(
            &router,
            "updated_db",
            "events",
            &json!([ { "Name": "dt", "Type": "string" } ]),
            Some(&json!([ { "IndexName": "dt_idx", "Keys": [ "dt" ] } ])),
        )
        .await;

        ok(
            &router,
            "UpdateDatabase",
            &json!({
                "Name": "updated_db",
                "DatabaseInput": { "Name": submitted, "Description": "updated" }
            }),
        )
        .await;

        let (status, read) = get_indexes(&router, reachable, "events", None).await;
        assert_eq!(
            status, 200,
            "case {case}: the table must still answer its indexes, got {status} with {read}"
        );
        assert_eq!(
            reported_names(&read),
            vec!["dt_idx".to_string()],
            "case {case}: expected the registered index under {reachable}, got {:?}",
            reported_names(&read)
        );
    }
}

// ---------------------------------------------------------------------------
// The records go when the table goes
// ---------------------------------------------------------------------------
/// A delete that drops a table drops the table's partition-index records with it, so the
/// store keeps nothing for a table the catalog no longer holds.
///
/// The rows differ along which delete drops the table: the single-table delete, the batch
/// delete, and the cascade of dropping the database that holds it. The records are inspected
/// in the store, because a dropped table cannot be read back over the wire — that is the
/// point of the delete.
#[tokio::test]
async fn a_delete_takes_the_tables_partition_indexes_with_it() {
    // (case, the operation that drops the table, its request body)
    let cases: [(&str, &str, Value); 3] = [
        (
            "the single-table delete",
            "DeleteTable",
            json!({ "DatabaseName": "dropped_db", "Name": "events" }),
        ),
        (
            "the batch delete",
            "BatchDeleteTable",
            json!({ "DatabaseName": "dropped_db", "TablesToDelete": [ "events" ] }),
        ),
        (
            "the cascade of dropping its database",
            "DeleteDatabase",
            json!({ "Name": "dropped_db" }),
        ),
    ];

    for (case, operation, request) in &cases {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(Arc::clone(&engine));
        create_indexed_table(
            &router,
            "dropped_db",
            "events",
            &json!([ { "Name": "dt", "Type": "string" } ]),
            Some(&json!([ { "IndexName": "dt_idx", "Keys": [ "dt" ] } ])),
        )
        .await;

        // The key the create recorded against, taken from the stored table exactly as the
        // read path takes it.
        let indexed_table = {
            let state = engine.state::<ComAmazonawsGlueAwsglueState>();
            let store = state.lock();
            let table = store
                .database(ACCOUNT_ID, "dropped_db")
                .and_then(|database| database.tables.get(&fold_name("events")))
                .unwrap_or_else(|| panic!("case {case}: the table was created"));
            TableIndexKey::of(table)
        };
        let records = engine.state::<PartitionIndexStore>();
        let before = records
            .lock()
            .descriptors(&indexed_table, SystemTime::now())
            .len();
        assert_eq!(
            before, 1,
            "case {case}: the create must have registered one index, got {before}"
        );

        ok(&router, operation, request).await;

        let after = records
            .lock()
            .descriptors(&indexed_table, SystemTime::now())
            .len();
        assert_eq!(
            after, 0,
            "case {case}: {operation} must take the table's index records with it, got {after} \
             record(s) still held"
        );
        // And the table itself is gone, which is what makes those records unreachable.
        let (status, read) = get_indexes(&router, "dropped_db", "events", None).await;
        assert_eq!(
            status, 400,
            "case {case}: the dropped table must be a miss, got {status} with {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// The continuation token
// ---------------------------------------------------------------------------

/// A continuation token names the `IndexName` a page ended at, so the answer resumes at the
/// first index strictly after it in the order the listing reports.
///
/// The rows differ along where the token sits in that order: before every name, at each name
/// in turn, and at the last of them.
#[tokio::test]
async fn a_continuation_token_resumes_after_the_index_it_names() {
    let router = emulator();
    create_indexed_table(
        &router,
        "paged_db",
        "events",
        &json!([
            { "Name": "a", "Type": "string" },
            { "Name": "b", "Type": "string" },
            { "Name": "c", "Type": "string" }
        ]),
        Some(&json!([
            { "IndexName": "a_idx", "Keys": [ "a" ] },
            { "IndexName": "b_idx", "Keys": [ "b" ] },
            { "IndexName": "c_idx", "Keys": [ "c" ] }
        ])),
    )
    .await;

    // (case, the token, the names the answer must carry)
    let cases: [(&str, Option<&str>, Vec<&str>); 5] = [
        ("no token", None, vec!["a_idx", "b_idx", "c_idx"]),
        (
            "a token sorting before every index",
            Some("0"),
            vec!["a_idx", "b_idx", "c_idx"],
        ),
        ("the first index", Some("a_idx"), vec!["b_idx", "c_idx"]),
        ("the second index", Some("b_idx"), vec!["c_idx"]),
        ("the last index", Some("c_idx"), vec![]),
    ];

    for (case, token, names) in &cases {
        let (status, read) = get_indexes(&router, "paged_db", "events", *token).await;
        assert_eq!(
            status, 200,
            "case {case}: the read must be served, got {status} with {read}"
        );
        let expected: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            reported_names(&read),
            expected,
            "case {case}: expected {expected:?}, got {:?} in {read}",
            reported_names(&read)
        );
        assert!(
            read["NextToken"].is_null(),
            "case {case}: the whole list fits one page, so no token may be handed back, got \
             {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// The status is the one the clock owes the index
// ---------------------------------------------------------------------------

/// The `IndexStatus` a read reports is a function of the DUE instant the registration
/// recorded and the instant of the call: the status the service builds an index in while that
/// instant is ahead of the clock, and the status the clock owes it from the instant it is
/// reached. Every read of one index reports the same status, however many times it is read.
///
/// The rows POSITION that due instant rather than leaving a margin against the wall clock,
/// which is what makes the side of it a read lands on the row's own choice and not the
/// scheduler's.
#[tokio::test]
async fn the_status_an_index_reports_is_the_one_its_due_instant_says() {
    // (case, where the due instant sits when the reads are made, the status it must report)
    let positions: [Position; 3] = [
        (
            "an hour ahead of the clock",
            far_future,
            INDEX_STATUS_CREATING,
        ),
        (
            "at the instant it was positioned",
            SystemTime::now,
            INDEX_STATUS_ACTIVE,
        ),
        (
            "a second behind the clock",
            already_past,
            INDEX_STATUS_ACTIVE,
        ),
    ];

    for (case, due_at, expected) in &positions {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(Arc::clone(&engine));
        create_indexed_table(
            &router,
            "settled_db",
            "events",
            &json!([ { "Name": "dt", "Type": "string" } ]),
            None,
        )
        .await;

        // The record is written against the STORED table, whose own key is what a read of it
        // resolves, and with its due instant positioned where this row wants it.
        let indexed_table = {
            let state = engine.state::<ComAmazonawsGlueAwsglueState>();
            let store = state.lock();
            let table = store
                .database(ACCOUNT_ID, "settled_db")
                .and_then(|database| database.tables.get(&fold_name("events")))
                .unwrap_or_else(|| panic!("case {case}: the table was created"));
            TableIndexKey::of(table)
        };
        engine.state::<PartitionIndexStore>().lock().record(
            indexed_table,
            vec![PartitionIndexEntry {
                index_name: "dt_idx".to_string(),
                keys: vec![KeySchemaElement {
                    name: "dt".to_string(),
                    r#type: "string".to_string(),
                }],
                active_at: due_at(),
                // An index whose build found nothing at fault is the one the clock owes
                // ACTIVE; what a FAILED build reports is asserted by the registration suite.
                backfill_errors: Vec::new(),
            }],
        );

        for read in 0..READS {
            let (status, body) = get_indexes(&router, "settled_db", "events", None).await;
            assert_eq!(
                status, 200,
                "case {case} (read {read}): the read must be served, got {status} with {body}"
            );
            assert_eq!(
                reported(&body),
                vec![(
                    "dt_idx".to_string(),
                    vec![("dt".to_string(), "string".to_string())],
                    (*expected).to_string()
                )],
                "case {case} (read {read}): expected the index {expected}, got {:?}",
                reported(&body)
            );
        }
    }
}
