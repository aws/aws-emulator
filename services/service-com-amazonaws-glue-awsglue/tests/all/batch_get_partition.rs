// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue batch partition read, `BatchGetPartition`.
//!
//! Every call goes over the wire the way a client makes it, through the shared
//! [`crate::wire`] harness: `POST /` with `X-Amz-Target: AWSGlue.<Operation>` and an
//! `awsJson1_1` body, dispatched by the same `MultiServiceRouter` the production front
//! door uses — so what is asserted here is HTTP behaviour and not a direct call into a
//! handler.
//!
//! The rule under test: a batch read of partitions answers, in the order its keys were
//! sent, the stored partition each requested value list addresses on the table the
//! request names — projected exactly as the single-partition read projects one — and a
//! value list the table holds no partition for contributes NOTHING to the answer instead
//! of failing the request. A database or table the catalog does not hold fails the whole
//! request with `EntityNotFoundException`, as it does for the single-partition read.
//!
//! The suites are tables whose rows differ along what the rule generalises over: how many
//! partition keys the table defines, how many keys one request carries, how many of them
//! the table holds a partition for, the order and multiplicity of the requested keys, the
//! form and length of a partition value, and the member set the stored partition carries.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::assert_refusal;
use crate::partitions::{
    batch_create, get_partition, instant, partition_errors, seed_table, values_of,
    without_instants,
};
use crate::wire::{assert_echoed, call, emulator, error_code, ACCOUNT_ID};

/// The bounds of ONE partition value: `ValueString`, length 1..1024, from `smithy_query`
/// on `ValueStringList`'s member.
const MIN_VALUE_LENGTH: usize = 1;
const MAX_VALUE_LENGTH: usize = 1024;

/// One requested value list, as a list of owned values.
fn vals(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}

/// `values` as the `&[&str]` the single-partition read is addressed by.
fn as_refs(values: &[String]) -> Vec<&str> {
    values.iter().map(String::as_str).collect()
}

/// Submit one `BatchGetPartition` for `keys`, returning its HTTP status and body.
async fn batch_get(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    keys: &[Vec<String>],
) -> (u16, Value) {
    call(router, "BatchGetPartition", &request(database, table, keys)).await
}

/// The request body of a batch read: each requested value list is one
/// `PartitionValueList` carrying its `Values`.
fn request(database: &str, table: &str, keys: &[Vec<String>]) -> Value {
    let to_get: Vec<Value> = keys
        .iter()
        .map(|values| json!({ "Values": values }))
        .collect();
    json!({
        "DatabaseName": database,
        "TableName": table,
        "PartitionsToGet": to_get,
    })
}

/// The `Partitions` list of a batch read that must have answered 200.
fn partitions_of<'body>(case: &str, status: u16, body: &'body Value) -> &'body Vec<Value> {
    assert_eq!(
        status, 200,
        "case {case}: BatchGetPartition must answer 200, got {status} with {body}"
    );
    body["Partitions"].as_array().unwrap_or_else(|| {
        panic!("case {case}: BatchGetPartition must report a Partitions list, got {body}")
    })
}

/// The `Values` of each partition in the answer, in the order it was returned.
fn returned_values(case: &str, partitions: &[Value]) -> Vec<Vec<String>> {
    partitions
        .iter()
        .map(|partition| {
            partition["Values"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case}: a returned partition must carry Values, got {partition}")
                })
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case}: a partition value is a string, got {value}")
                        })
                        .to_string()
                })
                .collect()
        })
        .collect()
}

/// Assert the answer claims no unprocessed key, whichever keys the request named.
///
/// `UnprocessedKeys` is how the batch read says it did not get to a key — the truncation
/// a batch too large for one response is reported by — so a key whose partition simply
/// does not exist must never appear there: it was answered, and the answer is that the
/// table holds no such partition.
fn assert_no_unprocessed_keys(case: &str, body: &Value) {
    let unprocessed = body.get("UnprocessedKeys");
    assert!(
        unprocessed.is_none_or(|keys| keys.as_array().is_some_and(Vec::is_empty)),
        "case {case}: every requested key was answered, so none of them is an unprocessed \
         key, got {body}"
    );
}

