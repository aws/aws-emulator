// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue partition slice: `BatchCreatePartition` and the
//! `GetPartition` read that serves what the batch stored.
//!
//! Every call goes over the wire the way a client makes it, through the shared
//! [`crate::wire`] harness: `POST /` with `X-Amz-Target: AWSGlue.<Operation>` and an
//! `awsJson1_1` body, dispatched by the same `MultiServiceRouter` the production front
//! door uses — so what is asserted here is HTTP behaviour and not a direct call into a
//! handler.
//!
//! The rule under test: a batch create writes each submitted `PartitionInput` onto the
//! table the request names, ADDRESSED by its `Values` — one value per partition key of
//! that table, matched exactly as sent — so a later read of those values serves what the
//! client sent plus the one member only the server sets, `CreationTime`. An item that
//! cannot be created fails ONLY its own item, reported in `Errors`, while a database or
//! table the catalog does not hold fails the whole request.
//!
//! The suites are tables whose rows differ along what the rule generalises over: how
//! many partition keys the table defines, which members a `PartitionInput` carries, how
//! many items one batch submits, the form and casing of a partition value, and the way
//! one item can fail.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::messages::PARTITION_NOT_FOUND;
use service_com_amazonaws_glue_awsglue::state::MAX_PARTITIONS_PER_TABLE;
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::assert_refusal;
use crate::wire::{assert_echoed, call, emulator, error_code, ok, ACCOUNT_ID, ERROR_DETAIL_TAG};

/// The largest `PartitionInputList` one batch may carry: `PartitionInputList`, length
/// 0..100, from `smithy_query` on `BatchCreatePartition`.
const MAX_PARTITIONS_PER_BATCH: usize = 100;

