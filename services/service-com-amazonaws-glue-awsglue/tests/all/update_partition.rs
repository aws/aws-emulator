// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdatePartition` and the `GetPartition` read that serves what it
//! wrote.
//!
//! Every call goes over the wire the way a client makes it, through the shared
//! [`crate::wire`] harness: `POST /` with `X-Amz-Target: AWSGlue.<Operation>` and an
//! `awsJson1_1` body, dispatched by the same `MultiServiceRouter` the production front
//! door uses — so what is asserted here is HTTP behaviour and not a direct call into a
//! handler.
//!
//! The rule under test: a single-partition update writes the submitted `PartitionInput`
//! over the WHOLE client-settable definition of the partition the request addresses by its
//! `PartitionValueList` — so the next read reports exactly the submitted members and
//! nothing the request omitted, while the partition keeps the address and the server-set
//! `CreationTime` its create wrote. An update that cannot be applied changes nothing and
//! is refused with the modelled exception for the reason it was refused, the same reason
//! the batch update reports per entry.
//!
//! The suites are tables whose rows differ along what the rule generalises over: how many
//! partition keys the table defines, which members the create and the update each carry,
//! the form of a partition value, how many partitions the table holds beside the one
//! addressed, and the way one update can fail.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::messages::{
    PARTITION_NOT_FOUND, PARTITION_VALUES_CANNOT_BE_CHANGED, PARTITION_VALUES_REQUIRED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::assert_refusal;
use crate::partitions::{
    batch_create, generation, instant, partition_errors, seed_table, stored, values_of,
    without_instants, SETTABLE_MEMBERS,
};
use crate::wire::{assert_echoed, call, emulator, ok, ACCOUNT_ID};

/// One `UpdatePartition` request body addressing `values` with `input`.
fn update_body(database: &str, table: &str, values: &[&str], input: &Value) -> Value {
    json!({
        "DatabaseName": database,
        "TableName": table,
        "PartitionValueList": values,
        "PartitionInput": input
    })
}

/// Submit one `UpdatePartition`, returning its HTTP status and body.
async fn update_partition(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    values: &[&str],
    input: &Value,
) -> (u16, Value) {
    call(
        router,
        "UpdatePartition",
        &update_body(database, table, values, input),
    )
    .await
}

/// Submit one `UpdatePartition` that must answer 200, addressing the partition its own
/// `PartitionInput` defines — the only address an update may write to.
async fn apply(router: &MultiServiceRouter, database: &str, table: &str, input: &Value) -> Value {
    ok(
        router,
        "UpdatePartition",
        &update_body(database, table, &values_of(input), input),
    )
    .await
}

// ---------------------------------------------------------------------------
// Update -> read round trip
// ---------------------------------------------------------------------------

/// One row of the round-trip table: the definitions a create stores and the one an update
/// writes over the partition it addresses.
struct UpdateCase {
    case: &'static str,
    /// The partition keys the addressed table defines.
    keys: Vec<&'static str>,
    /// The `PartitionInput`s the fixture create stores. Every one the update does not
    /// address must be left exactly as the create wrote it.
    created: Vec<Value>,
    /// The `PartitionInput` the update submits, addressing its own `Values`.
    updated: Value,
}

