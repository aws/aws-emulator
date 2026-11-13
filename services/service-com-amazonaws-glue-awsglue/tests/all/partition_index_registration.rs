// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreatePartitionIndex`: registering a partition index on a table the
//! catalog ALREADY holds, which is the one thing that distinguishes it from the indexes a
//! `CreateTable` is given — the table already holds partitions, so the registration has a
//! BACKFILL to report on.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler. The table a row registers
//! on is created by the same helper the `CreateTable` suite uses
//! ([`crate::partition_indexes::create_indexed_table`]), and read back through the same
//! reader ([`crate::partition_indexes::get_indexes`]), so both ways of registering an index
//! are asserted against one read.
//!
//! The rules under test:
//!
//! * a registration is addressed through a STORED table (an absent database or table is
//!   `EntityNotFoundException`) and registers an index of that table, reported by
//!   `GetPartitionIndexes` with the keys it named, each carrying the type the table's own
//!   partition key declares;
//! * an index may only name keys the table PARTITIONS by — a column of the table's
//!   `StorageDescriptor` is not one — an index name addresses one index of a table, and a
//!   table holds at most `MAX_INDEXES_PER_TABLE` of them; a request that breaks any of those
//!   is refused and registers nothing;
//! * the BUILD of an accepted index backfills over the partitions the table already holds: a
//!   stored partition whose value for an indexed key is not a value of the type that key
//!   declares, or which states no value for it, is reported in the index's `BackfillErrors`
//!   under the `BackfillErrorCode` that says which of the two it is — and an index with any
//!   such partition ends `FAILED` where one with none ends `ACTIVE`.
//!
//! The rows differ along what the rules generalise over: how many keys an index names and in
//! which order, which of the table's partition keys are indexed and what type each declares,
//! how many indexes the table already holds, which part of a request is impossible, and —
//! for the backfill — the declared TYPE of the indexed key (text, each integer width,
//! floating point, fixed point, boolean, date, timestamp, none at all), which value of it the
//! stored partitions carry, and whether the fault is a value of the wrong type or no value at
//! all.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::service::partition_indexes::{
    messages as index_messages, PartitionIndexStore, TableIndexKey,
    BACKFILL_INVALID_PARTITION_TYPE_DATA, BACKFILL_MISSING_PARTITION_VALUE, INDEX_STATUS_ACTIVE,
    INDEX_STATUS_CREATING, INDEX_STATUS_FAILED, MAX_BACKFILL_ERROR_PARTITIONS,
    MAX_INDEXES_PER_TABLE, PARTITION_INDEX_SETTLES_AFTER,
};
use service_com_amazonaws_glue_awsglue::state::{fold_name, messages, ComAmazonawsGlueAwsglueState};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::far_future;
use crate::partition_indexes::{create_indexed_table, get_indexes, reported, reported_names};
use crate::wire::{call, emulator, emulator_on, error_code, error_message, ok, ACCOUNT_ID};

/// One row of the registration table: the case, the table's `PartitionKeys`, the keys the
/// index names, and the `(name, type)` pairs the read must report for it.
type RegistrationCase = (
    &'static str,
    Value,
    Vec<&'static str>,
    Vec<(&'static str, &'static str)>,
);

/// One row of the refusal table: the case, the table's `PartitionKeys`, the index name and
/// the keys submitted, the modelled error expected, and the message it must carry, built from
/// the table the row creates.
type RefusalCase = (
    &'static str,
    Value,
    &'static str,
    Vec<&'static str>,
    &'static str,
    fn(&str) -> String,
);

/// One row of the duplicate table: the case, the index the table's own create registers (if
/// any), the keys the duplicate registration names, and the `(name, type)` pairs the held
/// index must still report.
type DuplicateCase = (
    &'static str,
    Option<Value>,
    Vec<&'static str>,
    Vec<(&'static str, &'static str)>,
);

/// One row of the backfill table.
struct BackfillCase {
    /// What this row varies.
    case: &'static str,
    /// The `PartitionKeys` the table declares when its partitions are stored.
    partition_keys: Value,
    /// The partitions the table already holds when the index is registered, by their
    /// `Values`.
    partitions: Vec<Vec<&'static str>>,
    /// The `PartitionKeys` the table is updated to AFTER those partitions are stored, or
    /// `None` when the table keeps the keys it was created with. A key the table gains this
    /// way is a key its stored partitions state no value for.
    repartitioned: Option<Value>,
    /// The keys the registered index names.
    index_keys: Vec<&'static str>,
    /// The `IndexStatus` the build is due.
    status: &'static str,
    /// The `BackfillErrors` it must report, as `(Code, [the Values of each partition])`.
    errors: Vec<(&'static str, Vec<Vec<&'static str>>)>,
}

/// Register one index on `table`, answering the HTTP status and body.
async fn register(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    index_name: &str,
    keys: &[&str],
) -> (u16, Value) {
    call(
        router,
        "CreatePartitionIndex",
        &json!({
            "CatalogId": ACCOUNT_ID,
            "DatabaseName": database,
            "TableName": table,
            "PartitionIndex": { "IndexName": index_name, "Keys": keys }
        }),
    )
    .await
}

/// Store one partition of `table` per value list, in the order given.
async fn create_partitions(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    partitions: &[Vec<&str>],
) {
    for values in partitions {
        ok(
            router,
            "CreatePartition",
            &json!({
                "DatabaseName": database,
                "TableName": table,
                "PartitionInput": { "Values": values }
            }),
        )
        .await;
    }
}

