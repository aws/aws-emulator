// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `BatchUpdatePartition` and the `GetPartition` read that serves
//! what the batch wrote.
//!
//! Every call goes over the wire the way a client makes it, through the shared
//! [`crate::wire`] harness: `POST /` with `X-Amz-Target: AWSGlue.<Operation>` and an
//! `awsJson1_1` body, dispatched by the same `MultiServiceRouter` the production front
//! door uses — so what is asserted here is HTTP behaviour and not a direct call into a
//! handler.
//!
//! The rule under test: a batch partition update writes each entry's `PartitionInput`
//! over the WHOLE client-settable definition of the partition that entry addresses by its
//! `PartitionValueList` — so the next read reports exactly the submitted members and
//! nothing the entry omitted, while the partition keeps the address and the server-set
//! `CreationTime` its create wrote. An entry that cannot be applied (no such partition,
//! or a `PartitionInput` that does not define that same partition) fails ONLY its own
//! entry, reported in `Errors`; a database or table the catalog does not hold fails the
//! whole request.
//!
//! The suites are tables whose rows differ along what the rule generalises over: how many
//! partition keys the table defines, which members the create and the update each carry,
//! how many entries one batch submits, the form of a partition value, and the way one
//! entry can fail.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::messages::{
    PARTITION_VALUES_CANNOT_BE_CHANGED, PARTITION_VALUES_REQUIRED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::assert_refusal;
use crate::partitions::{
    batch_create, generation, instant, partition_errors, seed_table, stored, values_of,
    without_instants, SETTABLE_MEMBERS,
};
use crate::wire::{assert_echoed, emulator, ok, ACCOUNT_ID, ERROR_DETAIL_TAG};

/// The sentence an entry that addresses a value list the table holds no partition for is
/// refused with, written here independently of the service so the suite pins the wording and
/// not the code: fixed text around the values that entry addressed, which is NOT the sentence
/// a partition READ answers for the same absent partition.
fn update_miss(values: &[&str]) -> String {
    format!("Partition [{}] not found", values.join(", "))
}

/// Submit one `BatchUpdatePartition` that must answer 200, and return its body.
async fn batch_update(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    entries: &[Value],
) -> Value {
    ok(
        router,
        "BatchUpdatePartition",
        &json!({
            "DatabaseName": database,
            "TableName": table,
            "Entries": entries
        }),
    )
    .await
}

