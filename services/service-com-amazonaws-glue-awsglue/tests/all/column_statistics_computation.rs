// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a Glue column statistics task run COMPUTES: the statistics
//! of the data behind the covered table, read out of the emulator's own Amazon S3 and
//! reported back through `GetColumnStatisticsForTable`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, and the
//! objects the runs read are staged through Amazon S3's own `PutObject`, so what is
//! asserted is HTTP behaviour over state the data plane really holds.
//!
//! The rule under test: a column statistics task run reads the objects under the covered
//! table's `StorageDescriptor$Location`, laid out the way its `StorageDescriptor$SerdeInfo`
//! says, and aggregates each covered column into the `ColumnStatisticsData` member its
//! declared `Column$Type` maps to — so the figures reported for a column are the
//! statistics of the cells that column holds (its nulls, its distinct values, its extremes
//! and its lengths). Such a run settles `SUCCEEDED` carrying NO `ErrorMessage`, and the
//! statistics it generated become readable through `GetColumnStatisticsForTable` with an
//! empty `Errors`. The modelled failure status is reserved for a run that could not read
//! the data, and such a run generates no statistics at all.
//!
//! The recorded case — a 20-row comma-separated object behind a table of an `int` and a
//! `string` column — is ONE row of the table below; the rows differ along the declared
//! column types (every `ColumnStatisticsType` a type maps to), the number of objects under
//! the location, the field delimiter, the null marker, whether a header line is skipped,
//! whether the rows are delimited text or one JSON document per line, and whether the
//! layout quotes its fields.
//!
//! The due instant a run's terminal status falls at is moved THROUGH THE STORE rather than
//! slept for ([`crate::column_statistics_task_runs::set_due_instant`]), so every
//! transition here is deterministic.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::messages::COLUMN_STATISTICS_NOT_FOUND;
use service_com_amazonaws_glue_awsglue::state::{
    column_statistics_task_run_unreadable_data, COLUMN_STATISTICS_TASK_RUN_NO_TABLE_LOCATION,
    COLUMN_STATISTICS_TASK_RUN_STATE_FAILED, COLUMN_STATISTICS_TASK_RUN_STATE_STARTING,
    COLUMN_STATISTICS_TASK_RUN_STATE_SUCCEEDED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::column_statistics_task_runs::{already_past, far_future, set_due_instant};
use crate::wire::{call, error_code, ok};

/// The IAM role a start names: "The IAM role that the service assumes to generate
/// statistics."
const ROLE: &str = "arn:aws:iam::000000000000:role/glue-column-stats-role";

/// The input format of a table whose objects are lines of text, which is what the recorded
/// table carries.
const TEXT_INPUT_FORMAT: &str = "org.apache.hadoop.mapred.TextInputFormat";

/// The serialization library of a Hive delimited-text table.
const LAZY_SIMPLE_SERDE: &str = "org.apache.hadoop.hive.serde2.lazy.LazySimpleSerDe";

/// The serialization library of a table whose rows are quoted comma-separated text.
const OPEN_CSV_SERDE: &str = "org.apache.hadoop.hive.serde2.OpenCSVSerde";

/// The serialization library of a table whose rows are one JSON document each.
const JSON_SERDE: &str = "org.openx.data.jsonserde.JsonSerDe";

/// The prefix, under each row's own bucket, that a table's data lives at.
const DATA_PREFIX: &str = "data/";

/// An emulator with Glue AND Amazon S3 mounted on one shared engine, plus the engine
/// itself so a test can move the due instant a run's terminal status falls at.
///
/// Amazon S3 is mounted because a column statistics task run READS the objects behind the
/// covered table: the run reaches them over the emulator's own in-process callback, so the
/// object a test stages with `PutObject` is the object the run aggregates.
fn emulator_with_s3() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// An Amazon S3 client over this emulator's own in-process callback — the seam a run reads
/// through, so what a test stages here is what the run aggregates. The shared harness owns
/// the one client configuration ([`crate::wire::s3_client`]).
fn s3(router: &MultiServiceRouter) -> aws_sdk_s3::Client {
    crate::wire::s3_client(router)
}

/// Create `bucket` and put every `(key under the data prefix, body)` into it.
async fn stage_objects(router: &MultiServiceRouter, bucket: &str, objects: &[(&str, &str)]) {
    let client = s3(router);
    client
        .create_bucket()
        .bucket(bucket)
        .send()
        .await
        .unwrap_or_else(|error| panic!("the bucket {bucket} must be created, got {error:?}"));
    stage_objects_into(router, bucket, objects).await;
}

/// Put further objects into a bucket that already exists.
async fn stage_objects_into(router: &MultiServiceRouter, bucket: &str, objects: &[(&str, &str)]) {
    let client = s3(router);
    for (key, body) in objects {
        client
            .put_object()
            .bucket(bucket)
            .key(format!("{DATA_PREFIX}{key}"))
            .body(aws_sdk_s3::primitives::ByteStream::from(
                body.as_bytes().to_vec(),
            ))
            .send()
            .await
            .unwrap_or_else(|error| panic!("the object {key} must be stored, got {error:?}"));
    }
}

/// The bucket a row's objects live in: its own name, in the character set a bucket name
/// admits (`CreateBucket` refuses an underscore).
fn bucket_of(name: &str) -> String {
    name.replace('_', "-")
}

/// Where a row's table points its data at: the prefix inside its own bucket.
fn location_of(name: &str) -> String {
    format!("s3://{}/{DATA_PREFIX}", bucket_of(name))
}

/// Store `database` holding `table`, whose storage descriptor names `columns`, the layout
/// `serde` describes and the location `location`.
async fn store_table(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    columns: &[(&str, &str)],
    descriptor: Value,
) {
    let (status, parsed) = call(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database } }),
    )
    .await;
    assert!(
        status == 200 || error_code(&parsed).ends_with("AlreadyExistsException"),
        "CreateDatabase must answer 200 or report the database is already stored, got {status} \
         with {parsed}"
    );
    let mut storage_descriptor = descriptor;
    storage_descriptor["Columns"] = columns
        .iter()
        .map(|(name, declared)| json!({ "Name": name, "Type": declared }))
        .collect::<Vec<Value>>()
        .into();
    ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": database,
            "TableInput": {
                "Name": table,
                "TableType": "EXTERNAL_TABLE",
                "StorageDescriptor": storage_descriptor,
            },
        }),
    )
    .await;
}

