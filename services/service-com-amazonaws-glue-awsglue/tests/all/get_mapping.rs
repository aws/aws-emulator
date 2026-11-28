// SPDX-License-Identifier: CC0-1.0

//! `GetMapping`: the field-by-field proposal from one Data Catalog table to the target
//! tables a request names.
//!
//! RULE: the proposal is computed from the STORED table definitions. For every column of
//! the source table's data — its storage-descriptor columns, then its partition keys — it
//! reports one entry per target table, naming that column as the source field (the
//! catalog's table name, the column's name, the type it declares) and the target table's
//! LIKE-NAMED column as the target field, so the target's own spelling and declared type
//! are what the entry carries; a target that declares no such column takes the source
//! field through unchanged, and a request naming no target table at all proposes the
//! source onto itself and names no target table. Nothing is written: two identical calls,
//! and a read of either table after them, answer what the catalog held before.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many columns the source
//! declares (none, one, several), whether they are partition keys, whether the target
//! declares the same column, the same column in another type, the same column in another
//! case or no such column, how many target tables the request names (none, one, two —
//! a request naming none states its destination as a `Location`, since a request stating
//! neither is refused, which [`crate::mapping_destination`] pins), whether a column declares
//! a type at all, and whether the request also carries a `Location`. A last table walks the
//! two size bounds this operation enforces for itself — the model bounds neither the `Sinks`
//! list nor a table's column count — at each ceiling and one past it.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::get_mapping::{
    MAX_MAPPING_ENTRIES, MAX_MAPPING_SINKS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::assert_refusal;
use crate::wire::{emulator, ok};

/// One column of a table under test: its `Name` and the `Type` it declares. A column need
/// not declare one — `Column$Type` is not required (`smithy_query shape=Column`) — which
/// `None` stands for.
type Col = (&'static str, Option<&'static str>);

/// A table one row stores: the name it is created under, its storage-descriptor columns
/// and its partition keys.
struct Stored {
    name: &'static str,
    columns: &'static [Col],
    partition_keys: &'static [Col],
}

/// A table with no partition keys, which is what most rows store.
fn flat(name: &'static str, columns: &'static [Col]) -> Stored {
    Stored {
        name,
        columns,
        partition_keys: &[],
    }
}

/// One expected `MappingEntry`: `SourceTable`, `SourcePath`, `SourceType`, `TargetTable`,
/// `TargetPath`, `TargetType`, with `None` for a member the entry must NOT carry.
type Entry = (
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    &'static str,
    Option<&'static str>,
);

/// One row of the proposal table: the case, the source table, the target tables the
/// request names, the `Location` it carries, and the `Mapping` it must answer.
struct Row {
    case: &'static str,
    source: Stored,
    sinks: Vec<Stored>,
    location: Option<Value>,
    expected: &'static [Entry],
}