/// Whatever a client submits to `UpdatePartition` is what `GetPartition` reads back: every
/// member the request carried, nothing it omitted, and the address, the attribution and
/// the `CreationTime` the create wrote.
///
/// The rows differ along the number of partition keys (1, 2, 3), the member sets the create
/// and the update carry (bare values updated to a fully described partition; a described
/// partition updated back to bare values; instants replaced by other instants), the form of
/// a partition value (spaces, digits, non-ASCII) and how many partitions the table holds
/// beside the addressed one.
#[tokio::test]
async fn an_update_is_read_back_from_the_store() {
    let router = emulator();

    for (index, row) in round_trip_cases().iter().enumerate() {
        let case = row.case;
        let database = format!("single_update_db_{index}");
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

        let answer = apply(&router, &database, &table, &row.updated).await;
        assert_eq!(
            answer,
            json!({}),
            "case {case}: UpdatePartitionResponse carries no member, so the answer reports \
             nothing of the stored definition, got {answer}"
        );

        let values = values_of(&row.updated);
        let partition = stored(&router, &database, &table, &values).await;

        // Every member the update sent comes back ...
        assert_echoed(case, &without_instants(&row.updated), &partition);
        for member in ["LastAccessTime", "LastAnalyzedTime"] {
            assert_eq!(
                instant(&partition, member),
                instant(&row.updated, member),
                "case {case}: partition {values:?} must report the {member} the update sent, \
                 got {:?}",
                partition.get(member)
            );
        }
        // ... and every settable member it did NOT send is gone, rather than left over
        // from the create: a `PartitionInput` is the whole definition.
        for member in SETTABLE_MEMBERS {
            if row.updated.get(member).is_some() {
                continue;
            }
            assert!(
                partition.get(member).is_none_or(Value::is_null),
                "case {case}: partition {values:?} must not report {member}, which the update \
                 omitted, got {:?}",
                partition.get(member)
            );
        }

        // The address and the attribution are the create's, and so is `CreationTime`: an
        // update produces no new partition.
        let created_at = before
            .iter()
            .find(|earlier| earlier["Values"] == json!(values))
            .unwrap_or_else(|| panic!("case {case}: {values:?} was created by the fixture"));
        assert_eq!(
            partition["CreationTime"], created_at["CreationTime"],
            "case {case}: partition {values:?} must keep the CreationTime its create wrote \
             ({}), got {}",
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
            "case {case}: partition {values:?} must stay attributed to its database, got {:?}",
            partition["DatabaseName"]
        );
        assert_eq!(
            partition["TableName"].as_str(),
            Some(folded_table.as_str()),
            "case {case}: partition {values:?} must stay attributed to its table by the folded \
             name, got {:?}",
            partition["TableName"]
        );
        assert_eq!(
            partition["CatalogId"].as_str(),
            Some(ACCOUNT_ID),
            "case {case}: partition {values:?} must stay attributed to the default catalog, got \
             {:?}",
            partition["CatalogId"]
        );

        // A partition the update did not address is untouched by it.
        for earlier in &before {
            let addressed: Vec<&str> = earlier["Values"]
                .as_array()
                .expect("a stored partition reports its values")
                .iter()
                .map(|value| value.as_str().expect("a partition value is a string"))
                .collect();
            if addressed == values {
                continue;
            }
            let after = stored(&router, &database, &table, &addressed).await;
            assert_eq!(
                after, *earlier,
                "case {case}: the update did not address {addressed:?}, so it must leave it as \
                 the create wrote it; expected {earlier}, got {after}"
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
            updated: json!({
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
            }),
        },
        UpdateCase {
            case: "two keys, a described partition updated back to bare values, two others \
                   stored beside it",
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
                json!({ "Values": ["2026-01-03", "eu-west-1"] }),
            ],
            updated: json!({ "Values": ["2026-01-01", "us-east-1"] }),
        },
        UpdateCase {
            case: "three keys, digit values, both instants replaced",
            keys: vec!["year", "month", "day"],
            created: vec![json!({
                "Values": ["2026", "01", "02"],
                "LastAccessTime": 1_767_225_600,
                "LastAnalyzedTime": 1_767_312_000
            })],
            updated: json!({
                "Values": ["2026", "01", "02"],
                "LastAccessTime": 1_767_398_400,
                "LastAnalyzedTime": 1_767_484_800
            }),
        },
        UpdateCase {
            case: "one key, a value carrying spaces",
            keys: vec!["k"],
            created: vec![
                json!({ "Values": ["a value with spaces"] }),
                json!({ "Values": ["100"] }),
            ],
            updated: json!({
                "Values": ["a value with spaces"],
                "Parameters": { "shape": "spaces" }
            }),
        },
        UpdateCase {
            case: "one key, a non-ASCII value",
            keys: vec!["k"],
            created: vec![
                json!({ "Values": ["ünïcode=yes"] }),
                json!({ "Values": ["plain"], "Parameters": { "shape": "untouched" } }),
            ],
            updated: json!({
                "Values": ["ünïcode=yes"],
                "Parameters": { "shape": "non-ascii" },
                "StorageDescriptor": { "Location": "s3://bucket/ünïcode/" }
            }),
        },
    ]
}