/// The storage descriptor of a delimited-text table at `name`'s bucket, with the serde
/// parameters `parameters` adds and the descriptor members `extra` adds.
///
/// A member `extra` names replaces the one here, so a row that states its own `SerdeInfo`
/// states the whole of it — its serialization library AND its parameters.
fn text_descriptor(name: &str, parameters: Value, extra: Value) -> Value {
    let mut descriptor = json!({
        "Location": location_of(name),
        "InputFormat": TEXT_INPUT_FORMAT,
        "SerdeInfo": {
            "SerializationLibrary": LAZY_SIMPLE_SERDE,
            "Parameters": parameters,
        },
    });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            descriptor[member] = value.clone();
        }
    }
    descriptor
}

/// Start a run of `table` and answer the `ColumnStatisticsTaskRunId` it reports.
async fn start_run(router: &MultiServiceRouter, database: &str, table: &str, extra: Value) -> String {
    let mut body = json!({ "DatabaseName": database, "TableName": table, "Role": ROLE });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    let started = ok(router, "StartColumnStatisticsTaskRun", &body).await;
    started
        .get("ColumnStatisticsTaskRunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the start must report a run id, got {started}"))
        .to_string()
}

/// The `ColumnStatisticsTaskRun` a read reports for one run.
async fn get_run(router: &MultiServiceRouter, run_id: &str) -> Value {
    let body = ok(
        router,
        "GetColumnStatisticsTaskRun",
        &json!({ "ColumnStatisticsTaskRunId": run_id }),
    )
    .await;
    body.get("ColumnStatisticsTaskRun")
        .cloned()
        .unwrap_or_else(|| panic!("the read must report a ColumnStatisticsTaskRun, got {body}"))
}

/// The answer `GetColumnStatisticsForTable` gives for `columns` of one table.
async fn get_statistics(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    columns: &[&str],
) -> Value {
    ok(
        router,
        "GetColumnStatisticsForTable",
        &json!({ "DatabaseName": database, "TableName": table, "ColumnNames": columns }),
    )
    .await
}

/// The `ColumnStatisticsList` of an answer, keyed by the column each entry belongs to.
fn statistics_by_column(answer: &Value) -> Vec<(String, Value)> {
    answer
        .get("ColumnStatisticsList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the answer must carry a ColumnStatisticsList, got {answer}"))
        .iter()
        .map(|entry| {
            (
                entry
                    .get("ColumnName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                entry.clone(),
            )
        })
        .collect()
}

/// A numeric member of a reported structure, taking an absent member as the model's own
/// default of 0 — awsJson omits a member whose value equals its modelled default.
fn number(structure: &Value, member: &str) -> f64 {
    structure
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_default()
}

/// Which member of `ColumnStatisticsData` carries the statistics of a column reported
/// under `statistics_type`: "only one data object should be set and indicated by the
/// `Type` attribute", and the member names are the modelled ones
/// (`smithy_query shape=ColumnStatisticsData`).
fn data_member(statistics_type: &str) -> &'static str {
    match statistics_type {
        "BOOLEAN" => "BooleanColumnStatisticsData",
        "DATE" => "DateColumnStatisticsData",
        "DECIMAL" => "DecimalColumnStatisticsData",
        "DOUBLE" => "DoubleColumnStatisticsData",
        "LONG" => "LongColumnStatisticsData",
        "STRING" => "StringColumnStatisticsData",
        "BINARY" => "BinaryColumnStatisticsData",
        other => panic!("{other} is not a modelled ColumnStatisticsType"),
    }
}

/// What one column's statistics must report: the `ColumnStatisticsType` it is reported
/// under, and the value of each numeric member of that type's data structure.
struct ExpectedColumn<'a> {
    column: &'a str,
    statistics_type: &'a str,
    numbers: &'a [(&'a str, f64)],
}

/// One row of the table below: a stored table, the objects behind it, and the statistics
/// each of its columns must report once a run over it has finished.
struct ComputationCase<'a> {
    case: &'a str,
    /// The bucket the row's objects are staged in, which is also its database and table
    /// name — one row, one table.
    name: &'a str,
    columns: &'a [(&'a str, &'a str)],
    /// The serde parameters the table states.
    parameters: Value,
    /// Further storage-descriptor members the table states.
    descriptor: Value,
    objects: &'a [(&'a str, &'a str)],
    expected: &'a [ExpectedColumn<'a>],
}

