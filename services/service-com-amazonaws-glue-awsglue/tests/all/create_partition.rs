// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the single-partition create, `CreatePartition`, and the
//! `GetPartition` read that serves what it stored.
//!
//! Every call goes over the wire the way a client makes it, through the shared
//! [`crate::wire`] harness: `POST /` with `X-Amz-Target: AWSGlue.<Operation>` and an
//! `awsJson1_1` body, dispatched by the same `MultiServiceRouter` the production front
//! door uses — so what is asserted here is HTTP behaviour and not a direct call into a
//! handler.
//!
//! The rule under test: a create writes the submitted `PartitionInput` onto the table the
//! request names, ADDRESSED by its `Values` — one value per partition key of that table,
//! matched exactly as sent — so a later read of those values serves what the client sent
//! plus the one member only the server sets, `CreationTime`; a create the catalog cannot
//! serve stores nothing and raises the modelled exception for its reason: a bad value
//! list is `InvalidInputException`, a value list the table already holds is
//! `AlreadyExistsException`, a create past the partition ceiling is
//! `ResourceNumberLimitExceededException`, and a database or table the catalog does not
//! hold is `EntityNotFoundException`.
//!
//! The suites are tables whose rows differ along what the rule generalises over: how many
//! partition keys the table defines, which members a `PartitionInput` carries, the form
//! and casing of a partition value, and the way one create can fail.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::MAX_PARTITIONS_PER_TABLE;
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::assert_refusal;
use crate::wire::{assert_echoed, call, emulator, error_code, ok, ACCOUNT_ID};

/// The largest `PartitionInputList` one `BatchCreatePartition` may carry:
/// `PartitionInputList`, length 0..100, from `smithy_query` on `BatchCreatePartition`.
/// Used only to seed a table up to its ceiling without making one request per partition.
const MAX_PARTITIONS_PER_BATCH: usize = 100;

/// Create `database` holding `table`, partitioned by `keys` (one `Column` per key).
async fn seed_table(router: &MultiServiceRouter, database: &str, table: &str, keys: &[&str]) {
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database } }),
    )
    .await;
    let partition_keys: Vec<Value> = keys
        .iter()
        .map(|key| json!({ "Name": key, "Type": "string" }))
        .collect();
    ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": database,
            "TableInput": { "Name": table, "PartitionKeys": partition_keys }
        }),
    )
    .await;
}

/// The `CreatePartition` request body for one `PartitionInput`.
fn create_body(database: &str, table: &str, input: &Value) -> Value {
    json!({
        "DatabaseName": database,
        "TableName": table,
        "PartitionInput": input
    })
}

/// Submit one `CreatePartition`, returning its HTTP status and body.
async fn create_partition(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    input: &Value,
) -> (u16, Value) {
    call(
        router,
        "CreatePartition",
        &create_body(database, table, input),
    )
    .await
}

/// Read one partition back, returning the HTTP status and body.
async fn get_partition(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    values: &[&str],
) -> (u16, Value) {
    call(
        router,
        "GetPartition",
        &json!({
            "DatabaseName": database,
            "TableName": table,
            "PartitionValues": values
        }),
    )
    .await
}

/// The `Values` a `PartitionInput` carries, as the list a read addresses it by.
fn values_of(input: &Value) -> Vec<&str> {
    input["Values"]
        .as_array()
        .expect("the input carries Values")
        .iter()
        .map(|value| value.as_str().expect("a partition value is a string"))
        .collect()
}

/// The member set a client sent, minus the two instants it may carry.
///
/// An instant is asserted on its numeric value instead ([`instant`]): awsJson renders a
/// timestamp as a JSON number, and whether that number is spelled with a fractional part
/// is the serializer's choice, not part of the value.
fn without_instants(input: &Value) -> Value {
    let mut copy = input.clone();
    if let Some(members) = copy.as_object_mut() {
        members.remove("LastAccessTime");
        members.remove("LastAnalyzedTime");
    }
    copy
}

/// One member's instant as epoch seconds, or `None` when it carries none.
fn instant(value: &Value, member: &str) -> Option<f64> {
    value.get(member).and_then(Value::as_f64)
}

// ---------------------------------------------------------------------------
// Create -> read round trip
// ---------------------------------------------------------------------------