/// An explicit `CatalogId` naming this emulator's account addresses the same catalog the
/// default does: the partition the create stored under no explicit id is the one this
/// update writes.
#[tokio::test]
async fn an_explicit_catalog_id_addresses_the_default_catalog() {
    let router = emulator();
    seed_table(&router, "explicit_db", "explicit_table", &["dt"]).await;
    let created = batch_create(
        &router,
        "explicit_db",
        "explicit_table",
        &[json!({ "Values": ["2026-01-01"], "Parameters": generation("first") })],
    )
    .await;
    assert!(
        partition_errors(&created).is_empty(),
        "the fixture partition must be created, got Errors {:?}",
        created["Errors"]
    );

    let mut request = update_body(
        "explicit_db",
        "explicit_table",
        &["2026-01-01"],
        &json!({ "Values": ["2026-01-01"], "Parameters": generation("second") }),
    );
    request["CatalogId"] = json!(ACCOUNT_ID);
    let body = ok(&router, "UpdatePartition", &request).await;
    assert_eq!(
        body,
        json!({}),
        "an update through the explicit catalog id answers the same empty response, got {body}"
    );

    let partition = stored(&router, "explicit_db", "explicit_table", &["2026-01-01"]).await;
    assert_eq!(
        partition["Parameters"],
        generation("second"),
        "the explicit catalog id must address the catalog the create wrote to, got {partition}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// The sentence an update of a value list the table holds no partition for is refused with,
/// written here independently of the service so the suite pins the wording and not the code:
/// fixed text around the values the request addressed, which is NOT the sentence a partition
/// READ answers for the same absent partition.
fn update_miss(values: &[&str]) -> String {
    format!("Partition [{}] not found", values.join(", "))
}

/// One row of the refusal table.
struct RefusalCase {
    case: &'static str,
    /// The partition keys the addressed table defines.
    keys: Vec<&'static str>,
    /// The partitions the fixture create stores, each carrying the generation that wrote
    /// it.
    created: Vec<Value>,
    /// The `PartitionValueList` the refused update addresses.
    values: Vec<&'static str>,
    /// The `PartitionInput` the refused update submits.
    input: Value,
    /// The error code and `Message` the refusal must carry.
    expected: (&'static str, String),
    /// Value lists whose stored definition the refused update must have left alone.
    unchanged: Vec<Vec<&'static str>>,
}

/// An update that cannot be applied is refused with the modelled exception the reason it
/// was refused names, carries that reason's wording, and stores nothing — the partitions
/// the table already held stay exactly as their create wrote them.
///
/// The rows differ along the way the update fails: a value list the table holds no
/// partition for, one differing from a stored partition's only in case, a `PartitionInput`
/// naming other values than the request addresses, one carrying no values at all, and a
/// value count the table's partition keys cannot address.
#[tokio::test]
async fn an_update_the_catalog_cannot_apply_is_refused_and_stores_nothing() {
    let router = emulator();

    for (index, row) in refusal_cases().iter().enumerate() {
        let case = row.case;
        let database = format!("single_update_refusal_db_{index}");
        let table = format!("single_update_refusal_table_{index}");
        seed_table(&router, &database, &table, &row.keys).await;
        let created = batch_create(&router, &database, &table, &row.created).await;
        assert!(
            partition_errors(&created).is_empty(),
            "case {case}: the fixture partitions must be created, got Errors {:?}",
            created["Errors"]
        );

        let (expected_code, expected_message) = &row.expected;
        assert_refusal(
            &router,
            case,
            "UpdatePartition",
            &update_body(&database, &table, &row.values, &row.input),
            expected_code,
            expected_message,
        )
        .await;

        for values in &row.unchanged {
            let partition = stored(&router, &database, &table, values).await;
            assert_eq!(
                partition["Parameters"],
                generation("first"),
                "case {case}: the refused update must leave {values:?} as the create wrote it, \
                 got {partition}"
            );
        }
    }
}

/// The rows of the refusal table, as data.
fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            case: "a value list the table holds no partition for",
            keys: vec!["dt"],
            created: vec![json!({ "Values": ["2026-01-01"], "Parameters": generation("first") })],
            values: vec!["2026-01-09"],
            input: json!({ "Values": ["2026-01-09"], "Parameters": generation("second") }),
            expected: ("EntityNotFoundException", update_miss(&["2026-01-09"])),
            unchanged: vec![vec!["2026-01-01"]],
        },
        RefusalCase {
            case: "a value differing from a stored one only in case addresses no partition",
            keys: vec!["dt"],
            created: vec![json!({ "Values": ["Monday"], "Parameters": generation("first") })],
            values: vec!["monday"],
            input: json!({ "Values": ["monday"], "Parameters": generation("second") }),
            expected: ("EntityNotFoundException", update_miss(&["monday"])),
            unchanged: vec![vec!["Monday"]],
        },
        RefusalCase {
            case: "a PartitionInput naming other values than the request addresses",
            keys: vec!["dt"],
            created: vec![
                json!({ "Values": ["2026-01-01"], "Parameters": generation("first") }),
                json!({ "Values": ["2026-01-02"], "Parameters": generation("first") }),
            ],
            values: vec!["2026-01-01"],
            input: json!({ "Values": ["2026-01-02"], "Parameters": generation("second") }),
            expected: (
                "InvalidInputException",
                PARTITION_VALUES_CANNOT_BE_CHANGED.to_string(),
            ),
            unchanged: vec![vec!["2026-01-01"], vec!["2026-01-02"]],
        },
        RefusalCase {
            case: "a PartitionInput carrying no values at all",
            keys: vec!["dt"],
            created: vec![json!({ "Values": ["2026-01-01"], "Parameters": generation("first") })],
            values: vec!["2026-01-01"],
            input: json!({ "Parameters": generation("second") }),
            expected: ("InvalidInputException", PARTITION_VALUES_REQUIRED.to_string()),
            unchanged: vec![vec!["2026-01-01"]],
        },
        RefusalCase {
            case: "more values than the table has partition keys",
            keys: vec!["dt"],
            created: vec![json!({ "Values": ["2026-01-01"], "Parameters": generation("first") })],
            values: vec!["2026-01-01", "us-east-1"],
            input: json!({
                "Values": ["2026-01-01", "us-east-1"],
                "Parameters": generation("second")
            }),
            expected: (
                "EntityNotFoundException",
                update_miss(&["2026-01-01", "us-east-1"]),
            ),
            unchanged: vec![vec!["2026-01-01"]],
        },
        RefusalCase {
            case: "fewer values than the table has partition keys",
            keys: vec!["dt", "region"],
            created: vec![json!({
                "Values": ["2026-01-01", "us-east-1"],
                "Parameters": generation("first")
            })],
            values: vec!["2026-01-01"],
            input: json!({ "Values": ["2026-01-01"], "Parameters": generation("second") }),
            expected: ("EntityNotFoundException", update_miss(&["2026-01-01"])),
            unchanged: vec![vec!["2026-01-01", "us-east-1"]],
        },
    ]
}