/// One row of the batch-read table.
struct Row {
    case: &'static str,
    /// The partition keys the addressed table defines.
    keys: Vec<&'static str>,
    /// The value lists created on that table before the read.
    stored: Vec<Vec<String>>,
    /// The `PartitionsToGet` of the batch read.
    requested: Vec<Vec<String>>,
    /// The `Values` of each partition the answer must carry, in order.
    expected: Vec<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Which partitions a batch read answers, and in which order
// ---------------------------------------------------------------------------

/// A batch read answers one partition per requested key the table holds a partition for,
/// in the order the keys were sent, and omits the keys it holds none for.
///
/// The rows differ along the number of partition keys (1, 2, 3), the number of keys one
/// request carries (0, 1, 3, 5, 64), how many of them are stored (all, some, none), the
/// order of the keys against the order the partitions were created in, whether a key is
/// repeated, and the form and length of a partition value (a date, a Region, digits, a
/// value with spaces, a non-ASCII value, values differing only in case, and a value at
/// each published length bound).
#[tokio::test]
async fn a_batch_read_answers_the_requested_keys_it_holds_partitions_for() {
    let router = emulator();

    let shortest = "v".repeat(MIN_VALUE_LENGTH);
    let longest = "w".repeat(MAX_VALUE_LENGTH);
    // A request carrying many keys, every other one of which is stored: the count axis of
    // `PartitionsToGet` (length 0..1000, from `smithy_query` on `BatchGetPartition`).
    let many_stored: Vec<Vec<String>> = (0..32).map(|n| vals(&[&format!("{}", n * 2)])).collect();
    let many_requested: Vec<Vec<String>> = (0..64).map(|n| vals(&[&format!("{n}")])).collect();
    let many_expected: Vec<Vec<String>> = many_requested
        .iter()
        .filter(|values| many_stored.contains(values))
        .cloned()
        .collect();

    let rows = vec![
        Row {
            case: "one key, both requested keys stored, requested in reverse of create order",
            keys: vec!["dt"],
            stored: vec![vals(&["2026-01-01"]), vals(&["2026-01-02"])],
            requested: vec![vals(&["2026-01-02"]), vals(&["2026-01-01"])],
            expected: vec![vals(&["2026-01-02"]), vals(&["2026-01-01"])],
        },
        Row {
            case: "two keys, misses interleaved between the stored keys",
            keys: vec!["dt", "region"],
            stored: vec![
                vals(&["2026-01-01", "us-east-1"]),
                vals(&["2026-01-01", "eu-west-1"]),
                vals(&["2026-01-02", "us-east-1"]),
            ],
            requested: vec![
                vals(&["2026-01-01", "ap-south-1"]),
                vals(&["2026-01-02", "us-east-1"]),
                vals(&["2026-01-09", "us-east-1"]),
                vals(&["2026-01-01", "eu-west-1"]),
                vals(&["2026-01-03", "eu-west-1"]),
            ],
            expected: vec![
                vals(&["2026-01-02", "us-east-1"]),
                vals(&["2026-01-01", "eu-west-1"]),
            ],
        },
        Row {
            case: "three keys, the same key asked for twice",
            keys: vec!["year", "month", "day"],
            stored: vec![vals(&["2026", "01", "02"])],
            requested: vec![
                vals(&["2026", "01", "02"]),
                vals(&["2026", "01", "03"]),
                vals(&["2026", "01", "02"]),
            ],
            // Each item is answered on its own, so a key sent twice is answered twice.
            expected: vec![vals(&["2026", "01", "02"]), vals(&["2026", "01", "02"])],
        },
        Row {
            case: "a value differing from a stored one only in case is a different key",
            keys: vec!["k"],
            stored: vec![vals(&["Monday"])],
            requested: vec![vals(&["Monday"]), vals(&["monday"]), vals(&["MONDAY"])],
            expected: vec![vals(&["Monday"])],
        },
        Row {
            case: "values with spaces, digits and non-ASCII characters",
            keys: vec!["k"],
            stored: vec![
                vals(&["a value with spaces"]),
                vals(&["100"]),
                vals(&["ünïcode=yes"]),
            ],
            requested: vec![
                vals(&["ünïcode=yes"]),
                vals(&["100"]),
                vals(&["a value with spaces"]),
            ],
            expected: vec![
                vals(&["ünïcode=yes"]),
                vals(&["100"]),
                vals(&["a value with spaces"]),
            ],
        },
        Row {
            case: "a value at each published length bound",
            keys: vec!["k"],
            stored: vec![vals(&[&shortest]), vals(&[&longest])],
            requested: vec![
                vals(&[&shortest]),
                vals(&[&"w".repeat(MAX_VALUE_LENGTH - 1)]),
                vals(&[&longest]),
            ],
            expected: vec![vals(&[&shortest]), vals(&[&longest])],
        },
        Row {
            case: "no requested key is stored",
            keys: vec!["dt"],
            stored: vec![vals(&["2026-01-01"])],
            requested: vec![vals(&["2026-02-01"]), vals(&["2026-03-01"])],
            expected: vec![],
        },
        Row {
            case: "no key requested at all",
            keys: vec!["dt"],
            stored: vec![vals(&["2026-01-01"])],
            requested: vec![],
            expected: vec![],
        },
        Row {
            case: "sixty-four keys requested, every other one stored",
            keys: vec!["n"],
            stored: many_stored,
            requested: many_requested,
            expected: many_expected,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let database = format!("batch_get_db_{index}");
        let table = format!("batch_get_table_{index}");
        seed_table(&router, &database, &table, &row.keys).await;
        if !row.stored.is_empty() {
            let inputs: Vec<Value> = row
                .stored
                .iter()
                .map(|values| json!({ "Values": values }))
                .collect();
            let created = batch_create(&router, &database, &table, &inputs).await;
            assert!(
                partition_errors(&created).is_empty(),
                "case {case}: the fixture partitions must be created, got Errors {:?}",
                created["Errors"]
            );
        }

        let (status, body) = batch_get(&router, &database, &table, &row.requested).await;
        let partitions = partitions_of(case, status, &body);
        assert_eq!(
            returned_values(case, partitions),
            row.expected,
            "case {case}: expected the partitions {:?} in request order, got {}",
            row.expected,
            body["Partitions"]
        );
        assert_no_unprocessed_keys(case, &body);

        // Each partition the batch answers is the SAME projection the single-partition
        // read serves for those values: one read of the store, reported one way.
        for (position, (partition, values)) in partitions.iter().zip(row.expected.iter()).enumerate()
        {
            let (single_status, single) =
                get_partition(&router, &database, &table, &as_refs(values)).await;
            assert_eq!(
                single_status, 200,
                "case {case}: GetPartition {values:?} must serve what the batch answered, got \
                 {single_status} with {single}"
            );
            assert_eq!(
                *partition, single["Partition"],
                "case {case}: partition {position} ({values:?}) must be projected as the single \
                 read projects it; expected {}, got {partition}",
                single["Partition"]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What each answered partition carries
// ---------------------------------------------------------------------------

/// Whatever a client stored on a partition is what the batch read reports for it, beside
/// the server-set `CreationTime` and the catalog's attribution of the partition to its
/// catalog, database and table.
///
/// The rows differ along the member set the stored partition carries: bare values, a
/// fully described storage descriptor with parameters, and both access instants.
#[tokio::test]
async fn a_batch_read_reports_what_each_partition_was_created_with() {
    let router = emulator();
    // Mixed case, so the attribution a partition reports is the FOLDED table name the
    // catalog stores.
    let database = "batch_get_members_db";
    let table = "Batch_Get_Members_Table";
    let folded_table = table.to_lowercase();
    seed_table(&router, database, table, &["dt", "region"]).await;

    let inputs = vec![
        json!({ "Values": ["2026-01-01", "us-east-1"] }),
        json!({
            "Values": ["2026-01-02", "us-east-1"],
            "StorageDescriptor": {
                "Columns": [
                    { "Name": "id", "Type": "string" },
                    { "Name": "amount", "Type": "double", "Comment": "cents" }
                ],
                "Location": "s3://bucket/events/dt=2026-01-02/region=us-east-1/",
                "InputFormat": "org.apache.hadoop.mapred.TextInputFormat",
                "OutputFormat": "org.apache.hadoop.hive.ql.io.HiveIgnoreKeyTextOutputFormat",
                "Compressed": true,
                "NumberOfBuckets": 2,
                "BucketColumns": ["id"],
                "SortColumns": [ { "Column": "id", "SortOrder": 1 } ],
                "SerdeInfo": {
                    "Name": "csv",
                    "SerializationLibrary": "org.apache.hadoop.hive.serde2.OpenCSVSerde",
                    "Parameters": { "separatorChar": "," }
                },
                "Parameters": { "skip.header.line.count": "1" }
            },
            "Parameters": { "classification": "csv" }
        }),
        json!({
            "Values": ["2026-01-03", "eu-west-1"],
            "LastAccessTime": 1_767_225_600,
            "LastAnalyzedTime": 1_767_312_000
        }),
    ];
    let created = batch_create(&router, database, table, &inputs).await;
    assert!(
        partition_errors(&created).is_empty(),
        "the fixture partitions must be created, got Errors {:?}",
        created["Errors"]
    );

    let requested: Vec<Vec<String>> = inputs
        .iter()
        .map(|input| {
            values_of(input)
                .iter()
                .map(|value| (*value).to_string())
                .collect()
        })
        .collect();
    let (status, body) = batch_get(&router, database, table, &requested).await;
    let partitions = partitions_of("every member set", status, &body);
    assert_eq!(
        partitions.len(),
        inputs.len(),
        "every stored partition must be answered, got {body}"
    );

    for (input, partition) in inputs.iter().zip(partitions.iter()) {
        let case = input["Values"].to_string();
        // The instants are compared numerically below: awsJson renders a timestamp as a
        // JSON number, and whether that number carries a fractional part is the
        // serializer's choice and not part of the value.
        assert_echoed(&case, &without_instants(input), partition);
        for member in ["LastAccessTime", "LastAnalyzedTime"] {
            assert_eq!(
                instant(partition, member),
                instant(input, member),
                "case {case}: the batch must report the {member} the partition was created \
                 with, got {:?}",
                partition.get(member)
            );
        }
        assert_eq!(
            partition["DatabaseName"].as_str(),
            Some(database),
            "case {case}: the partition must be attributed to its database, got {partition}"
        );
        assert_eq!(
            partition["TableName"].as_str(),
            Some(folded_table.as_str()),
            "case {case}: the partition must be attributed to its table by the folded name, \
             got {partition}"
        );
        assert_eq!(
            partition["CatalogId"].as_str(),
            Some(ACCOUNT_ID),
            "case {case}: the partition must be attributed to the default catalog, got {partition}"
        );
        assert!(
            partition["CreationTime"].as_f64().is_some_and(|at| at > 0.0),
            "case {case}: the partition must carry a populated CreationTime, got {partition}"
        );
    }
}

// ---------------------------------------------------------------------------
// A batch read is a read
// ---------------------------------------------------------------------------

/// A batch read reports the store and changes nothing a later caller sees: the same
/// request answers the same partitions again, a key it answered nothing for is still
/// absent afterwards, and a partition it answered is still served by the single read.
#[tokio::test]
async fn a_batch_read_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    seed_table(&router, "batch_get_reads_db", "batch_get_reads_table", &["dt"]).await;
    let created = batch_create(
        &router,
        "batch_get_reads_db",
        "batch_get_reads_table",
        &[
            json!({ "Values": ["2026-01-01"], "Parameters": { "rows": "17" } }),
            json!({ "Values": ["2026-01-02"] }),
        ],
    )
    .await;
    assert!(
        partition_errors(&created).is_empty(),
        "the fixture partitions must be created, got Errors {:?}",
        created["Errors"]
    );

    let requested = vec![
        vals(&["2026-01-01"]),
        vals(&["2026-01-09"]),
        vals(&["2026-01-02"]),
    ];
    let (status, first) = batch_get(
        &router,
        "batch_get_reads_db",
        "batch_get_reads_table",
        &requested,
    )
    .await;
    partitions_of("first read", status, &first);

    for attempt in ["second read", "third read"] {
        let (status, again) = batch_get(
            &router,
            "batch_get_reads_db",
            "batch_get_reads_table",
            &requested,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {attempt}: a repeated batch read must answer 200, got {status} with {again}"
        );
        assert_eq!(
            again["Partitions"], first["Partitions"],
            "case {attempt}: a read must not change what a later read sees; expected {}, got {}",
            first["Partitions"], again["Partitions"]
        );
    }

    // The key the batch answered nothing for is still absent: a read of a missing
    // partition creates nothing.
    let (status, missed) = get_partition(
        &router,
        "batch_get_reads_db",
        "batch_get_reads_table",
        &["2026-01-09"],
    )
    .await;
    assert_eq!(
        status, 400,
        "a key the batch omitted must still address no partition, got {status} with {missed}"
    );
}

/// A batch read resolves its database and table the way every catalog read does — by the
/// folded name — and only inside the catalog the request names.
#[tokio::test]
async fn a_batch_read_resolves_its_table_by_the_folded_name_in_the_named_catalog() {
    let router = emulator();
    seed_table(&router, "Folded_DB", "Folded_Table", &["dt"]).await;
    let created = batch_create(
        &router,
        "Folded_DB",
        "Folded_Table",
        &[json!({ "Values": ["2026-01-01"] })],
    )
    .await;
    assert!(
        partition_errors(&created).is_empty(),
        "the fixture partition must be created, got Errors {:?}",
        created["Errors"]
    );

    let requested = vec![vals(&["2026-01-01"])];
    for (case, database, table) in [
        ("as created", "Folded_DB", "Folded_Table"),
        ("lower case", "folded_db", "folded_table"),
        ("upper case", "FOLDED_DB", "FOLDED_TABLE"),
    ] {
        let (status, body) = batch_get(&router, database, table, &requested).await;
        let partitions = partitions_of(case, status, &body);
        assert_eq!(
            returned_values(case, partitions),
            requested,
            "case {case}: the folded name must address the same table, got {body}"
        );
    }

    // The default catalog is this account, so naming it explicitly is the same request.
    let (status, body) = call(
        &router,
        "BatchGetPartition",
        &json!({
            "CatalogId": ACCOUNT_ID,
            "DatabaseName": "folded_db",
            "TableName": "folded_table",
            "PartitionsToGet": [ { "Values": ["2026-01-01"] } ],
        }),
    )
    .await;
    let partitions = partitions_of("the default catalog, named", status, &body);
    assert_eq!(
        returned_values("the default catalog, named", partitions),
        requested,
        "naming the default catalog must address the same table, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Whole-request refusals
// ---------------------------------------------------------------------------

/// A database or table the addressed catalog does not hold fails the WHOLE batch read —
/// there is no table to read any key from — with `EntityNotFoundException` and the
/// wording this service answers for that resource, whatever the request's keys are.
#[tokio::test]
async fn a_batch_read_naming_no_stored_table_is_refused_whole() {
    let router = emulator();
    seed_table(
        &router,
        "batch_get_present_db",
        "batch_get_present_table",
        &["dt"],
    )
    .await;

    let one_key = vec![vals(&["2026-01-01"])];
    let no_key: Vec<Vec<String>> = vec![];
    let cases = vec![
        (
            "no such database",
            "ghost_db",
            "batch_get_present_table",
            &one_key,
            "Database ghost_db not found.",
        ),
        (
            "no such database, mixed case",
            "Ghost_DB",
            "batch_get_present_table",
            &one_key,
            "Database ghost_db not found.",
        ),
        (
            "no such database, no key requested",
            "ghost_db",
            "batch_get_present_table",
            &no_key,
            "Database ghost_db not found.",
        ),
        (
            "no such table",
            "batch_get_present_db",
            "ghost_table",
            &one_key,
            "Table ghost_table not found.",
        ),
        (
            "no such table, mixed case",
            "batch_get_present_db",
            "Ghost_Table",
            &one_key,
            "Table ghost_table not found.",
        ),
        (
            "no such table, no key requested",
            "batch_get_present_db",
            "ghost_table",
            &no_key,
            "Table ghost_table not found.",
        ),
    ];

    for (case, database, table, keys, expected_message) in cases {
        assert_refusal(
            &router,
            case,
            "BatchGetPartition",
            &request(database, table, keys),
            "EntityNotFoundException",
            expected_message,
        )
        .await;
    }

    // Another catalog holds none of this account's databases, so the same request against
    // it is the database miss.
    let mut other_catalog = request(
        "batch_get_present_db",
        "batch_get_present_table",
        &one_key,
    );
    other_catalog["CatalogId"] = json!("210987654321");
    assert_refusal(
        &router,
        "another catalog",
        "BatchGetPartition",
        &other_catalog,
        "EntityNotFoundException",
        "Database batch_get_present_db not found.",
    )
    .await;
}

/// One row of the wrong-arity table.
struct ArityRow {
    case: &'static str,
    /// The partition keys the addressed table defines.
    keys: Vec<&'static str>,
    /// The one value list created on that table, of the table's own arity.
    stored: Vec<String>,
    /// The requested value list whose length is not that arity.
    wrong_arity: Vec<String>,
}

/// A requested value list whose length is not the table's partition-key count addresses
/// no slice of that table, so it is a miss like any other key the table holds no
/// partition for — and it does not fail the keys beside it.
///
/// The rows differ along the table's partition-key count and along whether the bad key
/// carries too few or too many values.
#[tokio::test]
async fn a_requested_key_of_the_wrong_arity_is_a_miss() {
    let router = emulator();

    let cases = [
        ArityRow {
            case: "one key table, two values requested",
            keys: vec!["dt"],
            stored: vals(&["2026-01-01"]),
            wrong_arity: vals(&["2026-01-01", "us-east-1"]),
        },
        ArityRow {
            case: "two key table, one value requested",
            keys: vec!["dt", "region"],
            stored: vals(&["2026-01-01", "us-east-1"]),
            wrong_arity: vals(&["2026-01-01"]),
        },
        ArityRow {
            case: "two key table, three values requested",
            keys: vec!["dt", "region"],
            stored: vals(&["2026-01-01", "us-east-1"]),
            wrong_arity: vals(&["2026-01-01", "us-east-1", "extra"]),
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let ArityRow {
            case,
            keys,
            stored,
            wrong_arity,
        } = row;
        let database = format!("arity_db_{index}");
        let table = format!("arity_table_{index}");
        seed_table(&router, &database, &table, keys).await;
        let created = batch_create(
            &router,
            &database,
            &table,
            &[json!({ "Values": stored })],
        )
        .await;
        assert!(
            partition_errors(&created).is_empty(),
            "case {case}: the fixture partition must be created, got Errors {:?}",
            created["Errors"]
        );

        let requested = vec![wrong_arity.clone(), stored.clone()];
        let (status, body) = batch_get(&router, &database, &table, &requested).await;
        let partitions = partitions_of(case, status, &body);
        assert_eq!(
            returned_values(case, partitions),
            vec![stored.clone()],
            "case {case}: the wrong-arity key must be a miss and the good key must still be \
             answered, got {body}"
        );
        assert_no_unprocessed_keys(case, &body);
    }
}

/// A batch read asking this read to take part in a Lake Formation integration is refused
/// as not implemented — 501 `NotImplemented` — rather than served as if the audit record
/// had been written or the query session honoured.
///
/// The rows are the two members that ask for it, each on its own and both together.
#[tokio::test]
async fn a_batch_read_asking_for_lake_formation_context_is_refused_as_unimplemented() {
    let router = emulator();
    seed_table(&router, "context_db", "context_table", &["dt"]).await;
    let created = batch_create(
        &router,
        "context_db",
        "context_table",
        &[json!({ "Values": ["2026-01-01"] })],
    )
    .await;
    assert!(
        partition_errors(&created).is_empty(),
        "the fixture partition must be created, got Errors {:?}",
        created["Errors"]
    );

    let audit = json!({ "AdditionalAuditContext": "a query the caller is auditing" });
    let session = json!({ "QueryId": "a1b2c3", "ClusterId": "consumer-cluster" });
    let cases = vec![
        ("an audit context", Some(&audit), None),
        ("a query session context", None, Some(&session)),
        ("both contexts", Some(&audit), Some(&session)),
    ];

    for (case, audit_context, query_session_context) in cases {
        let mut body = request(
            "context_db",
            "context_table",
            &[vals(&["2026-01-01"])],
        );
        if let Some(audit_context) = audit_context {
            body["AuditContext"] = audit_context.clone();
        }
        if let Some(query_session_context) = query_session_context {
            body["QuerySessionContext"] = query_session_context.clone();
        }

        let (status, answer) = call(&router, "BatchGetPartition", &body).await;
        assert_eq!(
            status, 501,
            "case {case}: the batch read must be refused as not implemented, got {status} with \
             {answer}"
        );
        assert!(
            error_code(&answer).contains("NotImplemented"),
            "case {case}: expected NotImplemented, got {answer}"
        );
    }

    // The refusal is about the context, not about the table: the same read without it is
    // still served.
    let (status, served) = batch_get(
        &router,
        "context_db",
        "context_table",
        &[vals(&["2026-01-01"])],
    )
    .await;
    let partitions = partitions_of("without any context", status, &served);
    assert_eq!(
        returned_values("without any context", partitions),
        vec![vals(&["2026-01-01"])],
        "the same read without a Lake Formation context must be served, got {served}"
    );
}