/// Whatever a client submits to `CreatePartition` is what `GetPartition` reads back,
/// beside the server-set `CreationTime` and the catalog's own attribution of the
/// partition to its catalog, database and table.
///
/// The rows differ along the number of partition keys (1, 2, 3), the member set each
/// `PartitionInput` carries (bare values; a fully described storage descriptor and
/// parameters; access instants), and the form of a partition value (a date, a Region,
/// digits, a value with spaces, a non-ASCII value, and two values differing only in
/// case).
#[tokio::test]
async fn created_partitions_are_read_back_from_the_store() {
    let router = emulator();

    let cases: Vec<(&str, Vec<&str>, Vec<Value>)> = vec![
        (
            "one key, required members only",
            vec!["dt"],
            vec![json!({ "Values": ["2026-01-01"] })],
        ),
        (
            "two keys, fully described storage, created one call at a time",
            vec!["dt", "region"],
            vec![
                json!({
                    "Values": ["2026-01-01", "us-east-1"],
                    "StorageDescriptor": {
                        "Columns": [
                            { "Name": "id", "Type": "string" },
                            { "Name": "amount", "Type": "double", "Comment": "cents" }
                        ],
                        "Location": "s3://bucket/events/dt=2026-01-01/region=us-east-1/",
                        "InputFormat": "org.apache.hadoop.mapred.TextInputFormat",
                        "OutputFormat":
                            "org.apache.hadoop.hive.ql.io.HiveIgnoreKeyTextOutputFormat",
                        "Compressed": true,
                        "NumberOfBuckets": 2,
                        "BucketColumns": ["id"],
                        "SortColumns": [ { "Column": "id", "SortOrder": 1 } ],
                        "SerdeInfo": {
                            "Name": "csv",
                            "SerializationLibrary":
                                "org.apache.hadoop.hive.serde2.OpenCSVSerde",
                            "Parameters": { "separatorChar": "," }
                        },
                        "Parameters": { "skip.header.line.count": "1" }
                    },
                    "Parameters": { "classification": "csv" }
                }),
                json!({ "Values": ["2026-01-02", "eu-west-1"], "Parameters": { "rows": "17" } }),
            ],
        ),
        (
            "three keys, digit values, both access instants",
            vec!["year", "month", "day"],
            vec![json!({
                "Values": ["2026", "01", "02"],
                "LastAccessTime": 1_767_225_600,
                "LastAnalyzedTime": 1_767_312_000
            })],
        ),
        (
            "one key, five value forms",
            vec!["k"],
            vec![
                json!({ "Values": ["a"] }),
                // A partition value is table DATA: the catalog's Hive fold covers
                // database and table names, so these two are different partitions.
                json!({ "Values": ["A"] }),
                json!({ "Values": ["a value with spaces"] }),
                json!({ "Values": ["100"] }),
                json!({ "Values": ["ünïcode=yes"] }),
            ],
        ),
    ];

    for (index, (case, keys, inputs)) in cases.iter().enumerate() {
        let database = format!("single_create_db_{index}");
        // Mixed case, so the attribution a partition reports is the FOLDED table name
        // the catalog stores.
        let table = format!("Events_{index}");
        let folded_table = table.to_lowercase();
        seed_table(&router, &database, &table, keys).await;

        for input in inputs {
            let values = values_of(input);
            let (status, body) = create_partition(&router, &database, &table, input).await;
            assert_eq!(
                status, 200,
                "case {case}: CreatePartition {values:?} must answer 200, got {status} with \
                 {body}"
            );

            let (status, read) = get_partition(&router, &database, &table, &values).await;
            assert_eq!(
                status, 200,
                "case {case}: GetPartition {values:?} must serve what the create stored, got \
                 {status} with {read}"
            );
            let partition = &read["Partition"];

            assert_echoed(case, &without_instants(input), partition);
            for member in ["LastAccessTime", "LastAnalyzedTime"] {
                assert_eq!(
                    instant(partition, member),
                    instant(input, member),
                    "case {case}: partition {values:?} must report the {member} it was created \
                     with, got {:?}",
                    partition.get(member)
                );
            }
            assert_eq!(
                partition["DatabaseName"].as_str(),
                Some(database.as_str()),
                "case {case}: partition {values:?} must be attributed to its database, got {:?}",
                partition["DatabaseName"]
            );
            assert_eq!(
                partition["TableName"].as_str(),
                Some(folded_table.as_str()),
                "case {case}: partition {values:?} must be attributed to its table by the \
                 folded name, got {:?}",
                partition["TableName"]
            );
            assert_eq!(
                partition["CatalogId"].as_str(),
                Some(ACCOUNT_ID),
                "case {case}: partition {values:?} must be attributed to the default catalog, \
                 got {:?}",
                partition["CatalogId"]
            );
            assert!(
                partition["CreationTime"].as_f64().is_some_and(|at| at > 0.0),
                "case {case}: partition {values:?} must carry a populated CreationTime, got {:?}",
                partition["CreationTime"]
            );
        }

        // Every partition of the case is still readable after the later creates of the
        // same table: one create writes its own slice and disturbs no other.
        for input in inputs {
            let values = values_of(input);
            let (status, read) = get_partition(&router, &database, &table, &values).await;
            assert_eq!(
                status, 200,
                "case {case}: partition {values:?} must survive the later creates of its \
                 table, got {status} with {read}"
            );
        }
    }
}