/// A database or table the catalog does not hold is refused with `EntityNotFoundException`
/// and the wording this service answers for that resource: there is no table whose
/// partitions the request could address.
#[tokio::test]
async fn an_update_naming_no_stored_table_is_refused() {
    let router = emulator();
    seed_table(&router, "update_present_db", "update_present_table", &["dt"]).await;

    let cases = [
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
            "UpdatePartition",
            &update_body(
                database,
                table,
                &["2026-01-01"],
                &json!({ "Values": ["2026-01-01"] }),
            ),
            "EntityNotFoundException",
            expected_message,
        )
        .await;
    }
}

/// The refusal of a MUTATION of an absent partition names the partition it could not find,
/// while the READ of that same absent partition names none: the two sentences are each
/// operation's own and stay distinct, whatever values address the partition — and the entry
/// of a BATCH update is refused in the mutation's wording too, because it is the same
/// mutation.
///
/// The rows differ along how many values address the partition and what a value holds: one
/// value, two, three, a value carrying a space, and a value carrying the separator the
/// sentence lists values with.
#[tokio::test]
async fn the_update_miss_names_the_partition_while_the_read_miss_does_not() {
    let router = emulator();

    // (case, the partition keys the table declares, the values nothing was created under)
    let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("one value", vec!["dt"], vec!["2023"]),
        ("two values", vec!["dt", "region"], vec!["2023", "us-east-1"]),
        (
            "three values",
            vec!["dt", "region", "tier"],
            vec!["2023", "us-east-1", "gold"],
        ),
        ("a value carrying a space", vec!["dt"], vec!["last quarter"]),
        (
            "a value carrying the separator the sentence lists values with",
            vec!["dt", "region"],
            vec!["a, b", "us-east-1"],
        ),
    ];

    for (index, (case, keys, values)) in rows.iter().enumerate() {
        let database = format!("distinct_db_{index}");
        let table = format!("distinct_table_{index}");
        seed_table(&router, &database, &table, keys).await;

        // The mutation names the partition it could not find.
        assert_refusal(
            &router,
            case,
            "UpdatePartition",
            &update_body(
                &database,
                &table,
                values,
                &json!({ "Values": values, "Parameters": generation("second") }),
            ),
            "EntityNotFoundException",
            &update_miss(values),
        )
        .await;

        // The read of the same absent partition does not.
        assert_refusal(
            &router,
            case,
            "GetPartition",
            &json!({
                "DatabaseName": &database,
                "TableName": &table,
                "PartitionValues": values,
            }),
            "EntityNotFoundException",
            PARTITION_NOT_FOUND,
        )
        .await;
        assert_ne!(
            update_miss(values),
            PARTITION_NOT_FOUND,
            "case {case}: the mutation's sentence must not be the read's"
        );

        // One entry of a batch update is the same mutation, refused in the same wording.
        let batch = ok(
            &router,
            "BatchUpdatePartition",
            &json!({
                "DatabaseName": &database,
                "TableName": &table,
                "Entries": [{
                    "PartitionValueList": values,
                    "PartitionInput": { "Values": values },
                }],
            }),
        )
        .await;
        assert_eq!(
            batch["Errors"][0]["ErrorDetail"]["ErrorMessage"].as_str(),
            Some(update_miss(values).as_str()),
            "case {case}: an entry of a batch update is refused in the mutation's wording, got \
             {batch}"
        );
    }
}