/// The statistics a finished run reports are the statistics of the data behind the table:
/// one row per layout and per set of declared column types.
#[tokio::test]
async fn a_finished_run_reports_the_statistics_of_the_data_behind_the_table() {
    // The recorded case's own data: 20 rows of `<i>,cat<i % 4>`, so the int column holds
    // 20 distinct values and the string column 4, with no empty cell.
    let recorded: String = (1..=20)
        .map(|index| format!("{index},cat{}\n", index % 4))
        .collect();
    let recorded_objects = [("data.csv", recorded.as_str())];
    let cases: Vec<ComputationCase<'_>> = vec![
        ComputationCase {
            case: "the recorded shape: an int and a string column over one comma-separated object",
            name: "recorded",
            columns: &[("id", "int"), ("category", "string")],
            parameters: json!({ "field.delim": "," }),
            descriptor: json!({}),
            objects: &recorded_objects,
            expected: &[
                ExpectedColumn {
                    column: "id",
                    statistics_type: "LONG",
                    numbers: &[
                        ("NumberOfNulls", 0.0),
                        ("NumberOfDistinctValues", 20.0),
                        ("MinimumValue", 1.0),
                        ("MaximumValue", 20.0),
                    ],
                },
                ExpectedColumn {
                    column: "category",
                    statistics_type: "STRING",
                    numbers: &[
                        ("NumberOfNulls", 0.0),
                        ("NumberOfDistinctValues", 4.0),
                        ("MaximumLength", 4.0),
                        ("AverageLength", 4.0),
                    ],
                },
            ],
        },
        ComputationCase {
            case: "several objects under the location, aggregated together",
            name: "many_objects",
            columns: &[("id", "bigint")],
            parameters: json!({ "field.delim": "," }),
            descriptor: json!({}),
            objects: &[("part-0", "1\n2\n3\n"), ("part-1", "3\n4\n"), ("part-2", "9\n")],
            expected: &[ExpectedColumn {
                column: "id",
                statistics_type: "LONG",
                numbers: &[
                    ("NumberOfNulls", 0.0),
                    // 1, 2, 3, 3, 4, 9 across three objects: five distinct values.
                    ("NumberOfDistinctValues", 5.0),
                    ("MinimumValue", 1.0),
                    ("MaximumValue", 9.0),
                ],
            }],
        },
        ComputationCase {
            case: "another delimiter and a stated null marker",
            name: "pipe_delimited",
            columns: &[("amount", "double"), ("label", "string")],
            parameters: json!({ "field.delim": "|", "serialization.null.format": "NULL" }),
            descriptor: json!({}),
            objects: &[("rows", "1.5|alpha\nNULL|\n2.5|beta\n1.5|alpha\n")],
            expected: &[
                ExpectedColumn {
                    column: "amount",
                    statistics_type: "DOUBLE",
                    numbers: &[
                        ("NumberOfNulls", 1.0),
                        ("NumberOfDistinctValues", 2.0),
                        ("MinimumValue", 1.5),
                        ("MaximumValue", 2.5),
                    ],
                },
                ExpectedColumn {
                    column: "label",
                    statistics_type: "STRING",
                    numbers: &[
                        // An empty cell of a character-sequence column is the empty
                        // sequence, not a null: only the stated marker is one.
                        ("NumberOfNulls", 0.0),
                        ("NumberOfDistinctValues", 3.0),
                        ("MaximumLength", 5.0),
                        ("AverageLength", 3.5),
                    ],
                },
            ],
        },
        ComputationCase {
            case: "a header line the table says to skip",
            name: "with_header",
            columns: &[("id", "int"), ("name", "string")],
            parameters: json!({ "field.delim": "," }),
            // The Hive table property that says how many leading lines of each object are
            // a header rather than data.
            descriptor: json!({ "Parameters": { "skip.header.line.count": "1" } }),
            objects: &[("rows.csv", "id,name\n1,ann\n2,bob\n")],
            expected: &[
                ExpectedColumn {
                    column: "id",
                    statistics_type: "LONG",
                    numbers: &[
                        // The header line is not data, so it is neither a null nor a
                        // value: two rows, two distinct ids.
                        ("NumberOfNulls", 0.0),
                        ("NumberOfDistinctValues", 2.0),
                        ("MinimumValue", 1.0),
                        ("MaximumValue", 2.0),
                    ],
                },
                ExpectedColumn {
                    column: "name",
                    statistics_type: "STRING",
                    numbers: &[
                        ("NumberOfNulls", 0.0),
                        ("NumberOfDistinctValues", 2.0),
                        ("MaximumLength", 3.0),
                        ("AverageLength", 3.0),
                    ],
                },
            ],
        },
        ComputationCase {
            case: "one column of every type that has statistics",
            name: "every_type",
            columns: &[
                ("flag", "boolean"),
                ("count", "bigint"),
                ("ratio", "double"),
                ("note", "string"),
                ("day", "date"),
                ("price", "decimal(10,2)"),
                ("blob", "binary"),
            ],
            parameters: json!({ "field.delim": "," }),
            descriptor: json!({}),
            // The third row holds the default Hive null marker in every cell.
            objects: &[(
                "rows",
                "true,10,1.5,hello,2026-01-02,12.50,QUJD\n\
                 false,20,2.5,hi,2026-01-03,7.25,QQ\n\
                 \\N,\\N,\\N,\\N,\\N,\\N,\\N\n",
            )],
            expected: &[
                ExpectedColumn {
                    column: "flag",
                    statistics_type: "BOOLEAN",
                    numbers: &[
                        ("NumberOfTrues", 1.0),
                        ("NumberOfFalses", 1.0),
                        ("NumberOfNulls", 1.0),
                    ],
                },
                ExpectedColumn {
                    column: "count",
                    statistics_type: "LONG",
                    numbers: &[
                        ("NumberOfNulls", 1.0),
                        ("NumberOfDistinctValues", 2.0),
                        ("MinimumValue", 10.0),
                        ("MaximumValue", 20.0),
                    ],
                },
                ExpectedColumn {
                    column: "ratio",
                    statistics_type: "DOUBLE",
                    numbers: &[
                        ("NumberOfNulls", 1.0),
                        ("NumberOfDistinctValues", 2.0),
                        ("MinimumValue", 1.5),
                        ("MaximumValue", 2.5),
                    ],
                },
                ExpectedColumn {
                    column: "note",
                    statistics_type: "STRING",
                    numbers: &[
                        ("NumberOfNulls", 1.0),
                        ("NumberOfDistinctValues", 2.0),
                        ("MaximumLength", 5.0),
                        ("AverageLength", 3.5),
                    ],
                },
                ExpectedColumn {
                    column: "day",
                    statistics_type: "DATE",
                    numbers: &[("NumberOfNulls", 1.0), ("NumberOfDistinctValues", 2.0)],
                },
                ExpectedColumn {
                    column: "price",
                    statistics_type: "DECIMAL",
                    numbers: &[("NumberOfNulls", 1.0), ("NumberOfDistinctValues", 2.0)],
                },
                ExpectedColumn {
                    column: "blob",
                    statistics_type: "BINARY",
                    numbers: &[
                        ("NumberOfNulls", 1.0),
                        // A text-format table writes a binary value base64-encoded, so the
                        // lengths are the VALUE's: `QUJD` decodes to 3 bytes and `QQ` to 1,
                        // for a longest of 3 and an average of 2.
                        ("MaximumLength", 3.0),
                        ("AverageLength", 2.0),
                    ],
                },
            ],
        },
        ComputationCase {
            case: "one JSON document per line, whose members are the columns",
            name: "json_lines",
            columns: &[("id", "int"), ("name", "string")],
            parameters: json!({}),
            descriptor: json!({ "SerdeInfo": { "SerializationLibrary": JSON_SERDE } }),
            objects: &[(
                "rows.json",
                "{\"id\":1,\"name\":\"ann\"}\n\
                 {\"id\":2,\"name\":null}\n\
                 {\"id\":1,\"name\":\"bob\"}\n",
            )],
            expected: &[
                ExpectedColumn {
                    column: "id",
                    statistics_type: "LONG",
                    numbers: &[
                        ("NumberOfNulls", 0.0),
                        ("NumberOfDistinctValues", 2.0),
                        ("MinimumValue", 1.0),
                        ("MaximumValue", 2.0),
                    ],
                },
                ExpectedColumn {
                    column: "name",
                    statistics_type: "STRING",
                    numbers: &[
                        // JSON's own null is a null cell.
                        ("NumberOfNulls", 1.0),
                        ("NumberOfDistinctValues", 2.0),
                        ("MaximumLength", 3.0),
                        ("AverageLength", 3.0),
                    ],
                },
            ],
        },
        ComputationCase {
            case: "quoted comma-separated rows, whose quoted field carries the delimiter",
            name: "quoted_csv",
            columns: &[("id", "int"), ("label", "string")],
            parameters: json!({}),
            descriptor: json!({
                "SerdeInfo": {
                    "SerializationLibrary": OPEN_CSV_SERDE,
                    "Parameters": { "separatorChar": ",", "quoteChar": "\"" },
                }
            }),
            objects: &[("rows.csv", "\"1\",\"a,b\"\n\"2\",\"c\"\n")],
            expected: &[
                ExpectedColumn {
                    column: "id",
                    statistics_type: "LONG",
                    numbers: &[
                        ("NumberOfNulls", 0.0),
                        ("NumberOfDistinctValues", 2.0),
                        ("MinimumValue", 1.0),
                        ("MaximumValue", 2.0),
                    ],
                },
                ExpectedColumn {
                    column: "label",
                    statistics_type: "STRING",
                    numbers: &[
                        ("NumberOfNulls", 0.0),
                        ("NumberOfDistinctValues", 2.0),
                        // The quotes are not part of the value, so `a,b` is one cell of
                        // three characters.
                        ("MaximumLength", 3.0),
                        ("AverageLength", 2.0),
                    ],
                },
            ],
        },
        ComputationCase {
            case: "a table that states no input format, whose objects are read as text",
            name: "default_format",
            columns: &[("id", "int")],
            parameters: json!({ "field.delim": "," }),
            descriptor: json!({ "InputFormat": Value::Null }),
            objects: &[("rows", "7\n8\n")],
            expected: &[ExpectedColumn {
                column: "id",
                statistics_type: "LONG",
                numbers: &[
                    ("NumberOfNulls", 0.0),
                    ("NumberOfDistinctValues", 2.0),
                    ("MinimumValue", 7.0),
                    ("MaximumValue", 8.0),
                ],
            }],
        },
    ];

    for ComputationCase {
        case,
        name,
        columns,
        parameters,
        descriptor,
        objects,
        expected,
    } in cases
    {
        let (router, engine) = emulator_with_s3();
        stage_objects(&router, &bucket_of(name), objects).await;
        let mut storage_descriptor = text_descriptor(name, parameters, descriptor);
        // A row that states no input format carries none at all rather than a null.
        if storage_descriptor
            .get("InputFormat")
            .is_some_and(Value::is_null)
        {
            storage_descriptor
                .as_object_mut()
                .expect("the descriptor is an object")
                .remove("InputFormat");
        }
        store_table(&router, name, name, columns, storage_descriptor).await;

        let run_id = start_run(&router, name, name, json!({})).await;
        set_due_instant(&engine, already_past());
        let run = get_run(&router, &run_id).await;
        assert_eq!(
            run.get("Status").and_then(Value::as_str),
            Some(COLUMN_STATISTICS_TASK_RUN_STATE_SUCCEEDED),
            "case {case:?}: a run that read the table's data succeeds, got {run}"
        );
        assert!(
            run.get("ErrorMessage").is_none(),
            "case {case:?}: a run that generated the statistics failed at nothing, so it carries \
             no ErrorMessage, got {run}"
        );

        let requested: Vec<&str> = columns.iter().map(|(column, _)| *column).collect();
        let answer = get_statistics(&router, name, name, &requested).await;
        assert_eq!(
            answer.get("Errors").and_then(Value::as_array),
            Some(&Vec::new()),
            "case {case:?}: every requested column is one the table declares, so nothing failed, \
             got {answer}"
        );
        let reported = statistics_by_column(&answer);
        assert_eq!(
            reported.len(),
            expected.len(),
            "case {case:?}: expected statistics for {} columns, got {answer}",
            expected.len()
        );
        for ExpectedColumn {
            column,
            statistics_type,
            numbers,
        } in expected
        {
            let (_, entry) = reported
                .iter()
                .find(|(reported_column, _)| reported_column == column)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the answer must carry column {column:?}, got {answer}")
                });
            let declared = columns
                .iter()
                .find(|(name, _)| name == column)
                .map(|(_, declared)| *declared)
                .unwrap_or_default();
            assert_eq!(
                entry.get("ColumnType").and_then(Value::as_str),
                Some(declared),
                "case {case:?}: column {column:?} reports the type the table declares, got {entry}"
            );
            assert!(
                number(entry, "AnalyzedTime") > 0.0,
                "case {case:?}: column {column:?} reports when its statistics were generated, got \
                 {entry}"
            );
            let data = entry
                .get("StatisticsData")
                .unwrap_or_else(|| panic!("case {case:?}: column {column:?} carries no data"));
            assert_eq!(
                data.get("Type").and_then(Value::as_str),
                Some(*statistics_type),
                "case {case:?}: column {column:?} is reported under the type its declared type \
                 maps to, got {data}"
            );
            let member = data_member(statistics_type);
            let figures = data
                .get(member)
                .unwrap_or_else(|| panic!("case {case:?}: expected {member} in {data}"));
            for (figure, value) in *numbers {
                assert!(
                    (number(figures, figure) - value).abs() < f64::EPSILON,
                    "case {case:?}: column {column:?} must report {figure} {value}, got {figures}"
                );
            }
            // Only the one data member the type names carries figures.
            for other in [
                "BooleanColumnStatisticsData",
                "DateColumnStatisticsData",
                "DecimalColumnStatisticsData",
                "DoubleColumnStatisticsData",
                "LongColumnStatisticsData",
                "StringColumnStatisticsData",
                "BinaryColumnStatisticsData",
            ] {
                assert!(
                    other == member || data.get(other).is_none(),
                    "case {case:?}: column {column:?} carries only {member}, got {data}"
                );
            }
        }
    }
}