/// Update `table` to partition by `partition_keys`, leaving the partitions it already holds
/// where they are — so a key the table gains is a key those partitions state no value for.
async fn repartition(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    partition_keys: &Value,
) {
    ok(
        router,
        "UpdateTable",
        &json!({
            "DatabaseName": database,
            "TableInput": {
                "Name": table,
                "StorageDescriptor": {
                    "Location": "s3://lake/events/",
                    "Columns": [ { "Name": "payload", "Type": "string" } ]
                },
                "PartitionKeys": partition_keys
            }
        }),
    )
    .await;
}

/// The `(Code, [the Values of each named partition])` pairs one descriptor reports, in the
/// order the answer carries them. A descriptor reporting no `BackfillErrors` has none.
fn reported_backfill(descriptor: &Value) -> Vec<(String, Vec<Vec<String>>)> {
    descriptor["BackfillErrors"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|error| {
            let partitions = error["Partitions"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .map(|partition| {
                    partition["Values"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_default()
                        .iter()
                        .map(|value| value.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .collect();
            (
                error["Code"].as_str().unwrap_or_default().to_string(),
                partitions,
            )
        })
        .collect()
}

/// The one descriptor `table` holds, as the records themselves report it at `at` — the read
/// path's own projection, taken at an instant of the row's choosing rather than the wall
/// clock's, for the same reason [`crate::owed_status_materialisation`] reaches into the
/// store: whether the build is due is then the row's choice and not the scheduler's.
fn recorded_descriptor(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    database: &str,
    table: &str,
    at: SystemTime,
) -> Value {
    let indexed_table = {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let store = state.lock();
        let stored = store
            .database(ACCOUNT_ID, database)
            .and_then(|database| database.tables.get(&fold_name(table)))
            .unwrap_or_else(|| panic!("case {case}: the table was created"));
        TableIndexKey::of(stored)
    };
    let descriptors = engine
        .state::<PartitionIndexStore>()
        .lock()
        .descriptors(&indexed_table, at);
    assert_eq!(
        descriptors.len(),
        1,
        "case {case}: the registration must have recorded exactly one index, got {descriptors:?}"
    );
    let descriptor = &descriptors[0];
    json!({
        "IndexName": descriptor.index_name,
        "IndexStatus": descriptor.index_status,
        "BackfillErrors": descriptor.backfill_errors.as_ref().map(|errors| {
            errors
                .iter()
                .map(|error| {
                    json!({
                        "Code": error.code,
                        "Partitions": error.partitions.as_ref().map(|partitions| {
                            partitions
                                .iter()
                                .map(|partition| json!({ "Values": partition.values }))
                                .collect::<Vec<Value>>()
                        })
                    })
                })
                .collect::<Vec<Value>>()
        })
    })
}

/// `errors` as [`reported_backfill`] answers them, for comparing a row's expectation with
/// what a descriptor carries.
fn expected_backfill(
    errors: &[(&'static str, Vec<Vec<&'static str>>)],
) -> Vec<(String, Vec<Vec<String>>)> {
    errors
        .iter()
        .map(|(code, partitions)| {
            (
                (*code).to_string(),
                partitions
                    .iter()
                    .map(|values| values.iter().map(|value| (*value).to_string()).collect())
                    .collect(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// An index registered on a stored table is the index the read reports
// ---------------------------------------------------------------------------

/// A registration on a table the catalog holds is served, and the read reports exactly that
/// index: the name it was registered under, the keys it named in the order it named them,
/// each with the type the table's own partition key declares.
///
/// The rows differ along how many keys the index names, the order they are named in against
/// the partition-key order, which of the table's partition keys are indexed, and what type
/// each declares — including a partition key that declares none.
#[tokio::test]
async fn an_index_registered_on_a_stored_table_is_reported_by_the_read() {
    // (case, the table's PartitionKeys, the keys the index names, the (name, type) pairs the
    //  read must report)
    let cases: Vec<RegistrationCase> = vec![
        (
            "the single partition key",
            json!([ { "Name": "dt", "Type": "string" } ]),
            vec!["dt"],
            vec![("dt", "string")],
        ),
        (
            "two keys, in the partition-key order",
            json!([ { "Name": "year", "Type": "int" }, { "Name": "month", "Type": "int" } ]),
            vec!["year", "month"],
            vec![("year", "int"), ("month", "int")],
        ),
        (
            "two keys, in the reverse of the partition-key order",
            json!([ { "Name": "year", "Type": "int" }, { "Name": "month", "Type": "int" } ]),
            vec!["month", "year"],
            vec![("month", "int"), ("year", "int")],
        ),
        (
            "one key of the three the table partitions by",
            json!([
                { "Name": "region", "Type": "string" },
                { "Name": "dt", "Type": "date" },
                { "Name": "hour", "Type": "int" }
            ]),
            vec!["dt"],
            vec![("dt", "date")],
        ),
        (
            "every key of a table that partitions by three",
            json!([
                { "Name": "region", "Type": "string" },
                { "Name": "dt", "Type": "date" },
                { "Name": "hour", "Type": "int" }
            ]),
            vec!["region", "dt", "hour"],
            vec![("region", "string"), ("dt", "date"), ("hour", "int")],
        ),
        (
            "a partition key that declares no type",
            json!([ { "Name": "untyped" } ]),
            vec!["untyped"],
            vec![("untyped", "")],
        ),
    ];

    for (index, (case, partition_keys, keys, expected)) in cases.iter().enumerate() {
        let router = emulator();
        let database = format!("registered_db_{index}");
        let table = format!("registered_table_{index}");
        create_indexed_table(&router, &database, &table, partition_keys, None).await;

        let (status, body) = register(&router, &database, &table, "added_idx", keys).await;
        assert_eq!(
            status, 200,
            "case {case}: the registration must be served, got {status} with {body}"
        );
        assert_eq!(
            body,
            json!({}),
            "case {case}: the registration's answer carries no member of its own, got {body}"
        );

        let (status, read) = get_indexes(&router, &database, &table, None).await;
        assert_eq!(
            status, 200,
            "case {case}: the registered index must be readable, got {status} with {read}"
        );
        let expected_list = vec![(
            "added_idx".to_string(),
            expected
                .iter()
                .map(|(key, key_type)| ((*key).to_string(), (*key_type).to_string()))
                .collect(),
            // The registration put the index in the status the service builds one in; the
            // span before its build is due is what makes this read observe it.
            INDEX_STATUS_CREATING.to_string(),
        )];
        assert_eq!(
            reported(&read),
            expected_list,
            "case {case}: expected {expected_list:?}, got {:?} in {read}",
            reported(&read)
        );
        for descriptor in read["PartitionIndexDescriptorList"]
            .as_array()
            .unwrap_or(&Vec::new())
        {
            assert!(
                descriptor["BackfillErrors"].is_null(),
                "case {case}: an index still being built has reported no backfill result, got \
                 {descriptor}"
            );
        }

        // Registering an index changes nothing about the table it belongs to.
        let table_read = ok(
            &router,
            "GetTable",
            &json!({ "DatabaseName": database, "Name": table }),
        )
        .await;
        assert_eq!(
            table_read["Table"]["PartitionKeys"], *partition_keys,
            "case {case}: the table's partition keys must be left as its create sent them, got \
             {table_read}"
        );
    }
}

// ---------------------------------------------------------------------------
// A table takes indexes up to its maximum, beside the ones its create registered
// ---------------------------------------------------------------------------

/// Indexes registered one at a time join the ones the table's own create registered, up to
/// the maximum a table holds — and the registration past that maximum is refused, leaving the
/// indexes the table already had exactly as they were.
#[tokio::test]
async fn a_table_takes_indexes_up_to_its_maximum_and_refuses_one_past_it() {
    let router = emulator();
    create_indexed_table(
        &router,
        "filled_db",
        "events",
        &json!([
            { "Name": "a", "Type": "string" },
            { "Name": "b", "Type": "string" },
            { "Name": "c", "Type": "string" },
            { "Name": "d", "Type": "string" }
        ]),
        // The table arrives holding one index already, so what the registrations add is
        // counted against the indexes the create gave it and not from zero.
        Some(&json!([ { "IndexName": "a_idx", "Keys": [ "a" ] } ])),
    )
    .await;

    // (case, the index registered, the names the read must report afterwards)
    let added: Vec<(&str, &str, &str, Vec<&str>)> = vec![
        (
            "the second index of the table",
            "b_idx",
            "b",
            vec!["a_idx", "b_idx"],
        ),
        (
            "the last index the table holds",
            "c_idx",
            "c",
            vec!["a_idx", "b_idx", "c_idx"],
        ),
    ];
    for (case, index_name, key, expected) in &added {
        let (status, body) = register(&router, "filled_db", "events", index_name, &[key]).await;
        assert_eq!(
            status, 200,
            "case {case}: the registration must be served, got {status} with {body}"
        );
        let (_, read) = get_indexes(&router, "filled_db", "events", None).await;
        let expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            reported_names(&read),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            reported_names(&read)
        );
    }

    assert_eq!(
        MAX_INDEXES_PER_TABLE, 3,
        "the rows above fill the table to its maximum, which is three indexes"
    );
    let (status, body) = register(&router, "filled_db", "events", "d_idx", &["d"]).await;
    assert_eq!(
        status, 400,
        "case one index past the maximum: the registration must be refused, got {status} with \
         {body}"
    );
    assert!(
        error_code(&body).ends_with("ResourceNumberLimitExceededException"),
        "case one index past the maximum: expected ResourceNumberLimitExceededException, got {} \
         in {body}",
        error_code(&body)
    );
    assert_eq!(
        error_message(&body),
        index_messages::index_limit_reached("events", MAX_INDEXES_PER_TABLE),
        "case one index past the maximum: expected {:?}, got {:?}",
        index_messages::index_limit_reached("events", MAX_INDEXES_PER_TABLE),
        error_message(&body)
    );
    let (_, read) = get_indexes(&router, "filled_db", "events", None).await;
    assert_eq!(
        reported_names(&read),
        vec![
            "a_idx".to_string(),
            "b_idx".to_string(),
            "c_idx".to_string()
        ],
        "case one index past the maximum: the refused registration must add nothing, got {:?}",
        reported_names(&read)
    );
}

// ---------------------------------------------------------------------------
// A name the table already holds an index under
// ---------------------------------------------------------------------------

/// A registration naming an index the table already holds is a DUPLICATE, whichever
/// registration put that index there, and it leaves the index that is there alone.
///
/// The rows differ along how the index being duplicated was registered — with the table, or
/// by an earlier registration — and along whether the duplicate names the same keys.
#[tokio::test]
async fn a_registration_of_a_name_the_table_already_holds_is_a_duplicate() {
    // (case, the index the table starts with as CreateTable's PartitionIndexes, the keys the
    //  duplicate names, the keys the held index must still report)
    let cases: Vec<DuplicateCase> = vec![
        (
            "an index the table's own create registered",
            Some(json!([ { "IndexName": "held_idx", "Keys": [ "dt" ] } ])),
            vec!["dt"],
            vec![("dt", "string")],
        ),
        (
            "an index an earlier registration added",
            None,
            vec!["dt"],
            vec![("dt", "string")],
        ),
        (
            "a duplicate naming other keys than the index that is held",
            Some(json!([ { "IndexName": "held_idx", "Keys": [ "dt" ] } ])),
            vec!["region"],
            vec![("dt", "string")],
        ),
    ];

    for (index, (case, created_with, duplicate_keys, held_keys)) in cases.iter().enumerate() {
        let router = emulator();
        let database = format!("dup_db_{index}");
        create_indexed_table(
            &router,
            &database,
            "events",
            &json!([
                { "Name": "dt", "Type": "string" },
                { "Name": "region", "Type": "string" }
            ]),
            created_with.as_ref(),
        )
        .await;
        if created_with.is_none() {
            let (status, body) = register(&router, &database, "events", "held_idx", &["dt"]).await;
            assert_eq!(
                status, 200,
                "case {case}: the first registration must be served, got {status} with {body}"
            );
        }

        let (status, body) = register(&router, &database, "events", "held_idx", duplicate_keys).await;
        assert_eq!(
            status, 400,
            "case {case}: the duplicate must be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("AlreadyExistsException"),
            "case {case}: expected AlreadyExistsException, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            index_messages::PARTITION_INDEX_ALREADY_EXISTS,
            "case {case}: expected {:?}, got {:?}",
            index_messages::PARTITION_INDEX_ALREADY_EXISTS,
            error_message(&body)
        );

        // The index that is held is untouched: the duplicate replaced nothing.
        let (_, read) = get_indexes(&router, &database, "events", None).await;
        let expected = vec![(
            "held_idx".to_string(),
            held_keys
                .iter()
                .map(|(key, key_type)| ((*key).to_string(), (*key_type).to_string()))
                .collect(),
            INDEX_STATUS_CREATING.to_string(),
        )];
        assert_eq!(
            reported(&read),
            expected,
            "case {case}: expected the held index unchanged as {expected:?}, got {:?}",
            reported(&read)
        );
    }
}

// ---------------------------------------------------------------------------
// What a table cannot be registered with
// ---------------------------------------------------------------------------

/// A registration whose index the table cannot carry is REFUSED and registers nothing: the
/// keys of an index have to be keys the table PARTITIONS by, each named once.
///
/// The rows differ along what makes the submission impossible — a key that is a column of the
/// table but not a partition key, a key that is no column at all, a table that partitions by
/// nothing, one bad key among good ones, two bad keys at once, and a key named twice.
#[tokio::test]
async fn a_registration_naming_keys_the_table_cannot_index_is_refused() {
    // (case, the table's PartitionKeys, the index name, the keys submitted, the expected
    //  error, the message it must carry)
    let cases: Vec<RefusalCase> = vec![
        (
            "a key that is a column of the table but not a partition key",
            json!([ { "Name": "dt", "Type": "string" } ]),
            "payload_idx",
            vec!["payload"],
            "InvalidInputException",
            // The expected sentence is spelled out here, not taken from the builder the
            // handler uses, so this row pins the wording and not just its substitution.
            |_table| {
                "IndexKeys not a part of PartitionColumns. Verify the indexKeys : [payload]"
                    .to_string()
            },
        ),
        (
            "a key that is no column of the table at all",
            json!([ { "Name": "dt", "Type": "string" } ]),
            "ghost_idx",
            vec!["nowhere"],
            "InvalidInputException",
            |_table| index_messages::index_keys_not_partition_columns(&["nowhere"]),
        ),
        (
            "an index on a table that partitions by nothing",
            json!([]),
            "dt_idx",
            vec!["dt"],
            "InvalidInputException",
            |_table| index_messages::index_keys_not_partition_columns(&["dt"]),
        ),
        (
            "one key of two that is not a partition key",
            json!([ { "Name": "year", "Type": "int" } ]),
            "ym_idx",
            vec!["year", "month"],
            "InvalidInputException",
            |_table| index_messages::index_keys_not_partition_columns(&["month"]),
        ),
        (
            "two keys of three that are not partition keys, named in request order",
            json!([ { "Name": "year", "Type": "int" } ]),
            "ymd_idx",
            vec!["year", "month", "day"],
            "InvalidInputException",
            // Spelled out, as on the first row: this pins how several keys at fault are
            // listed.
            |_table| {
                "IndexKeys not a part of PartitionColumns. Verify the indexKeys : [month, day]"
                    .to_string()
            },
        ),
        (
            "one key named twice inside the index",
            json!([ { "Name": "year", "Type": "int" } ]),
            "yy_idx",
            vec!["year", "year"],
            "InvalidInputException",
            |table| index_messages::key_named_twice(table, "yy_idx", "year"),
        ),
    ];

    for (index, (case, partition_keys, index_name, keys, expected, expected_message)) in
        cases.iter().enumerate()
    {
        let router = emulator();
        let database = format!("refused_registration_db_{index}");
        let table = "events";
        create_indexed_table(&router, &database, table, partition_keys, None).await;

        let (status, body) = register(&router, &database, table, index_name, keys).await;
        assert_eq!(
            status, 400,
            "case {case}: the registration must be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with(expected),
            "case {case}: expected {expected}, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            expected_message(table),
            "case {case}: expected {:?}, got {:?}",
            expected_message(table),
            error_message(&body)
        );

        // Nothing was registered: the table is there, and it holds no index.
        let (status, read) = get_indexes(&router, &database, table, None).await;
        assert_eq!(
            status, 200,
            "case {case}: the table itself must be untouched, got {status} with {read}"
        );
        assert_eq!(
            read["PartitionIndexDescriptorList"],
            json!([]),
            "case {case}: the refused registration must register nothing, got {read}"
        );
    }

    // The control: a table whose registration was refused still takes an index over a key it
    // does partition by.
    let router = emulator();
    create_indexed_table(
        &router,
        "control_db",
        "events",
        &json!([ { "Name": "dt", "Type": "string" } ]),
        None,
    )
    .await;
    let (status, body) = register(&router, "control_db", "events", "payload_idx", &["payload"]).await;
    assert_eq!(
        status, 400,
        "case the control: the data column must be refused, got {status} with {body}"
    );
    let (status, body) = register(&router, "control_db", "events", "dt_idx", &["dt"]).await;
    assert_eq!(
        status, 200,
        "case the control: the partition key must be accepted, got {status} with {body}"
    );
    let (_, read) = get_indexes(&router, "control_db", "events", None).await;
    assert_eq!(
        reported(&read),
        vec![(
            "dt_idx".to_string(),
            vec![("dt".to_string(), "string".to_string())],
            INDEX_STATUS_CREATING.to_string()
        )],
        "case the control: only the accepted index may be registered, got {:?}",
        reported(&read)
    );
}

// ---------------------------------------------------------------------------
// The registration is addressed through a stored table
// ---------------------------------------------------------------------------

/// A registration whose address does not resolve to a stored table is
/// `EntityNotFoundException`, naming the entity that is missing — and nothing is registered
/// anywhere.
///
/// The rows differ along which part of the address is missing: the database, the table, and a
/// table whose name belongs to another database.
#[tokio::test]
async fn a_registration_through_a_table_the_catalog_does_not_hold_is_not_found() {
    let router = emulator();
    create_indexed_table(
        &router,
        "held_db",
        "events",
        &json!([ { "Name": "dt", "Type": "string" } ]),
        None,
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
        let (status, body) = register(&router, database, table, "dt_idx", &["dt"]).await;
        assert_eq!(
            status, 400,
            "case {case}: a registration needs a stored table, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            *message,
            "case {case}: expected {message:?}, got {:?}",
            error_message(&body)
        );
    }

    // The table that does exist is untouched by any of those misses.
    let (_, read) = get_indexes(&router, "held_db", "events", None).await;
    assert_eq!(
        read["PartitionIndexDescriptorList"],
        json!([]),
        "a refused registration must register nothing on the table that does exist, got {read}"
    );
}

// ---------------------------------------------------------------------------
// The backfill over the partitions the table already holds
// ---------------------------------------------------------------------------

/// The BUILD of an index registered on an existing table backfills over the partitions that
/// table already holds: a stored partition whose value for an indexed key is not a value of
/// the type that key declares, or which states no value for it at all, is reported under the
/// `BackfillErrorCode` that says which of the two it is, and the index's build ends FAILED.
/// A table with no such partition has nothing at fault, and its build ends ACTIVE.
///
/// The rows differ along the declared TYPE of the indexed key — text, each integer width,
/// floating point, fixed point, boolean, date, timestamp, and a key that declares no type —
/// along the values the stored partitions carry for it, along how many keys the index names
/// and whether the offending key is one of them, and along which fault the partition is at.
///
/// The type names are Hive type names, which is the syntax `Column$Type` ("the data type of
/// the column") is written in, parameterised forms included.
#[tokio::test]
async fn the_backfill_reports_the_stored_partitions_the_index_cannot_cover() {
    let cases: Vec<BackfillCase> = vec![
        BackfillCase {
            repartitioned: None,
            case: "a table that holds no partition at all",
            partition_keys: json!([ { "Name": "dt", "Type": "string" } ]),
            partitions: vec![],
            index_keys: vec!["dt"],
            status: INDEX_STATUS_ACTIVE,
            errors: vec![],
        },
        BackfillCase {
            repartitioned: None,
            case: "text values of a text key",
            partition_keys: json!([ { "Name": "dt", "Type": "string" } ]),
            partitions: vec![vec!["anything at all"], vec!["2024-01-01"]],
            index_keys: vec!["dt"],
            status: INDEX_STATUS_ACTIVE,
            errors: vec![],
        },
        BackfillCase {
            repartitioned: None,
            case: "integers of an int key",
            partition_keys: json!([ { "Name": "hour", "Type": "int" } ]),
            partitions: vec![vec!["0"], vec!["23"], vec!["-1"]],
            index_keys: vec!["hour"],
            status: INDEX_STATUS_ACTIVE,
            errors: vec![],
        },
        BackfillCase {
            repartitioned: None,
            case: "a value that is no integer at all of an int key",
            partition_keys: json!([ { "Name": "hour", "Type": "int" } ]),
            partitions: vec![vec!["23"], vec!["not-a-number"]],
            index_keys: vec!["hour"],
            status: INDEX_STATUS_FAILED,
            errors: vec![(
                BACKFILL_INVALID_PARTITION_TYPE_DATA,
                vec![vec!["not-a-number"]],
            )],
        },
        BackfillCase {
            repartitioned: None,
            case: "a value one past the width of a tinyint key",
            partition_keys: json!([ { "Name": "bucket", "Type": "tinyint" } ]),
            partitions: vec![vec!["127"], vec!["128"]],
            index_keys: vec!["bucket"],
            status: INDEX_STATUS_FAILED,
            errors: vec![(BACKFILL_INVALID_PARTITION_TYPE_DATA, vec![vec!["128"]])],
        },
        BackfillCase {
            repartitioned: None,
            case: "a value one past the width of a bigint key",
            partition_keys: json!([ { "Name": "id", "Type": "bigint" } ]),
            partitions: vec![
                vec!["9223372036854775807"],
                vec!["9223372036854775808"],
            ],
            index_keys: vec!["id"],
            status: INDEX_STATUS_FAILED,
            errors: vec![(
                BACKFILL_INVALID_PARTITION_TYPE_DATA,
                vec![vec!["9223372036854775808"]],
            )],
        },
        BackfillCase {
            repartitioned: None,
            case: "a day the calendar does not hold of a date key",
            partition_keys: json!([ { "Name": "dt", "Type": "date" } ]),
            partitions: vec![vec!["2024-02-29"], vec!["2024-02-30"]],
            index_keys: vec!["dt"],
            status: INDEX_STATUS_FAILED,
            errors: vec![(
                BACKFILL_INVALID_PARTITION_TYPE_DATA,
                vec![vec!["2024-02-30"]],
            )],
        },
        BackfillCase {
            repartitioned: None,
            case: "a date and a time of day of a timestamp key",
            partition_keys: json!([ { "Name": "at", "Type": "timestamp" } ]),
            partitions: vec![vec!["2024-01-15 10:30:00"], vec!["2024-01-15"]],
            index_keys: vec!["at"],
            status: INDEX_STATUS_ACTIVE,
            errors: vec![],
        },
        BackfillCase {
            repartitioned: None,
            case: "a value that is no fixed-point number of a parameterised decimal key",
            partition_keys: json!([ { "Name": "amount", "Type": "decimal(10,2)" } ]),
            partitions: vec![vec!["1.25"], vec!["1.2.3"]],
            index_keys: vec!["amount"],
            status: INDEX_STATUS_FAILED,
            errors: vec![(BACKFILL_INVALID_PARTITION_TYPE_DATA, vec![vec!["1.2.3"]])],
        },
        BackfillCase {
            repartitioned: None,
            case: "a value that is no floating-point number of a float key",
            partition_keys: json!([ { "Name": "ratio", "Type": "float" } ]),
            partitions: vec![vec!["1.5"], vec!["abc"]],
            index_keys: vec!["ratio"],
            status: INDEX_STATUS_FAILED,
            errors: vec![(BACKFILL_INVALID_PARTITION_TYPE_DATA, vec![vec!["abc"]])],
        },
        BackfillCase {
            repartitioned: None,
            case: "a value that is neither truth literal of a boolean key",
            partition_keys: json!([ { "Name": "flag", "Type": "boolean" } ]),
            partitions: vec![vec!["TRUE"], vec!["yes"]],
            index_keys: vec!["flag"],
            status: INDEX_STATUS_FAILED,
            errors: vec![(BACKFILL_INVALID_PARTITION_TYPE_DATA, vec![vec!["yes"]])],
        },
        BackfillCase {
            repartitioned: None,
            case: "two partitions at fault, in the order the table holds them",
            partition_keys: json!([
                { "Name": "dt", "Type": "date" },
                { "Name": "hour", "Type": "int" }
            ]),
            partitions: vec![
                vec!["2024-01-01", "5"],
                vec!["2024-02-30", "6"],
                vec!["2024-01-01", "noon"],
            ],
            index_keys: vec!["dt", "hour"],
            status: INDEX_STATUS_FAILED,
            errors: vec![(
                BACKFILL_INVALID_PARTITION_TYPE_DATA,
                vec![vec!["2024-01-01", "noon"], vec!["2024-02-30", "6"]],
            )],
        },
        BackfillCase {
            repartitioned: None,
            case: "a value at fault under a key the index does not name",
            partition_keys: json!([
                { "Name": "dt", "Type": "string" },
                { "Name": "hour", "Type": "int" }
            ]),
            partitions: vec![vec!["2024-01-01", "noon"]],
            index_keys: vec!["dt"],
            status: INDEX_STATUS_ACTIVE,
            errors: vec![],
        },
        BackfillCase {
            repartitioned: None,
            case: "any value of a partition key that declares no type",
            partition_keys: json!([ { "Name": "untyped" } ]),
            partitions: vec![vec!["anything at all"]],
            index_keys: vec!["untyped"],
            status: INDEX_STATUS_ACTIVE,
            errors: vec![],
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        assert_backfill_case(index, row).await;
    }
}

/// Drive ONE row of a backfill table: create the table, store the partitions it already
/// holds, repartition it when the row says so, register the index, and assert both what a
/// read observes while the build is still ahead of the clock and what the build reports at an
/// instant it is due.
async fn assert_backfill_case(index: usize, row: &BackfillCase) {
    let case = row.case;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let database = format!("backfill_db_{index}_{}", row.index_keys.len());
    let table = "events";
    create_indexed_table(&router, &database, table, &row.partition_keys, None).await;
    create_partitions(&router, &database, table, &row.partitions).await;
    if let Some(partition_keys) = &row.repartitioned {
        repartition(&router, &database, table, partition_keys).await;
    }

    let (status, body) = register(&router, &database, table, "added_idx", &row.index_keys).await;
    assert_eq!(
        status, 200,
        "case {case}: the registration must be served, got {status} with {body}"
    );

    // While the build is still ahead of the clock the index is being built, and there is
    // no result to report yet.
    let (_, read) = get_indexes(&router, &database, table, None).await;
    let building = reported(&read);
    assert_eq!(
        building
            .iter()
            .map(|(_, _, status)| status.as_str())
            .collect::<Vec<&str>>(),
        vec![INDEX_STATUS_CREATING],
        "case {case}: the index must be reported as being built, got {building:?}"
    );
    for descriptor in read["PartitionIndexDescriptorList"]
        .as_array()
        .unwrap_or(&Vec::new())
    {
        assert!(
            descriptor["BackfillErrors"].is_null(),
            "case {case}: no backfill result may be reported before the build is due, got \
             {descriptor}"
        );
    }

    // And at an instant the build is due, the index reports what the backfill found.
    let descriptor = recorded_descriptor(&engine, case, &database, table, far_future());
    assert_eq!(
        descriptor["IndexStatus"].as_str(),
        Some(row.status),
        "case {case}: expected the build to end {}, got {descriptor}",
        row.status
    );
    let expected = expected_backfill(&row.errors);
    assert_eq!(
        reported_backfill(&descriptor),
        expected,
        "case {case}: expected {expected:?}, got {:?} in {descriptor}",
        reported_backfill(&descriptor)
    );
}

// ---------------------------------------------------------------------------
// A stored partition that states no value for an indexed key
// ---------------------------------------------------------------------------

/// A stored partition states one value per partition key of its table, so a table that gains
/// a partition key after its partitions were stored holds partitions that state NO value for
/// that key — and an index over it reports them under the code for a missing partition value,
/// beside the type faults of the keys they do state a value for.
///
/// The rows differ along which fault the stored partitions are at: only a missing value, a
/// missing value beside a type fault, and a gained key the index does not name at all.
#[tokio::test]
async fn a_partition_stating_no_value_for_an_indexed_key_is_reported_missing() {
    let cases: Vec<BackfillCase> = vec![
        BackfillCase {
            case: "every stored partition states no value for the key the table gained",
            partition_keys: json!([ { "Name": "dt", "Type": "string" } ]),
            partitions: vec![vec!["2024-01-01"], vec!["2024-01-02"]],
            repartitioned: Some(json!([
                { "Name": "dt", "Type": "string" },
                { "Name": "hour", "Type": "int" }
            ])),
            index_keys: vec!["dt", "hour"],
            status: INDEX_STATUS_FAILED,
            errors: vec![(
                BACKFILL_MISSING_PARTITION_VALUE,
                vec![vec!["2024-01-01"], vec!["2024-01-02"]],
            )],
        },
        BackfillCase {
            case: "a missing value beside a value of the wrong type",
            partition_keys: json!([
                { "Name": "dt", "Type": "date" },
                { "Name": "hour", "Type": "int" }
            ]),
            partitions: vec![vec!["2024-01-01", "5"], vec!["2024-01-01", "noon"]],
            repartitioned: Some(json!([
                { "Name": "dt", "Type": "date" },
                { "Name": "hour", "Type": "int" },
                { "Name": "region", "Type": "string" }
            ])),
            index_keys: vec!["dt", "hour", "region"],
            status: INDEX_STATUS_FAILED,
            errors: vec![
                (
                    BACKFILL_INVALID_PARTITION_TYPE_DATA,
                    vec![vec!["2024-01-01", "noon"]],
                ),
                (
                    BACKFILL_MISSING_PARTITION_VALUE,
                    vec![vec!["2024-01-01", "5"]],
                ),
            ],
        },
        BackfillCase {
            case: "a gained key the index does not name",
            partition_keys: json!([ { "Name": "dt", "Type": "string" } ]),
            partitions: vec![vec!["2024-01-01"]],
            repartitioned: Some(json!([
                { "Name": "dt", "Type": "string" },
                { "Name": "hour", "Type": "int" }
            ])),
            index_keys: vec!["dt"],
            status: INDEX_STATUS_ACTIVE,
            errors: vec![],
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        assert_backfill_case(index + 100, row).await;
    }
}

// ---------------------------------------------------------------------------
// How many partitions one backfill error names
// ---------------------------------------------------------------------------

/// A backfill error names a LIMITED number of the partitions at fault: the code says why the
/// build failed however many partitions are at fault, and the list it carries stops at the
/// number this emulator reports.
#[tokio::test]
async fn a_backfill_error_names_a_limited_number_of_the_partitions_at_fault() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let case = "more partitions at fault than one error names";
    create_indexed_table(
        &router,
        "capped_db",
        "events",
        &json!([ { "Name": "hour", "Type": "int" } ]),
        None,
    )
    .await;
    // Every partition is at fault, and there are more of them than one error names.
    let at_fault: Vec<Vec<String>> = (0..MAX_BACKFILL_ERROR_PARTITIONS + 2)
        .map(|value| vec![format!("hour-{value}")])
        .collect();
    for values in &at_fault {
        ok(
            &router,
            "CreatePartition",
            &json!({
                "DatabaseName": "capped_db",
                "TableName": "events",
                "PartitionInput": { "Values": values }
            }),
        )
        .await;
    }

    let (status, body) = register(&router, "capped_db", "events", "hour_idx", &["hour"]).await;
    assert_eq!(
        status, 200,
        "case {case}: the registration must be served, got {status} with {body}"
    );

    let descriptor = recorded_descriptor(&engine, case, "capped_db", "events", far_future());
    assert_eq!(
        descriptor["IndexStatus"].as_str(),
        Some(INDEX_STATUS_FAILED),
        "case {case}: a build with partitions at fault must end {INDEX_STATUS_FAILED}, got \
         {descriptor}"
    );
    let reported = reported_backfill(&descriptor);
    assert_eq!(
        reported.len(),
        1,
        "case {case}: every partition is at fault for the same reason, so one error says so, \
         got {reported:?}"
    );
    let (code, partitions) = &reported[0];
    assert_eq!(
        code, BACKFILL_INVALID_PARTITION_TYPE_DATA,
        "case {case}: expected {BACKFILL_INVALID_PARTITION_TYPE_DATA}, got {code}"
    );
    assert_eq!(
        partitions.len(),
        MAX_BACKFILL_ERROR_PARTITIONS,
        "case {case}: expected {MAX_BACKFILL_ERROR_PARTITIONS} partition(s) named, got {:?}",
        partitions
    );
    for named in partitions {
        assert!(
            at_fault.contains(named),
            "case {case}: every partition named must be one the table holds, got {named:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The lifecycle, on the wall clock alone
// ---------------------------------------------------------------------------

/// The status of a registered index is owed by the WALL CLOCK: without anything writing a due
/// instant, an index registered on a table reaches the status its own build earned — ACTIVE
/// when the table's partitions are all covered, FAILED with the backfill result when one of
/// them is not — and a caller reading straight after the registration still observes it being
/// built.
///
/// The two tables differ along the one thing that decides the outcome: whether a stored
/// partition carries a value of the indexed key's declared type.
#[tokio::test]
async fn a_registered_index_reaches_the_status_its_build_earned() {
    let router = emulator();
    // (the table, the partition it holds, the status its build must reach)
    let tables: [(&str, &str, &str); 2] = [
        ("covered", "7", INDEX_STATUS_ACTIVE),
        ("uncovered", "noon", INDEX_STATUS_FAILED),
    ];

    for (table, value, _) in &tables {
        // One database per table: `create_indexed_table` creates the database it puts the
        // table in, and a database is created once.
        let database = format!("lifecycle_db_{table}");
        create_indexed_table(
            &router,
            &database,
            table,
            &json!([ { "Name": "hour", "Type": "int" } ]),
            None,
        )
        .await;
        create_partitions(&router, &database, table, &[vec![*value]]).await;
        let (status, body) = register(&router, &database, table, "hour_idx", &["hour"]).await;
        assert_eq!(
            status, 200,
            "case {table}: the registration must be served, got {status} with {body}"
        );
        let (_, read) = get_indexes(&router, &database, table, None).await;
        assert_eq!(
            reported(&read)
                .iter()
                .map(|(_, _, status)| status.clone())
                .collect::<Vec<String>>(),
            vec![INDEX_STATUS_CREATING.to_string()],
            "case {table}: a read straight after the registration must observe the index being \
             built, got {read}"
        );
    }

    // The build is owed one span after the registration, so it is due well inside this
    // window; a window that closes before it is fails the test rather than hanging, and
    // nothing here waits a fixed time.
    let deadline = SystemTime::now() + 20 * PARTITION_INDEX_SETTLES_AFTER;
    for (table, _, expected) in &tables {
        let database = format!("lifecycle_db_{table}");
        let mut descriptor = Value::Null;
        while SystemTime::now() < deadline {
            let (_, read) = get_indexes(&router, &database, table, None).await;
            let found = read["PartitionIndexDescriptorList"][0].clone();
            let status = found["IndexStatus"].as_str().unwrap_or_default().to_string();
            descriptor = found;
            if status != INDEX_STATUS_CREATING {
                break;
            }
            tokio::time::sleep(PARTITION_INDEX_SETTLES_AFTER / 10).await;
        }
        assert_eq!(
            descriptor["IndexStatus"].as_str(),
            Some(*expected),
            "case {table}: expected the build to reach {expected}, got {descriptor}"
        );
        let expected_errors = if *expected == INDEX_STATUS_FAILED {
            vec![(
                BACKFILL_INVALID_PARTITION_TYPE_DATA.to_string(),
                vec![vec!["noon".to_string()]],
            )]
        } else {
            vec![]
        };
        assert_eq!(
            reported_backfill(&descriptor),
            expected_errors,
            "case {table}: expected {expected_errors:?}, got {:?} in {descriptor}",
            reported_backfill(&descriptor)
        );
    }
}