/// The `Errors` list of a batch update response, which the service reports whether or not
/// anything failed.
fn update_errors(body: &Value) -> &[Value] {
    body["Errors"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_else(|| panic!("BatchUpdatePartition must report an Errors list, got {body}"))
}

/// One entry addressing the partition its own `PartitionInput` defines, which is the only
/// address an update may write to.
fn entry(input: &Value) -> Value {
    json!({ "PartitionValueList": values_of(input), "PartitionInput": input })
}

// ---------------------------------------------------------------------------
// Batch update -> read round trip
// ---------------------------------------------------------------------------

/// One row of the round-trip table: the definitions a create stores and the ones an
/// update writes over them.
struct UpdateCase {
    case: &'static str,
    /// The partition keys the addressed table defines.
    keys: Vec<&'static str>,
    /// The `PartitionInput`s the fixture batch create stores.
    created: Vec<Value>,
    /// The `PartitionInput`s the update submits, each addressing its own `Values`.
    updated: Vec<Value>,
}

/// Whatever a client submits to `BatchUpdatePartition` is what `GetPartition` reads back:
/// every member the entry carried, nothing it omitted, and the address and `CreationTime`
/// the create wrote.
///
/// The rows differ along the number of partition keys (1, 2, 3), the member sets the
/// create and the update carry (bare values updated to a fully described partition; a
/// described partition updated back to bare values; instants replaced by other instants),
/// how many entries one batch updates (1, 2, 3) and the form of a partition value (a
/// date, a Region, digits, a value with spaces, a non-ASCII value).
#[tokio::test]
async fn batch_updated_partitions_are_read_back_from_the_store() {
    let router = emulator();

    for (index, row) in round_trip_cases().iter().enumerate() {
        let case = row.case;
        let database = format!("update_db_{index}");
        // Mixed case, so the attribution a partition reports stays the FOLDED table name
        // the catalog stores.
        let table = format!("Events_{index}");
        let folded_table = table.to_lowercase();
        seed_table(&router, &database, &table, &row.keys).await;

        let created = batch_create(&router, &database, &table, &row.created).await;
        assert!(
            partition_errors(&created).is_empty(),
            "case {case}: the fixture partitions must be created, got Errors {:?}",
            created["Errors"]
        );
        // What the create wrote, read before the update so the members only the server
        // sets can be compared across it.
        let mut before: Vec<Value> = Vec::new();
        for input in &row.created {
            before.push(stored(&router, &database, &table, &values_of(input)).await);
        }

        let entries: Vec<Value> = row.updated.iter().map(entry).collect();
        let body = batch_update(&router, &database, &table, &entries).await;
        assert!(
            update_errors(&body).is_empty(),
            "case {case}: every submitted entry must be applied, got {body}"
        );

        for input in &row.updated {
            let values = values_of(input);
            let partition = stored(&router, &database, &table, &values).await;

            // Every member the update sent comes back ...
            assert_echoed(case, &without_instants(input), &partition);
            for member in ["LastAccessTime", "LastAnalyzedTime"] {
                assert_eq!(
                    instant(&partition, member),
                    instant(input, member),
                    "case {case}: partition {values:?} must report the {member} the update \
                     sent, got {:?}",
                    partition.get(member)
                );
            }
            // ... and every settable member it did NOT send is gone, rather than left
            // over from the create: a `PartitionInput` is the whole definition.
            for member in SETTABLE_MEMBERS {
                if input.get(member).is_some() {
                    continue;
                }
                assert!(
                    partition.get(member).is_none_or(Value::is_null),
                    "case {case}: partition {values:?} must not report {member}, which the \
                     update omitted, got {:?}",
                    partition.get(member)
                );
            }

            // The address and the attribution are the create's, and so is `CreationTime`:
            // an update produces no new partition.
            let created_at = before
                .iter()
                .find(|earlier| earlier["Values"] == json!(values))
                .unwrap_or_else(|| panic!("case {case}: {values:?} was created by the fixture"));
            assert_eq!(
                partition["CreationTime"], created_at["CreationTime"],
                "case {case}: partition {values:?} must keep the CreationTime its create \
                 wrote ({}), got {}",
                created_at["CreationTime"], partition["CreationTime"]
            );
            assert_eq!(
                partition["Values"],
                json!(values),
                "case {case}: partition {values:?} must keep its address, got {}",
                partition["Values"]
            );
            assert_eq!(
                partition["DatabaseName"].as_str(),
                Some(database.as_str()),
                "case {case}: partition {values:?} must stay attributed to its database, got \
                 {:?}",
                partition["DatabaseName"]
            );
            assert_eq!(
                partition["TableName"].as_str(),
                Some(folded_table.as_str()),
                "case {case}: partition {values:?} must stay attributed to its table by the \
                 folded name, got {:?}",
                partition["TableName"]
            );
            assert_eq!(
                partition["CatalogId"].as_str(),
                Some(ACCOUNT_ID),
                "case {case}: partition {values:?} must stay attributed to the default \
                 catalog, got {:?}",
                partition["CatalogId"]
            );
        }

        // A partition the batch did not address is untouched by it.
        for earlier in &before {
            let addressed: Vec<&str> = earlier["Values"]
                .as_array()
                .expect("a stored partition reports its values")
                .iter()
                .map(|value| value.as_str().expect("a partition value is a string"))
                .collect();
            if row
                .updated
                .iter()
                .any(|input| values_of(input) == addressed)
            {
                continue;
            }
            let after = stored(&router, &database, &table, &addressed).await;
            assert_eq!(
                after, *earlier,
                "case {case}: the partition {addressed:?} was not in the batch, so the update \
                 must leave it as the create wrote it; expected {earlier}, got {after}"
            );
        }
    }
}

/// The rows of the round-trip table, as data.
fn round_trip_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "one key, bare values updated to a fully described partition",
            keys: vec!["dt"],
            created: vec![json!({ "Values": ["2026-01-01"] })],
            updated: vec![json!({
                "Values": ["2026-01-01"],
                "StorageDescriptor": {
                    "Columns": [
                        { "Name": "id", "Type": "string" },
                        { "Name": "amount", "Type": "double", "Comment": "cents" }
                    ],
                    "Location": "s3://bucket/events/dt=2026-01-01/",
                    "InputFormat": "org.apache.hadoop.mapred.TextInputFormat",
                    "OutputFormat":
                        "org.apache.hadoop.hive.ql.io.HiveIgnoreKeyTextOutputFormat",
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
                "Parameters": { "classification": "csv" },
                "LastAccessTime": 1_767_225_600
            })],
        },
        UpdateCase {
            case: "two keys, two described partitions updated back to bare values",
            keys: vec!["dt", "region"],
            created: vec![
                json!({
                    "Values": ["2026-01-01", "us-east-1"],
                    "StorageDescriptor": { "Location": "s3://bucket/old/" },
                    "Parameters": { "generation": "first" },
                    "LastAccessTime": 1_767_225_600,
                    "LastAnalyzedTime": 1_767_312_000
                }),
                json!({
                    "Values": ["2026-01-02", "eu-west-1"],
                    "Parameters": { "rows": "17" }
                }),
                // Created, never updated: the suite asserts the batch left it alone.
                json!({ "Values": ["2026-01-03", "eu-west-1"] }),
            ],
            updated: vec![
                json!({ "Values": ["2026-01-01", "us-east-1"] }),
                json!({ "Values": ["2026-01-02", "eu-west-1"] }),
            ],
        },
        UpdateCase {
            case: "three keys, digit values, both instants replaced",
            keys: vec!["year", "month", "day"],
            created: vec![json!({
                "Values": ["2026", "01", "02"],
                "LastAccessTime": 1_767_225_600,
                "LastAnalyzedTime": 1_767_312_000
            })],
            updated: vec![json!({
                "Values": ["2026", "01", "02"],
                "LastAccessTime": 1_767_398_400,
                "LastAnalyzedTime": 1_767_484_800
            })],
        },
        UpdateCase {
            case: "one key, three partitions spanning value forms",
            keys: vec!["k"],
            created: vec![
                json!({ "Values": ["a value with spaces"] }),
                json!({ "Values": ["100"] }),
                json!({ "Values": ["ünïcode=yes"] }),
            ],
            updated: vec![
                json!({
                    "Values": ["a value with spaces"],
                    "Parameters": { "shape": "spaces" }
                }),
                json!({ "Values": ["100"], "Parameters": { "shape": "digits" } }),
                json!({
                    "Values": ["ünïcode=yes"],
                    "Parameters": { "shape": "non-ascii" }
                }),
            ],
        },
    ]
}