/// The header line is skipped whether the property sits on the storage descriptor or on
/// the TABLE itself, which is where a create that states it beside the table's other
/// properties puts it.
#[tokio::test]
async fn a_header_line_the_table_itself_declares_is_not_data() {
    let (router, engine) = emulator_with_s3();
    stage_objects(
        &router,
        &bucket_of("table-header"),
        &[("rows.csv", "id,name\n1,ann\n2,bob\n")],
    )
    .await;
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": "table_header" } }),
    )
    .await;
    ok(
        &router,
        "CreateTable",
        &json!({
            "DatabaseName": "table_header",
            "TableInput": {
                "Name": "table_header",
                "TableType": "EXTERNAL_TABLE",
                // The Hive property on the TABLE rather than on its storage descriptor.
                "Parameters": { "skip.header.line.count": "1" },
                "StorageDescriptor": {
                    "Location": location_of("table-header"),
                    "InputFormat": TEXT_INPUT_FORMAT,
                    "SerdeInfo": {
                        "SerializationLibrary": LAZY_SIMPLE_SERDE,
                        "Parameters": { "field.delim": "," },
                    },
                    "Columns": [
                        { "Name": "id", "Type": "int" },
                        { "Name": "name", "Type": "string" },
                    ],
                },
            },
        }),
    )
    .await;

    let run_id = start_run(&router, "table_header", "table_header", json!({})).await;
    set_due_instant(&engine, already_past());
    let run = get_run(&router, &run_id).await;
    assert_eq!(
        run.get("Status").and_then(Value::as_str),
        Some(COLUMN_STATISTICS_TASK_RUN_STATE_SUCCEEDED),
        "the run over the staged data succeeds, got {run}"
    );

    let answer = get_statistics(&router, "table_header", "table_header", &["id"]).await;
    let figures = &answer["ColumnStatisticsList"][0]["StatisticsData"]["LongColumnStatisticsData"];
    assert!(
        (number(figures, "NumberOfNulls") - 0.0).abs() < f64::EPSILON
            && (number(figures, "NumberOfDistinctValues") - 2.0).abs() < f64::EPSILON
            && (number(figures, "MinimumValue") - 1.0).abs() < f64::EPSILON
            && (number(figures, "MaximumValue") - 2.0).abs() < f64::EPSILON,
        "case {:?}: the header line is not data, so two rows hold two distinct ids and no null, \
         got {figures}",
        "the property on the table itself"
    );
}