/// The rows: one per shape of source, target and request the rule has to cover.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a target declaring the same columns maps each one onto itself",
            source: flat("orders", &[("id", Some("int")), ("name", Some("string"))]),
            sinks: vec![flat(
                "orders_copy",
                &[("id", Some("int")), ("name", Some("string"))],
            )],
            location: None,
            expected: &[
                (
                    "orders",
                    "id",
                    Some("int"),
                    Some("orders_copy"),
                    "id",
                    Some("int"),
                ),
                (
                    "orders",
                    "name",
                    Some("string"),
                    Some("orders_copy"),
                    "name",
                    Some("string"),
                ),
            ],
        },
        Row {
            case: "a target declaring another type maps the column onto that type",
            source: flat("orders", &[("id", Some("int")), ("name", Some("string"))]),
            sinks: vec![flat(
                "orders_warehouse",
                &[("id", Some("bigint")), ("name", Some("varchar(64)"))],
            )],
            location: None,
            expected: &[
                (
                    "orders",
                    "id",
                    Some("int"),
                    Some("orders_warehouse"),
                    "id",
                    Some("bigint"),
                ),
                (
                    "orders",
                    "name",
                    Some("string"),
                    Some("orders_warehouse"),
                    "name",
                    Some("varchar(64)"),
                ),
            ],
        },
        Row {
            case: "a target declaring no such column takes the field through unchanged",
            source: flat("orders", &[("id", Some("int")), ("name", Some("string"))]),
            sinks: vec![flat("orders_ids", &[("id", Some("int"))])],
            location: None,
            expected: &[
                (
                    "orders",
                    "id",
                    Some("int"),
                    Some("orders_ids"),
                    "id",
                    Some("int"),
                ),
                (
                    "orders",
                    "name",
                    Some("string"),
                    Some("orders_ids"),
                    "name",
                    Some("string"),
                ),
            ],
        },
        Row {
            case: "a target spelling the column in another case carries its own spelling",
            source: flat("orders", &[("id", Some("int"))]),
            sinks: vec![flat("orders_upper", &[("ID", Some("bigint"))])],
            location: None,
            expected: &[(
                "orders",
                "id",
                Some("int"),
                Some("orders_upper"),
                "ID",
                Some("bigint"),
            )],
        },
        Row {
            case: "a column declaring no type maps on without one",
            source: flat("payloads", &[("payload", None)]),
            sinks: vec![flat("payloads_copy", &[("payload", None)])],
            location: None,
            expected: &[("payloads", "payload", None, Some("payloads_copy"), "payload", None)],
        },
        Row {
            case: "the partition keys follow the storage-descriptor columns",
            source: Stored {
                name: "events",
                columns: &[("id", Some("int"))],
                partition_keys: &[("dt", Some("string")), ("hour", Some("int"))],
            },
            sinks: vec![flat(
                "events_copy",
                &[
                    ("id", Some("int")),
                    ("dt", Some("date")),
                    ("hour", Some("smallint")),
                ],
            )],
            location: None,
            expected: &[
                (
                    "events",
                    "id",
                    Some("int"),
                    Some("events_copy"),
                    "id",
                    Some("int"),
                ),
                (
                    "events",
                    "dt",
                    Some("string"),
                    Some("events_copy"),
                    "dt",
                    Some("date"),
                ),
                (
                    "events",
                    "hour",
                    Some("int"),
                    Some("events_copy"),
                    "hour",
                    Some("smallint"),
                ),
            ],
        },
        Row {
            case: "two targets are proposed one after the other, in the order sent",
            source: flat("orders", &[("id", Some("int"))]),
            sinks: vec![
                flat("orders_first", &[("id", Some("bigint"))]),
                flat("orders_second", &[("order_id", Some("int"))]),
            ],
            location: None,
            expected: &[
                (
                    "orders",
                    "id",
                    Some("int"),
                    Some("orders_first"),
                    "id",
                    Some("bigint"),
                ),
                (
                    "orders",
                    "id",
                    Some("int"),
                    Some("orders_second"),
                    "id",
                    Some("int"),
                ),
            ],
        },
        Row {
            case: "no target table, only a location: the source is proposed onto itself",
            source: flat("orders", &[("id", Some("int")), ("name", Some("string"))]),
            sinks: vec![],
            // The destination this request states is the location, which names no columns,
            // so the proposal is the source's own fields onto themselves. A request stating
            // NO destination at all is refused, which `mapping_destination` pins.
            location: Some(json!({
                "S3": [{ "Name": "path", "Value": "s3://bucket/orders/" }]
            })),
            expected: &[
                ("orders", "id", Some("int"), None, "id", Some("int")),
                ("orders", "name", Some("string"), None, "name", Some("string")),
            ],
        },
        Row {
            case: "a source declaring no columns proposes nothing",
            source: flat("empty_tbl", &[]),
            sinks: vec![flat("empty_copy", &[("id", Some("int"))])],
            location: None,
            expected: &[],
        },
        Row {
            case: "a Location beside the target does not change the proposal",
            source: flat("orders", &[("id", Some("int"))]),
            sinks: vec![flat("orders_copy", &[("id", Some("int"))])],
            location: Some(json!({
                "S3": [{ "Name": "path", "Value": "s3://bucket/orders/" }]
            })),
            expected: &[(
                "orders",
                "id",
                Some("int"),
                Some("orders_copy"),
                "id",
                Some("int"),
            )],
        },
    ]
}