/// Create `database` holding `table`, partitioned by `keys` (one `Column` per key).
pub(crate) async fn seed_table(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    keys: &[&str],
) {
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

/// Submit one `BatchCreatePartition` that must answer 200, and return its body.
pub(crate) async fn batch_create(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    inputs: &[Value],
) -> Value {
    ok(
        router,
        "BatchCreatePartition",
        &json!({
            "DatabaseName": database,
            "TableName": table,
            "PartitionInputList": inputs
        }),
    )
    .await
}

/// The `Errors` list of a batch create response, which the service reports whether or
/// not anything failed.
pub(crate) fn partition_errors(body: &Value) -> &[Value] {
    body["Errors"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_else(|| panic!("BatchCreatePartition must report an Errors list, got {body}"))
}

/// Read one partition back, returning the HTTP status and body.
pub(crate) async fn get_partition(
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
pub(crate) fn values_of(input: &Value) -> Vec<&str> {
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
pub(crate) fn without_instants(input: &Value) -> Value {
    let mut copy = input.clone();
    if let Some(members) = copy.as_object_mut() {
        members.remove("LastAccessTime");
        members.remove("LastAnalyzedTime");
    }
    copy
}

/// One member's instant as epoch seconds, or `None` when it carries none.
pub(crate) fn instant(value: &Value, member: &str) -> Option<f64> {
    value.get(member).and_then(Value::as_f64)
}

/// The members of a stored partition a `PartitionInput` can set, so a suite can assert
/// the ones a write OMITTED are gone rather than left over from an earlier write.
///
/// Source: the members of `com.amazonaws.glue#PartitionInput` other than `Values`, which
/// is the partition's address (`smithy_query shape=PartitionInput`).
pub(crate) const SETTABLE_MEMBERS: [&str; 4] = [
    "StorageDescriptor",
    "Parameters",
    "LastAccessTime",
    "LastAnalyzedTime",
];

/// Read one partition that must be there, and return it.
pub(crate) async fn stored(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    values: &[&str],
) -> Value {
    let (status, body) = get_partition(router, database, table, values).await;
    assert_eq!(
        status, 200,
        "the partition {values:?} must be readable, got {status} with {body}"
    );
    body["Partition"].clone()
}

/// `Parameters` naming which write stored a definition, so a read says which one it is
/// serving.
pub(crate) fn generation(which: &str) -> Value {
    json!({ "generation": which })
}

// ---------------------------------------------------------------------------
// Batch create -> read round trip
// ---------------------------------------------------------------------------

/// Whatever a client submits to `BatchCreatePartition` is what `GetPartition` reads
/// back, beside the server-set `CreationTime` and the catalog's own attribution of the
/// partition to its catalog, database and table.
///
/// The rows differ along the number of partition keys (1, 2, 3), the member set each
/// `PartitionInput` carries (bare values; a fully described storage descriptor and
/// parameters; access instants), the number of items in one batch (1, 3, 5), and the
/// form of a partition value (a date, a Region, digits, a value with a space, a
/// non-ASCII value, and two values that differ only in case).
#[tokio::test]
async fn batch_created_partitions_are_read_back_from_the_store() {
    let router = emulator();

    let cases: Vec<(&str, Vec<&str>, Vec<Value>)> = vec![
        (
            "one key, one partition, required members only",
            vec!["dt"],
            vec![json!({ "Values": ["2026-01-01"] })],
        ),
        (
            "two keys, three partitions, fully described storage",
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
                json!({
                    "Values": ["2026-01-02", "us-east-1"],
                    "Parameters": { "rows": "17" }
                }),
                json!({ "Values": ["2026-01-02", "eu-west-1"] }),
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
            "one key, five partitions spanning value forms",
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
        let database = format!("partitions_db_{index}");
        // Mixed case, so the attribution a partition reports is the FOLDED table name
        // the catalog stores.
        let table = format!("Events_{index}");
        let folded_table = table.to_lowercase();
        seed_table(&router, &database, &table, keys).await;

        let created = batch_create(&router, &database, &table, inputs).await;
        assert!(
            partition_errors(&created).is_empty(),
            "case {case}: every submitted partition must be created, got Errors {:?}",
            created["Errors"]
        );

        for input in inputs {
            let values = values_of(input);
            let (status, body) = get_partition(&router, &database, &table, &values).await;
            assert_eq!(
                status, 200,
                "case {case}: GetPartition {values:?} must answer 200, got {status} with {body}"
            );
            let partition = &body["Partition"];

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
                "case {case}: partition {values:?} must be attributed to its table by the folded \
                 name, got {:?}",
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

            // A read reports the stored partition and changes nothing about it, so the
            // next read of the same values answers exactly the same thing.
            let (status, again) = get_partition(&router, &database, &table, &values).await;
            assert_eq!(
                status, 200,
                "case {case}: a second GetPartition {values:?} must answer 200, got {status} \
                 with {again}"
            );
            assert_eq!(
                again["Partition"], *partition,
                "case {case}: a read must not change what a later read of {values:?} sees; \
                 expected {partition}, got {}",
                again["Partition"]
            );
        }
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
    /// The batch's `PartitionInputList`.
    inputs: Vec<Value>,
    /// The `ErrorCode` of each `Errors` entry, in order.
    expected_codes: Vec<&'static str>,
    /// The `PartitionValues` of each `Errors` entry, in order; `None` for an item that
    /// named no values.
    expected_values: Vec<Option<Vec<&'static str>>>,
    /// Value lists a read must serve after the batch.
    readable: Vec<Vec<&'static str>>,
    /// Value lists a read must NOT serve after the batch.
    missing: Vec<Vec<&'static str>>,
}

/// An item that cannot be created fails only its own item: the batch answers 200, names
/// that item in `Errors` with the error code for the reason it was refused, and leaves
/// every other item of the same batch created.
///
/// The rows differ along the way one item fails — a duplicate inside one batch, a
/// duplicate of a partition an earlier item of the same batch created — and along how many
/// items of the batch succeed beside it. An item whose value count is not the table's
/// partition-key count is NOT one of these: that refuses the whole request
/// ([`an_arity_mismatch_refuses_the_whole_request`]).
#[tokio::test]
async fn an_item_that_cannot_be_created_fails_only_itself() {
    let router = emulator();

    let cases = [
        FailureCase {
            case: "the same values twice in one batch",
            keys: vec!["dt"],
            inputs: vec![
                json!({ "Values": ["2026-01-01"] }),
                json!({ "Values": ["2026-01-01"] }),
            ],
            expected_codes: vec!["AlreadyExistsException"],
            expected_values: vec![Some(vec!["2026-01-01"])],
            readable: vec![vec!["2026-01-01"]],
            missing: vec![],
        },
        FailureCase {
            case: "two good items beside a duplicate of the first",
            keys: vec!["dt", "region"],
            inputs: vec![
                json!({ "Values": ["2026-01-01", "us-east-1"] }),
                json!({ "Values": ["2026-01-02", "eu-west-1"] }),
                json!({ "Values": ["2026-01-01", "us-east-1"] }),
            ],
            expected_codes: vec!["AlreadyExistsException"],
            expected_values: vec![Some(vec!["2026-01-01", "us-east-1"])],
            readable: vec![
                vec!["2026-01-01", "us-east-1"],
                vec!["2026-01-02", "eu-west-1"],
            ],
            missing: vec![vec!["2026-01-03", "us-east-1"]],
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let case = row.case;
        let database = format!("failing_db_{index}");
        let table = format!("failing_table_{index}");
        seed_table(&router, &database, &table, &row.keys).await;

        let body = batch_create(&router, &database, &table, &row.inputs).await;
        let reported = partition_errors(&body);
        assert_eq!(
            reported.len(),
            row.expected_codes.len(),
            "case {case}: expected {} per-item errors, got {:?}",
            row.expected_codes.len(),
            body["Errors"]
        );
        for (position, (entry, expected_code)) in
            reported.iter().zip(row.expected_codes.iter()).enumerate()
        {
            let detail = &entry["ErrorDetail"];
            assert_eq!(
                detail["ErrorCode"].as_str(),
                Some(*expected_code),
                "case {case}: error {position} must carry ErrorCode {expected_code}, got {entry}"
            );
            assert!(
                detail["ErrorMessage"]
                    .as_str()
                    .is_some_and(|message| !message.is_empty()),
                "case {case}: error {position} must carry an ErrorMessage, got {entry}"
            );
            assert_eq!(
                detail["__type"].as_str(),
                Some(ERROR_DETAIL_TAG),
                "case {case}: error {position} must carry the ErrorDetail type tag, got {entry}"
            );
            let expected_values = row
                .expected_values
                .get(position)
                .expect("a row states the values of every error it expects");
            match expected_values {
                Some(values) => assert_eq!(
                    entry["PartitionValues"],
                    json!(values),
                    "case {case}: error {position} must name the values its item asked for, \
                     got {entry}"
                ),
                None => assert!(
                    entry["PartitionValues"].is_null(),
                    "case {case}: error {position} names no values, because its item named \
                     none, got {entry}"
                ),
            }
        }

        for values in &row.readable {
            let (status, read) = get_partition(&router, &database, &table, values).await;
            assert_eq!(
                status, 200,
                "case {case}: the item {values:?} was created, so a read must serve it; got \
                 {status} with {read}"
            );
        }
        for values in &row.missing {
            let (status, read) = get_partition(&router, &database, &table, values).await;
            assert_eq!(
                status, 400,
                "case {case}: the refused item {values:?} must not have been stored; got \
                 {status} with {read}"
            );
        }
    }
}

/// One row of the request-level refusal table.
struct ArityCase {
    case: &'static str,
    /// The partition keys the addressed table defines.
    keys: Vec<&'static str>,
    /// The batch's `PartitionInputList`, at least one item of which carries a value count
    /// other than the table's key count.
    inputs: Vec<Value>,
    /// Value lists a read must NOT serve after the refused batch, because a refused
    /// request creates nothing at all — including the items that were well-formed.
    created_nothing: Vec<Vec<&'static str>>,
}

/// A `PartitionInput` whose value count is not the addressed table's partition-key count
/// refuses the WHOLE request with `InvalidInputException`: a partition is one value per
/// partition key of its table, so such an item describes a slice of no table and the
/// request is malformed rather than partly servable. The response carries no `Errors` list
/// at all, and nothing is created — not even the items of the same batch that were
/// well-formed.
///
/// The rows differ along how the count is wrong (fewer values than keys, more values than
/// keys, no `Values` member at all, a value for a table that defines no partition keys),
/// how many partition keys the table defines (0, 1, 2, 3), how many items the batch
/// carries and where in it the malformed item sits.
#[tokio::test]
async fn an_arity_mismatch_refuses_the_whole_request() {
    let router = emulator();

    let cases = [
        ArityCase {
            case: "fewer values than the table has partition keys",
            keys: vec!["dt", "region"],
            inputs: vec![json!({ "Values": ["2026-01-01"] })],
            created_nothing: vec![vec!["2026-01-01", "us-east-1"]],
        },
        ArityCase {
            case: "more values than the table has partition keys",
            keys: vec!["dt"],
            inputs: vec![json!({ "Values": ["2026-01-01", "us-east-1"] })],
            created_nothing: vec![vec!["2026-01-01"]],
        },
        ArityCase {
            case: "an input that names no values at all",
            keys: vec!["dt"],
            inputs: vec![json!({ "Parameters": { "classification": "csv" } })],
            created_nothing: vec![vec!["2026-01-01"]],
        },
        ArityCase {
            case: "a table that defines no partition keys",
            keys: vec![],
            inputs: vec![json!({ "Values": ["2026-01-01"] })],
            created_nothing: vec![vec!["2026-01-01"]],
        },
        ArityCase {
            case: "the malformed item last, after two well-formed ones",
            keys: vec!["dt"],
            inputs: vec![
                json!({ "Values": ["2026-01-01"] }),
                json!({ "Values": ["2026-01-02"] }),
                json!({ "Values": ["2026-01-03", "extra"] }),
            ],
            created_nothing: vec![vec!["2026-01-01"], vec!["2026-01-02"]],
        },
        ArityCase {
            case: "three keys and an item carrying two values",
            keys: vec!["year", "month", "day"],
            inputs: vec![
                json!({ "Values": ["2026", "01"] }),
                json!({ "Values": ["2026", "01", "02"] }),
            ],
            created_nothing: vec![vec!["2026", "01", "02"]],
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let case = row.case;
        let database = format!("arity_db_{index}");
        let table = format!("arity_table_{index}");
        seed_table(&router, &database, &table, &row.keys).await;

        let (status, body) = call(
            &router,
            "BatchCreatePartition",
            &json!({
                "DatabaseName": database,
                "TableName": table,
                "PartitionInputList": row.inputs
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: the whole request must be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {} in {body}",
            error_code(&body)
        );
        assert!(
            body["Errors"].is_null(),
            "case {case}: a request-level refusal carries no Errors list, got {body}"
        );

        for values in &row.created_nothing {
            let (status, read) = get_partition(&router, &database, &table, values).await;
            assert_eq!(
                status, 400,
                "case {case}: a refused request creates nothing, so {values:?} must not be \
                 readable; got {status} with {read}"
            );
        }
    }
}

/// A partition an EARLIER batch created is a duplicate for every later one: the second
/// create is refused per item and leaves the stored partition exactly as the first
/// create wrote it, rather than overwriting it.
#[tokio::test]
async fn an_earlier_created_partition_is_a_duplicate_and_is_not_overwritten() {
    let router = emulator();
    seed_table(&router, "duplicate_db", "duplicate_table", &["dt"]).await;

    let first = batch_create(
        &router,
        "duplicate_db",
        "duplicate_table",
        &[json!({ "Values": ["2026-01-01"], "Parameters": { "generation": "first" } })],
    )
    .await;
    assert!(
        partition_errors(&first).is_empty(),
        "the first create must succeed, got Errors {:?}",
        first["Errors"]
    );
    let (_, stored) = get_partition(
        &router,
        "duplicate_db",
        "duplicate_table",
        &["2026-01-01"],
    )
    .await;

    let second = batch_create(
        &router,
        "duplicate_db",
        "duplicate_table",
        &[json!({ "Values": ["2026-01-01"], "Parameters": { "generation": "second" } })],
    )
    .await;
    let reported = partition_errors(&second);
    assert_eq!(
        reported.len(),
        1,
        "the duplicate create must fail its one item, got Errors {:?}",
        second["Errors"]
    );
    assert_eq!(
        reported[0]["ErrorDetail"]["ErrorCode"].as_str(),
        Some("AlreadyExistsException"),
        "a duplicate partition must be refused as already existing, got {:?}",
        reported[0]
    );

    let (status, after) = get_partition(
        &router,
        "duplicate_db",
        "duplicate_table",
        &["2026-01-01"],
    )
    .await;
    assert_eq!(
        status, 200,
        "the partition the first create wrote must still be readable, got {status} with {after}"
    );
    assert_eq!(
        after["Partition"], stored["Partition"],
        "a refused duplicate must not overwrite the stored partition; expected {}, got {}",
        stored["Partition"], after["Partition"]
    );
}

/// An empty `PartitionInputList` is a valid input (`PartitionInputList`, length 0..100,
/// from `smithy_query` on `BatchCreatePartition`): it creates nothing and reports no
/// per-item error.
#[tokio::test]
async fn an_empty_batch_creates_nothing_and_reports_no_errors() {
    let router = emulator();
    seed_table(&router, "empty_batch_db", "empty_batch_table", &["dt"]).await;

    let body = batch_create(&router, "empty_batch_db", "empty_batch_table", &[]).await;
    assert!(
        partition_errors(&body).is_empty(),
        "an empty batch reports no error, got {body}"
    );
    let (status, read) =
        get_partition(&router, "empty_batch_db", "empty_batch_table", &["2026-01-01"]).await;
    assert_eq!(
        status, 400,
        "an empty batch creates no partition, so a read must still miss; got {status} with {read}"
    );
}

// ---------------------------------------------------------------------------
// Whole-request refusals
// ---------------------------------------------------------------------------

/// A database or table the catalog does not hold fails the WHOLE batch — there is no
/// table to write any item onto — with `EntityNotFoundException` and the wording this
/// service answers for that resource.
#[tokio::test]
async fn a_batch_naming_no_stored_table_is_refused_whole() {
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
            "BatchCreatePartition",
            &json!({
                "DatabaseName": database,
                "TableName": table,
                "PartitionInputList": [ { "Values": ["2026-01-01"] } ]
            }),
            "EntityNotFoundException",
            expected_message,
        )
        .await;
    }
}

/// A read of a partition the catalog holds none of is `EntityNotFoundException`, and so
/// is a read through a database or table it holds none of. The partition miss rows also
/// pin that a partition value is matched EXACTLY as sent: a value differing only in case
/// addresses a different partition.
#[tokio::test]
async fn a_read_of_an_absent_partition_is_refused() {
    let router = emulator();
    seed_table(&router, "reads_db", "reads_table", &["dt"]).await;
    let created = batch_create(
        &router,
        "reads_db",
        "reads_table",
        &[
            json!({ "Values": ["2026-01-01"] }),
            json!({ "Values": ["Monday"] }),
        ],
    )
    .await;
    assert!(
        partition_errors(&created).is_empty(),
        "the fixture partitions must be created, got Errors {:?}",
        created["Errors"]
    );

    let cases = vec![
        (
            "no such database",
            "ghost_db",
            "reads_table",
            vec!["2026-01-01"],
            "Database ghost_db not found.",
        ),
        (
            "no such table",
            "reads_db",
            "ghost_table",
            vec!["2026-01-01"],
            "Table ghost_table not found.",
        ),
        (
            "a value no partition of the table carries",
            "reads_db",
            "reads_table",
            vec!["2026-01-09"],
            PARTITION_NOT_FOUND,
        ),
        (
            "a value differing from a stored one only in case",
            "reads_db",
            "reads_table",
            vec!["monday"],
            PARTITION_NOT_FOUND,
        ),
        (
            "more values than the table has partition keys",
            "reads_db",
            "reads_table",
            vec!["2026-01-01", "us-east-1"],
            PARTITION_NOT_FOUND,
        ),
    ];

    for (case, database, table, values, expected_message) in cases {
        assert_refusal(
            &router,
            case,
            "GetPartition",
            &json!({
                "DatabaseName": database,
                "TableName": table,
                "PartitionValues": values
            }),
            "EntityNotFoundException",
            expected_message,
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// Capacity
// ---------------------------------------------------------------------------

/// One table holds at most [`MAX_PARTITIONS_PER_TABLE`] partitions: the create past the
/// ceiling is refused per item, as a resource limit rather than as a bad input, and the
/// partitions already stored stay readable.
#[tokio::test]
async fn a_table_holds_at_most_the_partition_ceiling() {
    let router = emulator();
    seed_table(&router, "capacity_db", "capacity_table", &["n"]).await;

    let mut created = 0usize;
    while created < MAX_PARTITIONS_PER_TABLE {
        let room = (MAX_PARTITIONS_PER_TABLE - created).min(MAX_PARTITIONS_PER_BATCH);
        let inputs: Vec<Value> = (created..created + room)
            .map(|n| json!({ "Values": [n.to_string()] }))
            .collect();
        let body = batch_create(&router, "capacity_db", "capacity_table", &inputs).await;
        assert!(
            partition_errors(&body).is_empty(),
            "the first {MAX_PARTITIONS_PER_TABLE} partitions must be created, got Errors {:?}",
            body["Errors"]
        );
        created += room;
    }

    let over = batch_create(
        &router,
        "capacity_db",
        "capacity_table",
        &[json!({ "Values": [MAX_PARTITIONS_PER_TABLE.to_string()] })],
    )
    .await;
    let reported = partition_errors(&over);
    assert_eq!(
        reported.len(),
        1,
        "the create past the ceiling must fail its one item, got Errors {:?}",
        over["Errors"]
    );
    assert_eq!(
        reported[0]["ErrorDetail"]["ErrorCode"].as_str(),
        Some("ResourceNumberLimitExceededException"),
        "a create past the ceiling must be refused as a resource limit, got {:?}",
        reported[0]
    );

    let (status, read) = get_partition(&router, "capacity_db", "capacity_table", &["0"]).await;
    assert_eq!(
        status, 200,
        "a refused create must not disturb the partitions already stored; got {status} with \
         {read}"
    );
}