/// The extremes of a date and of a fixed-point column are the values the data holds, in
/// the shapes those types report them in: an instant for a date, and an unscaled value
/// with its scale for a decimal.
#[tokio::test]
async fn the_extremes_of_a_date_and_a_decimal_column_are_the_values_the_data_holds() {
    let (router, engine) = emulator_with_s3();
    stage_objects(
        &router,
        &bucket_of("extremes"),
        &[("rows", "2026-01-02,12.50\n2026-01-03,7.25\n")],
    )
    .await;
    store_table(
        &router,
        "extremes",
        "extremes",
        &[("day", "date"), ("price", "decimal(10,2)")],
        text_descriptor("extremes", json!({ "field.delim": "," }), json!({})),
    )
    .await;

    let run_id = start_run(&router, "extremes", "extremes", json!({})).await;
    set_due_instant(&engine, already_past());
    let run = get_run(&router, &run_id).await;
    assert_eq!(
        run.get("Status").and_then(Value::as_str),
        Some(COLUMN_STATISTICS_TASK_RUN_STATE_SUCCEEDED),
        "the run over the staged data succeeds, got {run}"
    );

    let answer = get_statistics(&router, "extremes", "extremes", &["day", "price"]).await;
    let reported = statistics_by_column(&answer);
    let day = reported
        .iter()
        .find(|(column, _)| column == "day")
        .map(|(_, entry)| entry.clone())
        .unwrap_or_else(|| panic!("the answer must carry the date column, got {answer}"));
    let dates = &day["StatisticsData"]["DateColumnStatisticsData"];
    // The expectation is the instant the cell names, read through the same ISO-8601
    // parser a client would use rather than spelled as a magic number.
    for (member, civil) in [
        ("MinimumValue", "2026-01-02T00:00:00Z"),
        ("MaximumValue", "2026-01-03T00:00:00Z"),
    ] {
        let expected = DateTime::from_str(civil, Format::DateTime)
            .unwrap_or_else(|error| panic!("the case is a date-time, got {error}"))
            .secs() as f64;
        assert!(
            (number(dates, member) - expected).abs() < f64::EPSILON,
            "case {civil:?}: the date column must report {member} {expected}, got {dates}"
        );
    }

    let price = reported
        .iter()
        .find(|(column, _)| column == "price")
        .map(|(_, entry)| entry.clone())
        .unwrap_or_else(|| panic!("the answer must carry the decimal column, got {answer}"));
    let decimals = &price["StatisticsData"]["DecimalColumnStatisticsData"];
    // A `DecimalNumber` carries "big-endian, two's complement representations of the
    // decimal's unscaled value" with the scale beside it: 7.25 is the unscaled 725
    // (0x02D5) at scale 2, and 12.50 the unscaled 1250 (0x04E2) at the same scale. The
    // blob travels base64-encoded, as awsJson encodes every blob.
    for (member, unscaled, scale) in [("MinimumValue", "AtU=", 2.0), ("MaximumValue", "BOI=", 2.0)] {
        let reported = &decimals[member];
        assert_eq!(
            reported.get("UnscaledValue").and_then(Value::as_str),
            Some(unscaled),
            "case {member:?}: the decimal column reports the unscaled value of the cell, got \
             {reported}"
        );
        assert!(
            (number(reported, "Scale") - scale).abs() < f64::EPSILON,
            "case {member:?}: the decimal column reports the scale the cell was written with, got \
             {reported}"
        );
    }
}