/// A create under an explicit `CatalogId` — the account id, which is also the default —
/// addresses the same catalog a create that names none does, so the partition is served
/// by a read either way.
#[tokio::test]
async fn an_explicit_catalog_id_addresses_the_default_catalog() {
    let router = emulator();
    seed_table(&router, "catalog_db", "catalog_table", &["dt"]).await;

    let body = ok(
        &router,
        "CreatePartition",
        &json!({
            "CatalogId": ACCOUNT_ID,
            "DatabaseName": "catalog_db",
            "TableName": "catalog_table",
            "PartitionInput": { "Values": ["2026-03-04"] }
        }),
    )
    .await;
    assert!(
        body.is_null() || body.is_object(),
        "CreatePartition answers an empty response body, got {body}"
    );

    let (status, read) = get_partition(&router, "catalog_db", "catalog_table", &["2026-03-04"]).await;
    assert_eq!(
        status, 200,
        "a partition created under the explicit catalog id must be readable, got {status} with \
         {read}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// One row of the refusal table.
struct RefusalCase {
    case: &'static str,
    /// The partition keys the addressed table defines.
    keys: Vec<&'static str>,
    /// A partition created before the one under test, if the row needs one.
    seeded: Option<Value>,
    /// The `PartitionInput` whose create must be refused.
    input: Value,
    expected_code: &'static str,
    expected_message: String,
    /// Value lists a read must NOT serve after the refusal.
    missing: Vec<Vec<&'static str>>,
}

/// A create the catalog cannot serve is refused with the modelled exception its reason
/// names, stores nothing, and leaves what is already stored exactly as it was.
///
/// The rows differ along the way the create fails — a value list the table already holds,
/// fewer values than the table has partition keys, more values than it has, no values at
/// all, and a table that defines no partition keys — and along how many partition keys
/// the addressed table defines.
#[tokio::test]
async fn a_create_the_catalog_cannot_serve_is_refused_and_stores_nothing() {
    let router = emulator();

    let cases = [
        RefusalCase {
            case: "the values of a partition the table already holds",
            keys: vec!["dt"],
            seeded: Some(json!({ "Values": ["2026-01-01"] })),
            input: json!({ "Values": ["2026-01-01"], "Parameters": { "generation": "second" } }),
            expected_code: "AlreadyExistsException",
            expected_message: "Partition already exists.".to_string(),
            missing: vec![],
        },
        RefusalCase {
            case: "fewer values than the table has partition keys",
            keys: vec!["dt", "region"],
            seeded: None,
            input: json!({ "Values": ["2026-01-01"] }),
            expected_code: "InvalidInputException",
            expected_message: "Partition values must name one value per partition key of the \
                               table; the table has 2 partition keys and the input carried 1 \
                               values"
                .to_string(),
            missing: vec![vec!["2026-01-01"]],
        },
        RefusalCase {
            case: "more values than the table has partition keys",
            keys: vec!["dt"],
            seeded: None,
            input: json!({ "Values": ["2026-01-01", "us-east-1"] }),
            expected_code: "InvalidInputException",
            expected_message: "Partition values must name one value per partition key of the \
                               table; the table has 1 partition keys and the input carried 2 \
                               values"
                .to_string(),
            missing: vec![vec!["2026-01-01", "us-east-1"], vec!["2026-01-01"]],
        },
        RefusalCase {
            case: "an input that names no values at all",
            keys: vec!["dt"],
            seeded: None,
            input: json!({ "Parameters": { "classification": "csv" } }),
            expected_code: "InvalidInputException",
            expected_message: "PartitionInput must specify Values.".to_string(),
            missing: vec![],
        },
        RefusalCase {
            case: "a table that defines no partition keys",
            keys: vec![],
            seeded: None,
            input: json!({ "Values": ["2026-01-01"] }),
            expected_code: "InvalidInputException",
            expected_message: "Partition values must name one value per partition key of the \
                               table; the table has 0 partition keys and the input carried 1 \
                               values"
                .to_string(),
            missing: vec![vec!["2026-01-01"]],
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let case = row.case;
        let database = format!("refused_db_{index}");
        let table = format!("refused_table_{index}");
        seed_table(&router, &database, &table, &row.keys).await;

        let before = match &row.seeded {
            Some(input) => {
                let (status, body) = create_partition(&router, &database, &table, input).await;
                assert_eq!(
                    status, 200,
                    "case {case}: the seeded create must answer 200, got {status} with {body}"
                );
                let values = values_of(input);
                let (_, read) = get_partition(&router, &database, &table, &values).await;
                Some((values, read))
            }
            None => None,
        };

        assert_refusal(
            &router,
            case,
            "CreatePartition",
            &create_body(&database, &table, &row.input),
            row.expected_code,
            &row.expected_message,
        )
        .await;

        for values in &row.missing {
            let (status, read) = get_partition(&router, &database, &table, values).await;
            assert_eq!(
                status, 400,
                "case {case}: a refused create must store nothing, so a read of {values:?} must \
                 still miss; got {status} with {read}"
            );
        }

        if let Some((values, stored)) = before {
            let (status, after) = get_partition(&router, &database, &table, &values).await;
            assert_eq!(
                status, 200,
                "case {case}: the partition stored before the refusal must still be readable, \
                 got {status} with {after}"
            );
            assert_eq!(
                after["Partition"], stored["Partition"],
                "case {case}: a refused create must not overwrite the stored partition; \
                 expected {}, got {}",
                stored["Partition"], after["Partition"]
            );
        }
    }
}

/// A database or table the catalog does not hold is `EntityNotFoundException` with the
/// wording this service answers for that resource — there is no table to write onto.
///
/// The rows differ along which resource is missing and the casing of the name, which the
/// catalog folds for lookup but reports back as sent.
#[tokio::test]
async fn a_create_naming_no_stored_table_is_refused() {
    let router = emulator();
    seed_table(&router, "present_db", "present_table", &["dt"]).await;

    let cases = vec![
        (
            "no such database",
            "ghost_db",
            "present_table",
            "Database ghost_db not found.",
        ),
        (
            "no such database, mixed case",
            "Ghost_DB",
            "present_table",
            "Database ghost_db not found.",
        ),
        (
            "no such table",
            "present_db",
            "ghost_table",
            "Table ghost_table not found.",
        ),
        (
            "no such table, mixed case",
            "present_db",
            "Ghost_Table",
            "Table ghost_table not found.",
        ),
    ];

    for (case, database, table, expected_message) in cases {
        assert_refusal(
            &router,
            case,
            "CreatePartition",
            &create_body(database, table, &json!({ "Values": ["2026-01-01"] })),
            "EntityNotFoundException",
            expected_message,
        )
        .await;
    }
}

/// One table holds at most [`MAX_PARTITIONS_PER_TABLE`] partitions: the single create
/// past the ceiling is refused as a resource limit rather than as a bad input, and the
/// partitions already stored stay readable.
#[tokio::test]
async fn a_create_past_the_partition_ceiling_is_refused_as_a_resource_limit() {
    let router = emulator();
    seed_table(&router, "ceiling_db", "ceiling_table", &["n"]).await;

    // Seeded in batches rather than one create per partition: the ceiling is the subject
    // here, not how many requests it takes to reach it.
    let mut created = 0usize;
    while created < MAX_PARTITIONS_PER_TABLE {
        let room = (MAX_PARTITIONS_PER_TABLE - created).min(MAX_PARTITIONS_PER_BATCH);
        let inputs: Vec<Value> = (created..created + room)
            .map(|n| json!({ "Values": [n.to_string()] }))
            .collect();
        let body = ok(
            &router,
            "BatchCreatePartition",
            &json!({
                "DatabaseName": "ceiling_db",
                "TableName": "ceiling_table",
                "PartitionInputList": inputs
            }),
        )
        .await;
        assert_eq!(
            body["Errors"].as_array().map(Vec::len),
            Some(0),
            "the first {MAX_PARTITIONS_PER_TABLE} partitions must be created, got {body}"
        );
        created += room;
    }

    let over = json!({ "Values": [MAX_PARTITIONS_PER_TABLE.to_string()] });
    let (status, body) = create_partition(&router, "ceiling_db", "ceiling_table", &over).await;
    assert_eq!(
        status, 400,
        "case ceiling: the create past the ceiling must be refused, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("ResourceNumberLimitExceededException"),
        "case ceiling: the create past the ceiling must be refused as a resource limit, got \
         {body}"
    );

    let (status, read) = get_partition(&router, "ceiling_db", "ceiling_table", &["0"]).await;
    assert_eq!(
        status, 200,
        "case ceiling: a refused create must not disturb the partitions already stored; got \
         {status} with {read}"
    );
}