/// An update is a write and a read is not: reading a partition back twice serves the same
/// definition the update wrote, and applying the same definition again stores the same
/// partition.
#[tokio::test]
async fn an_update_is_idempotent_and_a_read_changes_nothing() {
    let router = emulator();
    seed_table(
        &router,
        "single_idempotent_db",
        "single_idempotent_table",
        &["dt"],
    )
    .await;
    let created = batch_create(
        &router,
        "single_idempotent_db",
        "single_idempotent_table",
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
        apply(
            &router,
            "single_idempotent_db",
            "single_idempotent_table",
            &definition,
        )
        .await;
        let first_read = stored(
            &router,
            "single_idempotent_db",
            "single_idempotent_table",
            &["2026-01-01"],
        )
        .await;
        let second_read = stored(
            &router,
            "single_idempotent_db",
            "single_idempotent_table",
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

/// A partition the single update wrote is the one every other partition reader serves: the
/// definition is stored once, in the record every read consults, rather than in a copy only
/// `GetPartition` sees.
#[tokio::test]
async fn what_an_update_wrote_is_served_by_every_partition_reader() {
    let router = emulator();
    seed_table(&router, "readers_db", "readers_table", &["dt"]).await;
    let created = batch_create(
        &router,
        "readers_db",
        "readers_table",
        &[json!({ "Values": ["2026-01-01"], "Parameters": generation("first") })],
    )
    .await;
    assert!(
        partition_errors(&created).is_empty(),
        "the fixture partition must be created, got Errors {:?}",
        created["Errors"]
    );

    apply(
        &router,
        "readers_db",
        "readers_table",
        &json!({ "Values": ["2026-01-01"], "Parameters": generation("second") }),
    )
    .await;

    let batch = ok(
        &router,
        "BatchGetPartition",
        &json!({
            "DatabaseName": "readers_db",
            "TableName": "readers_table",
            "PartitionsToGet": [{ "Values": ["2026-01-01"] }]
        }),
    )
    .await;
    let partitions = batch["Partitions"]
        .as_array()
        .unwrap_or_else(|| panic!("BatchGetPartition must report a Partitions list, got {batch}"));
    assert_eq!(
        partitions.len(),
        1,
        "the batch read must serve the one stored partition, got {batch}"
    );
    assert_eq!(
        partitions[0]["Parameters"],
        generation("second"),
        "the batch reader must serve what the single update wrote, got {batch}"
    );

    let (status, read) = update_partition(
        &router,
        "readers_db",
        "readers_table",
        &["2026-01-01"],
        &json!({ "Values": ["2026-01-01"], "Parameters": generation("third") }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a second update of the same partition must be applied, got {status} with {read}"
    );
    let partition = stored(&router, "readers_db", "readers_table", &["2026-01-01"]).await;
    assert_eq!(
        partition["Parameters"],
        generation("third"),
        "the last update of a partition is the definition stored, got {partition}"
    );
}