/// The statistics a run generates become readable only once the run has FINISHED, and a
/// read of them changes nothing: two reads of a settled table's statistics answer the
/// same.
#[tokio::test]
async fn statistics_are_published_when_the_run_finishes_and_a_read_moves_nothing() {
    let (router, engine) = emulator_with_s3();
    stage_objects(&router, &bucket_of("published"), &[("rows", "1\n2\n2\n")]).await;
    store_table(
        &router,
        "published",
        "published",
        &[("id", "int")],
        text_descriptor("published", json!({ "field.delim": "," }), json!({})),
    )
    .await;

    let run_id = start_run(&router, "published", "published", json!({})).await;
    // Hold the transient window open: the run has read the data, but the clock has not
    // finished it yet.
    set_due_instant(&engine, far_future());
    let in_flight = get_run(&router, &run_id).await;
    assert_eq!(
        in_flight.get("Status").and_then(Value::as_str),
        Some(COLUMN_STATISTICS_TASK_RUN_STATE_STARTING),
        "a run read straight after the start is still in flight, got {in_flight}"
    );
    let before = get_statistics(&router, "published", "published", &["id"]).await;
    assert_eq!(
        statistics_by_column(&before).len(),
        0,
        "case {:?}: a run that has not finished has generated nothing yet, got {before}",
        "while the run is in flight"
    );

    set_due_instant(&engine, already_past());
    let first = get_statistics(&router, "published", "published", &["id"]).await;
    let entries = statistics_by_column(&first);
    assert_eq!(
        entries.len(),
        1,
        "case {:?}: the finished run published the statistics of the column it covered, got \
         {first}",
        "once the run has finished"
    );
    let figures = &first["ColumnStatisticsList"][0]["StatisticsData"]["LongColumnStatisticsData"];
    assert!(
        (number(figures, "NumberOfDistinctValues") - 2.0).abs() < f64::EPSILON
            && (number(figures, "NumberOfNulls") - 0.0).abs() < f64::EPSILON,
        "case {:?}: three rows holding two distinct ids and no null, got {figures}",
        "once the run has finished"
    );

    let again = get_statistics(&router, "published", "published", &["id"]).await;
    assert_eq!(
        again, first,
        "case {:?}: reading statistics must not change what a later caller sees, got {again} \
         after {first}",
        "read twice"
    );
}