/// The `Columns` list a table's columns are created with.
fn columns_json(columns: &[Col]) -> Vec<Value> {
    columns
        .iter()
        .map(|(name, data_type)| {
            let mut column = json!({ "Name": name });
            if let Some(data_type) = data_type {
                column["Type"] = json!(data_type);
            }
            column
        })
        .collect()
}

/// Store one table of a row in `database`.
async fn given_table(router: &MultiServiceRouter, database: &str, table: &Stored) {
    let mut table_input = json!({
        "Name": table.name,
        "StorageDescriptor": {
            "Location": format!("s3://bucket/{}/", table.name),
            "Columns": columns_json(table.columns),
        },
    });
    if !table.partition_keys.is_empty() {
        table_input["PartitionKeys"] = json!(columns_json(table.partition_keys));
    }
    ok(
        router,
        "CreateTable",
        &json!({ "DatabaseName": database, "TableInput": table_input }),
    )
    .await;
}

/// The `Mapping` a row expects, as the wire renders it: a member the entry does not carry
/// is absent from the object rather than null.
fn expected_mapping(expected: &[Entry]) -> Value {
    Value::Array(
        expected
            .iter()
            .map(|entry| {
                let mut mapped = json!({
                    "SourceTable": entry.0,
                    "SourcePath": entry.1,
                    "TargetPath": entry.4,
                });
                if let Some(source_type) = entry.2 {
                    mapped["SourceType"] = json!(source_type);
                }
                if let Some(target_table) = entry.3 {
                    mapped["TargetTable"] = json!(target_table);
                }
                if let Some(target_type) = entry.5 {
                    mapped["TargetType"] = json!(target_type);
                }
                mapped
            })
            .collect(),
    )
}

/// One `GetMapping` body for a row.
fn get_mapping_request(database: &str, row: &Row) -> Value {
    let mut request = json!({
        "Source": { "DatabaseName": database, "TableName": row.source.name },
    });
    if !row.sinks.is_empty() {
        request["Sinks"] = Value::Array(
            row.sinks
                .iter()
                .map(|sink| json!({ "DatabaseName": database, "TableName": sink.name }))
                .collect(),
        );
    }
    if let Some(location) = &row.location {
        request["Location"] = location.clone();
    }
    request
}

/// Store a row's database, its source table and its target tables.
async fn given_row(router: &MultiServiceRouter, database: &str, row: &Row) {
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database } }),
    )
    .await;
    given_table(router, database, &row.source).await;
    for sink in &row.sinks {
        given_table(router, database, sink).await;
    }
}

/// Every row's proposal is the one the stored definitions imply.
#[tokio::test]
async fn the_proposal_maps_every_source_field_onto_each_target() {
    for (index, row) in rows().iter().enumerate() {
        // One store per row, so a row's tables cannot be seen by another's.
        let router = emulator();
        let database = format!("mapping_db_{index}");
        given_row(&router, &database, row).await;

        let answered = ok(&router, "GetMapping", &get_mapping_request(&database, row)).await;
        assert_eq!(
            answered["Mapping"],
            expected_mapping(row.expected),
            "case {case:?}: expected {expected}, got {answered}",
            case = row.case,
            expected = expected_mapping(row.expected),
        );
    }
}

/// The proposal is a REPORT: asking for it twice answers the same thing, and the tables it
/// read are unchanged afterwards.
#[tokio::test]
async fn asking_for_the_proposal_changes_nothing() {
    for (index, row) in rows().iter().enumerate() {
        let router = emulator();
        let database = format!("mapping_db_{index}");
        given_row(&router, &database, row).await;
        let request = get_mapping_request(&database, row);

        let before = ok(
            &router,
            "GetTable",
            &json!({ "DatabaseName": &database, "Name": row.source.name }),
        )
        .await;
        let first = ok(&router, "GetMapping", &request).await;
        let again = ok(&router, "GetMapping", &request).await;
        let after = ok(
            &router,
            "GetTable",
            &json!({ "DatabaseName": &database, "Name": row.source.name }),
        )
        .await;

        assert_eq!(
            first, again,
            "case {case:?}: two identical calls must answer the same proposal; got {first} then \
             {again}",
            case = row.case,
        );
        assert_eq!(
            before, after,
            "case {case:?}: the source table must be unchanged by the proposal; it read {before} \
             before and {after} after",
            case = row.case,
        );
    }
}