// ---------------------------------------------------------------------------
// Per-entry failures
// ---------------------------------------------------------------------------

/// One row of the per-entry failure table.
struct FailureCase {
    case: &'static str,
    /// The partition keys the addressed table defines.
    keys: Vec<&'static str>,
    /// The partitions the fixture create stores, each carrying the generation that wrote
    /// it.
    created: Vec<Value>,
    /// The batch's `Entries`.
    entries: Vec<Value>,
    /// The `ErrorCode` and `ErrorMessage` of each `Errors` entry, in order.
    expected: Vec<(&'static str, String)>,
    /// The `PartitionValueList` of each `Errors` entry, in order.
    expected_values: Vec<Vec<&'static str>>,
    /// Value lists whose stored definition the batch must have left as the create wrote
    /// it.
    unchanged: Vec<Vec<&'static str>>,
    /// Value lists the batch must have written the second generation onto.
    applied: Vec<Vec<&'static str>>,
}

/// An entry that cannot be applied fails only its own entry: the batch answers 200, names
/// that entry in `Errors` with the code and wording for the reason it was refused, and
/// leaves every other entry of the same batch applied and every partition it could not
/// write untouched.
///
/// The rows differ along the way one entry fails — a value list the table holds no
/// partition for, one differing from a stored partition's only in case, a `PartitionInput`
/// naming other values than the entry addresses, one carrying no values at all, a value
/// count the table's partition keys cannot address — and along how many entries of the
/// batch succeed beside it.
#[tokio::test]
async fn an_entry_that_cannot_be_applied_fails_only_itself() {
    let router = emulator();

    for (index, row) in failure_cases().iter().enumerate() {
        let case = row.case;
        let database = format!("update_failures_db_{index}");
        let table = format!("update_failures_table_{index}");
        seed_table(&router, &database, &table, &row.keys).await;
        let created = batch_create(&router, &database, &table, &row.created).await;
        assert!(
            partition_errors(&created).is_empty(),
            "case {case}: the fixture partitions must be created, got Errors {:?}",
            created["Errors"]
        );

        let body = batch_update(&router, &database, &table, &row.entries).await;
        let reported = update_errors(&body);
        assert_eq!(
            reported.len(),
            row.expected.len(),
            "case {case}: expected {} per-entry errors, got {:?}",
            row.expected.len(),
            body["Errors"]
        );
        for (position, (failure, (expected_code, expected_message))) in
            reported.iter().zip(row.expected.iter()).enumerate()
        {
            let detail = &failure["ErrorDetail"];
            assert_eq!(
                detail["ErrorCode"].as_str(),
                Some(*expected_code),
                "case {case}: error {position} must carry ErrorCode {expected_code}, got \
                 {failure}"
            );
            assert_eq!(
                detail["ErrorMessage"].as_str(),
                Some(expected_message.as_str()),
                "case {case}: error {position} must carry ErrorMessage {expected_message:?}, \
                 got {failure}"
            );
            assert_eq!(
                detail["__type"].as_str(),
                Some(ERROR_DETAIL_TAG),
                "case {case}: error {position} must carry the ErrorDetail type tag, got \
                 {failure}"
            );
            let expected_values = row
                .expected_values
                .get(position)
                .expect("a row states the values of every error it expects");
            assert_eq!(
                failure["PartitionValueList"],
                json!(expected_values),
                "case {case}: error {position} must name the values its entry addressed, got \
                 {failure}"
            );
        }

        for values in &row.unchanged {
            let partition = stored(&router, &database, &table, values).await;
            assert_eq!(
                partition["Parameters"],
                generation("first"),
                "case {case}: the refused entry must leave {values:?} as the create wrote it, \
                 got {partition}"
            );
        }
        for values in &row.applied {
            let partition = stored(&router, &database, &table, values).await;
            assert_eq!(
                partition["Parameters"],
                generation("second"),
                "case {case}: the entry for {values:?} was applied, so a read must serve the \
                 definition it sent, got {partition}"
            );
        }
    }
}

/// The rows of the per-entry failure table, as data.
fn failure_cases() -> Vec<FailureCase> {
    vec![
        FailureCase {
            case: "a value list the table holds no partition for",
            keys: vec!["dt"],
            created: vec![json!({ "Values": ["2026-01-01"], "Parameters": generation("first") })],
            entries: vec![json!({
                "PartitionValueList": ["2026-01-09"],
                "PartitionInput": {
                    "Values": ["2026-01-09"],
                    "Parameters": generation("second")
                }
            })],
            expected: vec![("EntityNotFoundException", update_miss(&["2026-01-09"]))],
            expected_values: vec![vec!["2026-01-09"]],
            unchanged: vec![vec!["2026-01-01"]],
            applied: vec![],
        },
        FailureCase {
            case: "a value differing from a stored one only in case addresses no partition",
            keys: vec!["dt"],
            created: vec![json!({ "Values": ["Monday"], "Parameters": generation("first") })],
            entries: vec![json!({
                "PartitionValueList": ["monday"],
                "PartitionInput": { "Values": ["monday"], "Parameters": generation("second") }
            })],
            expected: vec![("EntityNotFoundException", update_miss(&["monday"]))],
            expected_values: vec![vec!["monday"]],
            unchanged: vec![vec!["Monday"]],
            applied: vec![],
        },
        FailureCase {
            case: "a PartitionInput naming other values than the entry addresses",
            keys: vec!["dt"],
            created: vec![
                json!({ "Values": ["2026-01-01"], "Parameters": generation("first") }),
                json!({ "Values": ["2026-01-02"], "Parameters": generation("first") }),
            ],
            entries: vec![json!({
                "PartitionValueList": ["2026-01-01"],
                "PartitionInput": {
                    "Values": ["2026-01-02"],
                    "Parameters": generation("second")
                }
            })],
            expected: vec![(
                "InvalidInputException",
                PARTITION_VALUES_CANNOT_BE_CHANGED.to_string(),
            )],
            expected_values: vec![vec!["2026-01-01"]],
            unchanged: vec![vec!["2026-01-01"], vec!["2026-01-02"]],
            applied: vec![],
        },
        FailureCase {
            case: "a PartitionInput carrying no values at all",
            keys: vec!["dt"],
            created: vec![json!({ "Values": ["2026-01-01"], "Parameters": generation("first") })],
            entries: vec![json!({
                "PartitionValueList": ["2026-01-01"],
                "PartitionInput": { "Parameters": generation("second") }
            })],
            expected: vec![(
                "InvalidInputException",
                PARTITION_VALUES_REQUIRED.to_string(),
            )],
            expected_values: vec![vec!["2026-01-01"]],
            unchanged: vec![vec!["2026-01-01"]],
            applied: vec![],
        },
        FailureCase {
            case: "more values than the table has partition keys",
            keys: vec!["dt"],
            created: vec![json!({ "Values": ["2026-01-01"], "Parameters": generation("first") })],
            entries: vec![json!({
                "PartitionValueList": ["2026-01-01", "us-east-1"],
                "PartitionInput": {
                    "Values": ["2026-01-01", "us-east-1"],
                    "Parameters": generation("second")
                }
            })],
            expected: vec![(
                "EntityNotFoundException",
                update_miss(&["2026-01-01", "us-east-1"]),
            )],
            expected_values: vec![vec!["2026-01-01", "us-east-1"]],
            unchanged: vec![vec!["2026-01-01"]],
            applied: vec![],
        },
        FailureCase {
            case: "two good entries beside two bad ones",
            keys: vec!["dt", "region"],
            created: vec![
                json!({
                    "Values": ["2026-01-01", "us-east-1"],
                    "Parameters": generation("first")
                }),
                json!({
                    "Values": ["2026-01-02", "us-east-1"],
                    "Parameters": generation("first")
                }),
                json!({
                    "Values": ["2026-01-03", "us-east-1"],
                    "Parameters": generation("first")
                }),
            ],
            entries: vec![
                json!({
                    "PartitionValueList": ["2026-01-01", "us-east-1"],
                    "PartitionInput": {
                        "Values": ["2026-01-01", "us-east-1"],
                        "Parameters": generation("second")
                    }
                }),
                json!({
                    "PartitionValueList": ["2026-01-09", "us-east-1"],
                    "PartitionInput": {
                        "Values": ["2026-01-09", "us-east-1"],
                        "Parameters": generation("second")
                    }
                }),
                json!({
                    "PartitionValueList": ["2026-01-02", "us-east-1"],
                    "PartitionInput": { "Parameters": generation("second") }
                }),
                json!({
                    "PartitionValueList": ["2026-01-03", "us-east-1"],
                    "PartitionInput": {
                        "Values": ["2026-01-03", "us-east-1"],
                        "Parameters": generation("second")
                    }
                }),
            ],
            expected: vec![
                (
                    "EntityNotFoundException",
                    update_miss(&["2026-01-09", "us-east-1"]),
                ),
                (
                    "InvalidInputException",
                    PARTITION_VALUES_REQUIRED.to_string(),
                ),
            ],
            expected_values: vec![
                vec!["2026-01-09", "us-east-1"],
                vec!["2026-01-02", "us-east-1"],
            ],
            unchanged: vec![vec!["2026-01-02", "us-east-1"]],
            applied: vec![
                vec!["2026-01-01", "us-east-1"],
                vec!["2026-01-03", "us-east-1"],
            ],
        },
    ]
}

/// Two entries of ONE batch addressing the same partition are applied in the order they
/// were sent, so the definition the last of them submitted is what a read serves.
#[tokio::test]
async fn the_last_entry_for_one_partition_is_the_definition_that_is_stored() {
    let router = emulator();
    seed_table(&router, "last_write_db", "last_write_table", &["dt"]).await;
    let created = batch_create(
        &router,
        "last_write_db",
        "last_write_table",
        &[json!({ "Values": ["2026-01-01"], "Parameters": generation("first") })],
    )
    .await;
    assert!(
        partition_errors(&created).is_empty(),
        "the fixture partition must be created, got Errors {:?}",
        created["Errors"]
    );

    let body = batch_update(
        &router,
        "last_write_db",
        "last_write_table",
        &[
            json!({
                "PartitionValueList": ["2026-01-01"],
                "PartitionInput": {
                    "Values": ["2026-01-01"],
                    "Parameters": generation("second")
                }
            }),
            json!({
                "PartitionValueList": ["2026-01-01"],
                "PartitionInput": {
                    "Values": ["2026-01-01"],
                    "Parameters": generation("third")
                }
            }),
        ],
    )
    .await;
    assert!(
        update_errors(&body).is_empty(),
        "both entries address a stored partition, so neither fails, got {body}"
    );

    let partition = stored(&router, "last_write_db", "last_write_table", &["2026-01-01"]).await;
    assert_eq!(
        partition["Parameters"],
        generation("third"),
        "the last entry for a partition is the definition stored, got {partition}"
    );
}

/// An update is a write and a read is not: reading a partition back twice serves the same
/// definition the update wrote, and applying the same definition again stores the same
/// partition.
#[tokio::test]
async fn an_update_is_idempotent_and_a_read_changes_nothing() {
    let router = emulator();
    seed_table(&router, "idempotent_db", "idempotent_table", &["dt"]).await;
    let created = batch_create(
        &router,
        "idempotent_db",
        "idempotent_table",
        &[json!({ "Values": ["2026-01-01"] })],
    )
    .await;
    assert!(
        partition_errors(&created).is_empty(),
        "the fixture partition must be created, got Errors {:?}",
        created["Errors"]
    );

    let definition = json!({
        "Values": ["2026-01-01"],
        "StorageDescriptor": { "Location": "s3://bucket/events/dt=2026-01-01/" },
        "Parameters": { "rows": "17" }
    });
    let mut previous: Option<Value> = None;
    for attempt in 1..=2 {
        let body = batch_update(
            &router,
            "idempotent_db",
            "idempotent_table",
            &[entry(&definition)],
        )
        .await;
        assert!(
            update_errors(&body).is_empty(),
            "attempt {attempt}: the entry addresses a stored partition, so it must be \
             applied, got {body}"
        );
        let first_read = stored(
            &router,
            "idempotent_db",
            "idempotent_table",
            &["2026-01-01"],
        )
        .await;
        let second_read = stored(
            &router,
            "idempotent_db",
            "idempotent_table",
            &["2026-01-01"],
        )
        .await;
        assert_eq!(
            first_read, second_read,
            "attempt {attempt}: a read must not change what a later read sees; expected \
             {first_read}, got {second_read}"
        );
        if let Some(earlier) = previous.as_ref() {
            assert_eq!(
                *earlier, first_read,
                "attempt {attempt}: applying the same definition again must store the same \
                 partition; expected {earlier}, got {first_read}"
            );
        }
        previous = Some(first_read);
    }
}

// ---------------------------------------------------------------------------
// Whole-request refusals
// ---------------------------------------------------------------------------

/// A database or table the catalog does not hold fails the WHOLE batch — there is no
/// table whose partitions any entry could address — with `EntityNotFoundException` and
/// the wording this service answers for that resource.
#[tokio::test]
async fn a_batch_naming_no_stored_table_is_refused_whole() {
    let router = emulator();
    seed_table(&router, "update_present_db", "update_present_table", &["dt"]).await;

    let cases = vec![
        (
            "no such database",
            "ghost_db",
            "update_present_table",
            "Database ghost_db not found.",
        ),
        (
            "no such database, mixed case",
            "Ghost_DB",
            "update_present_table",
            "Database ghost_db not found.",
        ),
        (
            "no such table",
            "update_present_db",
            "ghost_table",
            "Table ghost_table not found.",
        ),
        (
            "no such table, mixed case",
            "update_present_db",
            "Ghost_Table",
            "Table ghost_table not found.",
        ),
    ];

    for (case, database, table, expected_message) in cases {
        assert_refusal(
            &router,
            case,
            "BatchUpdatePartition",
            &json!({
                "DatabaseName": database,
                "TableName": table,
                "Entries": [ {
                    "PartitionValueList": ["2026-01-01"],
                    "PartitionInput": { "Values": ["2026-01-01"] }
                } ]
            }),
            "EntityNotFoundException",
            expected_message,
        )
        .await;
    }
}