/// A later run over the same table REPLACES the statistics of the columns it covers: the
/// statistics a table carries are the ones the last run computed from the data behind it.
#[tokio::test]
async fn a_later_run_replaces_the_statistics_of_the_columns_it_covers() {
    let (router, engine) = emulator_with_s3();
    stage_objects(&router, &bucket_of("recomputed"), &[("rows", "1\n2\n")]).await;
    store_table(
        &router,
        "recomputed",
        "recomputed",
        &[("id", "int")],
        text_descriptor("recomputed", json!({ "field.delim": "," }), json!({})),
    )
    .await;

    let first_run = start_run(&router, "recomputed", "recomputed", json!({})).await;
    set_due_instant(&engine, already_past());
    let _ = get_run(&router, &first_run).await;
    let first = get_statistics(&router, "recomputed", "recomputed", &["id"]).await;
    let figures = &first["ColumnStatisticsList"][0]["StatisticsData"]["LongColumnStatisticsData"];
    assert!(
        (number(figures, "NumberOfDistinctValues") - 2.0).abs() < f64::EPSILON,
        "case {:?}: two rows hold two distinct ids, got {figures}",
        "the first run"
    );

    // The data behind the table grows, and a second run reads it as it now stands.
    stage_objects_into(&router, &bucket_of("recomputed"), &[("more", "3\n4\n5\n")]).await;
    let second_run = start_run(&router, "recomputed", "recomputed", json!({})).await;
    set_due_instant(&engine, already_past());
    let second = get_run(&router, &second_run).await;
    assert_eq!(
        second.get("Status").and_then(Value::as_str),
        Some(COLUMN_STATISTICS_TASK_RUN_STATE_SUCCEEDED),
        "the second run reads the data too, got {second}"
    );
    let after = get_statistics(&router, "recomputed", "recomputed", &["id"]).await;
    let figures = &after["ColumnStatisticsList"][0]["StatisticsData"]["LongColumnStatisticsData"];
    assert!(
        (number(figures, "NumberOfDistinctValues") - 5.0).abs() < f64::EPSILON
            && (number(figures, "MaximumValue") - 5.0).abs() < f64::EPSILON,
        "case {:?}: the statistics are the ones the LAST run computed, got {figures}",
        "the second run"
    );
}

/// Only the columns the run COVERS get statistics, and only the columns that HAVE them are
/// reported: every other requested column — one the run did not cover as much as a name the
/// table declares no column under — is reported in `Errors`, because the catalog holds no
/// statistics for it.
#[tokio::test]
async fn statistics_cover_the_columns_the_run_named_and_errors_name_the_rest() {
    let (router, engine) = emulator_with_s3();
    stage_objects(
        &router,
        &bucket_of("covered"),
        &[("rows", "1,alpha,x\n2,beta,y\n")],
    )
    .await;
    store_table(
        &router,
        "covered",
        "covered",
        &[("id", "int"), ("label", "string"), ("extra", "string")],
        text_descriptor("covered", json!({ "field.delim": "," }), json!({})),
    )
    .await;

    let run_id = start_run(
        &router,
        "covered",
        "covered",
        json!({ "ColumnNameList": ["id", "label"] }),
    )
    .await;
    set_due_instant(&engine, already_past());
    let run = get_run(&router, &run_id).await;
    assert_eq!(
        run.get("Status").and_then(Value::as_str),
        Some(COLUMN_STATISTICS_TASK_RUN_STATE_SUCCEEDED),
        "the run over the covered columns succeeds, got {run}"
    );

    let answer = get_statistics(
        &router,
        "covered",
        "covered",
        &["id", "label", "extra", "never_declared"],
    )
    .await;
    let reported: Vec<String> = statistics_by_column(&answer)
        .into_iter()
        .map(|(column, _)| column)
        .collect();
    assert_eq!(
        reported,
        vec!["id".to_string(), "label".to_string()],
        "case {:?}: only the columns the run covered carry statistics, got {answer}",
        "a run over two of three columns"
    );
    let errors = answer
        .get("Errors")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the answer must carry an Errors list, got {answer}"));
    let failed: Vec<&str> = errors
        .iter()
        .map(|entry| entry.get("ColumnName").and_then(Value::as_str).unwrap_or(""))
        .collect();
    assert_eq!(
        failed,
        vec!["extra", "never_declared"],
        "case {:?}: every requested column the catalog holds no statistics for is reported, got \
         {answer}",
        "a run over two of three columns"
    );
    for entry in errors {
        let detail = &entry["Error"];
        assert_eq!(
            detail.get("ErrorMessage").and_then(Value::as_str),
            Some(COLUMN_STATISTICS_NOT_FOUND),
            "case {:?}: a column with no statistics is reported with the catalog's own wording, \
             got {detail}",
            "a run over two of three columns"
        );
    }
}