/// One row of the miss table: the case, the `Source` and `Sinks` sent, and the `Message`
/// the refusal has to carry.
struct MissRow {
    case: &'static str,
    source: &'static str,
    sinks: Vec<&'static str>,
    database: &'static str,
    message: String,
}

/// `GetMapping` for a table or database the catalog does not hold is refused with HTTP
/// 400 and `EntityNotFoundException`, whether the absent entity is the source's or a
/// target's.
#[tokio::test]
async fn an_absent_source_or_target_is_a_miss() {
    const DATABASE: &str = "mapping_misses";
    const PRESENT: &str = "present_tbl";

    let misses = vec![
        MissRow {
            case: "the source names no stored database",
            source: PRESENT,
            sinks: vec![],
            database: "no_such_db",
            // The refusal for an absent database, as this service words it.
            message: "Database no_such_db not found.".to_string(),
        },
        MissRow {
            case: "the source names no stored table",
            source: "no_such_tbl",
            sinks: vec![],
            database: DATABASE,
            // The refusal for an absent table read through its database.
            message: "Table no_such_tbl not found.".to_string(),
        },
        MissRow {
            case: "a target names no stored table",
            source: PRESENT,
            sinks: vec!["no_such_sink"],
            database: DATABASE,
            message: "Table no_such_sink not found.".to_string(),
        },
        MissRow {
            case: "the second of two targets names no stored table",
            source: PRESENT,
            sinks: vec![PRESENT, "no_such_sink"],
            database: DATABASE,
            message: "Table no_such_sink not found.".to_string(),
        },
    ];

    let router = emulator();
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;
    given_table(
        &router,
        DATABASE,
        &flat(PRESENT, &[("id", Some("int")), ("name", Some("string"))]),
    )
    .await;

    for miss in &misses {
        let mut request = json!({
            "Source": { "DatabaseName": miss.database, "TableName": miss.source },
            // Every miss request states a destination, so what it is refused for is the
            // entity it names and not the either/or rule `mapping_destination` pins.
            "Location": { "S3": [{ "Name": "path", "Value": "s3://bucket/out/" }] },
        });
        if !miss.sinks.is_empty() {
            request["Sinks"] = Value::Array(
                miss.sinks
                    .iter()
                    .map(|sink| json!({ "DatabaseName": miss.database, "TableName": sink }))
                    .collect(),
            );
        }
        // The status, code and wording assertions are this crate's shared ones
        // (`error_messages::assert_refusal`).
        assert_refusal(
            &router,
            miss.case,
            "GetMapping",
            &request,
            "EntityNotFoundException",
            &miss.message,
        )
        .await;
    }

    // A refused proposal is still a read: the table that IS stored is served afterwards,
    // so the misses above cannot be a handler that refuses everything.
    let served = ok(
        &router,
        "GetMapping",
        &json!({
            "Source": { "DatabaseName": DATABASE, "TableName": PRESENT },
            "Sinks": [{ "DatabaseName": DATABASE, "TableName": PRESENT }],
        }),
    )
    .await;
    assert_eq!(
        served["Mapping"],
        expected_mapping(&[
            (PRESENT, "id", Some("int"), Some(PRESENT), "id", Some("int")),
            (
                PRESENT,
                "name",
                Some("string"),
                Some(PRESENT),
                "name",
                Some("string")
            ),
        ]),
        "the stored table must still be proposed, got {served}"
    );
}

/// Store a table of `columns` columns, named and typed uniformly, so a row can reach the
/// answer-size bound without spelling every column out.
async fn given_wide_table(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    columns: usize,
) {
    let declared: Vec<Value> = (0..columns)
        .map(|at| json!({ "Name": format!("col_{at}"), "Type": "string" }))
        .collect();
    ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": database,
            "TableInput": {
                "Name": table,
                "StorageDescriptor": {
                    "Location": format!("s3://bucket/{table}/"),
                    "Columns": declared,
                },
            },
        }),
    )
    .await;
}