/// What makes a run FAIL: data it could not read. Each row is a table whose location
/// offers the run nothing, and the reason the run must report for it — and no row leaves
/// any statistics behind.
#[tokio::test]
async fn a_run_fails_only_when_the_tables_data_could_not_be_read() {
    let cases: Vec<(&str, Value, bool, String)> = vec![
        (
            "a location whose bucket holds no object under the prefix",
            text_descriptor("empty_prefix", json!({ "field.delim": "," }), json!({})),
            true,
            column_statistics_task_run_unreadable_data(&location_of("empty_prefix")),
        ),
        (
            "a location whose bucket does not exist",
            text_descriptor("never_created", json!({ "field.delim": "," }), json!({})),
            false,
            column_statistics_task_run_unreadable_data(&location_of("never_created")),
        ),
        (
            "a table that states no location at all",
            json!({ "InputFormat": TEXT_INPUT_FORMAT }),
            false,
            COLUMN_STATISTICS_TASK_RUN_NO_TABLE_LOCATION.to_string(),
        ),
        (
            "a location whose objects are not lines of text",
            text_descriptor(
                "parquet_table",
                json!({ "field.delim": "," }),
                json!({
                    "InputFormat": "org.apache.hadoop.hive.ql.io.parquet.MapredParquetInputFormat"
                }),
            ),
            true,
            column_statistics_task_run_unreadable_data(&location_of("parquet_table")),
        ),
    ];

    for (case, descriptor, create_bucket, expected) in cases {
        let (router, engine) = emulator_with_s3();
        if create_bucket {
            let bucket = descriptor["Location"]
                .as_str()
                .and_then(|location| location.strip_prefix("s3://"))
                .and_then(|rest| rest.split('/').next())
                .unwrap_or_default()
                .to_string();
            // The bucket exists; what it does not hold is a readable object under the
            // table's own prefix.
            stage_objects(&router, &bucket, &[]).await;
            let client = s3(&router);
            client
                .put_object()
                .bucket(&bucket)
                .key("elsewhere/other.csv")
                .body(aws_sdk_s3::primitives::ByteStream::from(
                    b"1\n".to_vec(),
                ))
                .send()
                .await
                .unwrap_or_else(|error| panic!("case {case:?}: the object must be stored, got {error:?}"));
        }
        store_table(&router, "unreadable", "unreadable", &[("id", "int")], descriptor).await;
        let run_id = start_run(&router, "unreadable", "unreadable", json!({})).await;
        set_due_instant(&engine, already_past());
        let run = get_run(&router, &run_id).await;
        assert_eq!(
            run.get("Status").and_then(Value::as_str),
            Some(COLUMN_STATISTICS_TASK_RUN_STATE_FAILED),
            "case {case:?}: a run that could not read the data fails, got {run}"
        );
        assert_eq!(
            run.get("ErrorMessage").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: the run reports why it could not generate statistics, got {run}"
        );
        let answer = get_statistics(&router, "unreadable", "unreadable", &["id"]).await;
        assert_eq!(
            statistics_by_column(&answer).len(),
            0,
            "case {case:?}: a failed run leaves no statistics behind, got {answer}"
        );
    }
}

/// Data the layout says is one thing and the bytes are another fails the run too, and
/// leaves no statistics: the rows differ along WHICH way the object breaks its own layout.
#[tokio::test]
async fn data_that_breaks_its_own_layout_fails_the_run_and_leaves_no_statistics() {
    // Each row is `(case, the object's bytes)`, staged behind a table whose rows are one
    // JSON document each — the layout a row can break in both ways.
    let cases: Vec<(&str, Vec<u8>)> = vec![
        (
            "a row that is not the JSON document the layout states",
            b"{\"id\":1}\nnot a document at all\n".to_vec(),
        ),
        (
            "bytes that are not the UTF-8 text the layout states",
            vec![0x7B, 0x22, 0x69, 0x64, 0x22, 0x3A, 0xFF, 0xFE, 0x7D, 0x0A],
        ),
    ];

    for (case, bytes) in cases {
        let (router, engine) = emulator_with_s3();
        let bucket = bucket_of("broken-layout");
        let client = s3(&router);
        client
            .create_bucket()
            .bucket(&bucket)
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the bucket must exist, got {error:?}"));
        client
            .put_object()
            .bucket(&bucket)
            .key(format!("{DATA_PREFIX}rows.json"))
            .body(aws_sdk_s3::primitives::ByteStream::from(bytes))
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the object must be stored, got {error:?}"));
        store_table(
            &router,
            "broken_layout",
            "broken_layout",
            &[("id", "int")],
            text_descriptor(
                "broken-layout",
                json!({}),
                json!({ "SerdeInfo": { "SerializationLibrary": JSON_SERDE } }),
            ),
        )
        .await;

        let run_id = start_run(&router, "broken_layout", "broken_layout", json!({})).await;
        set_due_instant(&engine, already_past());
        let run = get_run(&router, &run_id).await;
        assert_eq!(
            run.get("Status").and_then(Value::as_str),
            Some(COLUMN_STATISTICS_TASK_RUN_STATE_FAILED),
            "case {case:?}: a run whose data breaks its layout fails, got {run}"
        );
        assert_eq!(
            run.get("ErrorMessage").and_then(Value::as_str),
            Some(
                column_statistics_task_run_unreadable_data(&location_of("broken-layout")).as_str()
            ),
            "case {case:?}: the run reports that the table's data could not be read, got {run}"
        );
        let answer = get_statistics(&router, "broken_layout", "broken_layout", &["id"]).await;
        assert_eq!(
            statistics_by_column(&answer).len(),
            0,
            "case {case:?}: a failed run leaves no statistics behind, got {answer}"
        );
    }
}