/// One `GetMapping` body naming `sinks` copies of `table` as the targets of `source`.
fn request_with_sinks(database: &str, source: &str, table: &str, sinks: usize) -> Value {
    json!({
        "Source": { "DatabaseName": database, "TableName": source },
        "Sinks": (0..sinks)
            .map(|_| json!({ "DatabaseName": database, "TableName": table }))
            .collect::<Vec<Value>>(),
    })
}

/// The model bounds neither the `Sinks` list nor a table's column count, so this operation
/// bounds the request and the answer itself: a request AT either ceiling is served in full,
/// and one past it is refused with the modelled `InvalidInputException` naming the ceiling
/// — rather than served as an answer whose size is the request's size times a stored
/// table's column count.
#[tokio::test]
async fn the_size_bounds_admit_the_ceiling_and_refuse_past_it() {
    const DATABASE: &str = "mapping_bounds";
    const NARROW: &str = "narrow_tbl";
    const WIDE: &str = "wide_tbl";
    // A source wide enough that a proposal onto half the admitted targets sits exactly on
    // the entry ceiling, so both the ceiling and one past it are reachable cheaply.
    let targets_at_the_entry_ceiling = MAX_MAPPING_SINKS / 2;
    let wide_columns = MAX_MAPPING_ENTRIES / targets_at_the_entry_ceiling;

    let router = emulator();
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;
    given_wide_table(&router, DATABASE, NARROW, 1).await;
    given_wide_table(&router, DATABASE, WIDE, wide_columns).await;

    // AT the target ceiling: one entry per target, every one of them served.
    let served = ok(
        &router,
        "GetMapping",
        &request_with_sinks(DATABASE, NARROW, NARROW, MAX_MAPPING_SINKS),
    )
    .await;
    assert_eq!(
        served["Mapping"].as_array().map(Vec::len),
        Some(MAX_MAPPING_SINKS),
        "a request naming the admitted maximum of targets must be served in full, got {served}"
    );

    // One target past it: refused as a request-size failure, before any lookup.
    assert_refusal(
        &router,
        "one target past the ceiling",
        "GetMapping",
        &request_with_sinks(DATABASE, NARROW, NARROW, MAX_MAPPING_SINKS + 1),
        "InvalidInputException",
        &format!("Sinks may name at most {MAX_MAPPING_SINKS} tables"),
    )
    .await;

    // AT the entry ceiling: the product of the source's fields and the targets named.
    let served = ok(
        &router,
        "GetMapping",
        &request_with_sinks(DATABASE, WIDE, NARROW, targets_at_the_entry_ceiling),
    )
    .await;
    assert_eq!(
        served["Mapping"].as_array().map(Vec::len),
        Some(MAX_MAPPING_ENTRIES),
        "a proposal of exactly the admitted maximum of entries must be served in full, got {} \
         entries",
        served["Mapping"].as_array().map_or(0, Vec::len)
    );

    // One target past it: the same request-size refusal, on the product this time, naming
    // both factors so a caller can see which to cut.
    let past = targets_at_the_entry_ceiling + 1;
    assert_refusal(
        &router,
        "one target past the entry ceiling",
        "GetMapping",
        &request_with_sinks(DATABASE, WIDE, NARROW, past),
        "InvalidInputException",
        &format!(
            "a mapping of {wide_columns} source fields onto {past} target tables carries more \
             than the {MAX_MAPPING_ENTRIES} entries one mapping may carry"
        ),
    )
    .await;

    // The bounds refuse a request, never the store: the narrow proposal is still served.
    let served = ok(
        &router,
        "GetMapping",
        &request_with_sinks(DATABASE, NARROW, NARROW, 1),
    )
    .await;
    assert_eq!(
        served["Mapping"].as_array().map(Vec::len),
        Some(1),
        "a request inside both bounds must still be served, got {served}"
    );
}
