// SPDX-License-Identifier: CC0-1.0

//! Record format conversion on the Firehose-to-Amazon-S3 data plane, driven over the wire.
//!
//! The rule under test: a destination whose `DataFormatConversionConfiguration` is in force
//! converts the records of each flush from JSON to the columnar format its serializer names,
//! using "the column information from the Amazon Web Services Glue table" its
//! `SchemaConfiguration` binds — so the object delivered under `Prefix` is a columnar file
//! holding one row per record, with one leaf per Glue column. A record that cannot be
//! deserialized into those columns (not one JSON object, or a value a column does not hold)
//! is a FAILED record: it is written under the destination's `ErrorOutputPrefix` ("a prefix
//! that Firehose evaluates and adds to failed records before writing them to S3"), carrying
//! its own bytes, while its well-formed siblings in the same buffer still arrive. A binding
//! the Data Catalog holds no table for has no columns to convert against, so every record of
//! the buffer is such a failed record.
//!
//! Firehose, Amazon S3 and Glue are mounted on ONE shared engine, exactly as the router
//! mounts them, so what is exercised is the hand-off between the three: the Glue table is
//! created through Glue's own wire, the records are put through Firehose's, and the
//! delivered objects are read back through Amazon S3's.

use std::time::{Duration, Instant};

use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::Field;
use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::{PAST_THE_TRANSITION, REGION, ROLE_ARN};
use crate::destination_delivery::{base64, Emulator};

/// How long a delivery is waited for before a case fails: far longer than the emulator's
/// delivery span, so a slow machine does not fail a case the clock would have served.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(10);

/// How often the destination is polled while waiting for a delivery.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The `lastErrorCode` the error output names for a record whose bytes the conversion could
/// not read as JSON at all.
///
/// `pub(crate)`, like the Glue helpers below, so the sibling suites read the error output by
/// the same name rather than re-spelling it.
pub(crate) const PARSE_ERROR: &str = "DataFormatConversion.ParseError";

/// The `lastErrorCode` the error output names for a record the conversion READ but whose
/// values the bound columns do not hold — the class that tells data not matching the schema
/// from unparseable JSON.
pub(crate) const MALFORMED_DATA: &str = "DataFormatConversion.MalformedData";

/// The `lastErrorCode` the error output names when the bound schema could not be read.
const INVALID_SCHEMA: &str = "DataFormatConversion.InvalidSchema";

/// What `!{firehose:error-output-type}` expands to for a record the conversion failed on.
pub(crate) const FAILED_OUTPUT_TYPE: &str = "format-conversion-failed";

/// One record that must convert: the JSON document a producer puts, and the row it must
/// land as — every column rendered as text, so one table serves every column type.
struct ConvertedRecord {
    put: &'static str,
    row: &'static [&'static str],
}

/// One record that must NOT convert: the JSON a producer puts, the `lastErrorCode` its own
/// error-output document must name, and what that document's message must name — the column
/// and the value that could not be deserialized, which is what a consumer of the error prefix
/// diagnoses the record by. An empty list is a failure that is not about one value of the
/// record, so the class's own wording stands.
struct FailedRecord {
    put: &'static str,
    error_code: &'static str,
    fault_mentions: &'static [&'static str],
}

/// One conversion case: the Glue table the destination binds, the deserializer it reads its
/// records with, the records that must convert and the records that must not.
struct ConversionRow {
    case: &'static str,
    /// The table's columns as `(Name, Type)`. The types are Hive's own data type names,
    /// which is what a Glue column's `Type` carries (`Column$Type`).
    columns: &'static [(&'static str, &'static str)],
    /// Whether the bound table is created in the Data Catalog at all. A row that leaves it
    /// out is the binding Firehose can read no column information for.
    catalogued: bool,
    /// The `Deserializer` the destination's `InputFormatConfiguration` names.
    deserializer: Value,
    /// The records that must convert, in put order.
    converted: &'static [ConvertedRecord],
    /// The records that must land under the error output prefix instead.
    failed: &'static [FailedRecord],
}

/// The conversion cases.
///
/// The rows differ along what the rule generalises over: WHICH column types the bound schema
/// declares (every Hive type the conversion writes a leaf for, in one row), HOW a record
/// fails to deserialize into them (a value of the wrong JSON type, a document that is not a
/// JSON object at all, bytes that are not JSON), HOW MANY records of one buffer fail beside
/// the ones that convert, which DESERIALIZER reads the keys (the OpenX one, with and without
/// a column-to-key mapping, and the native Hive one) and whether the binding names a table
/// the catalog HOLDS.
fn conversion_rows() -> Vec<ConversionRow> {
    vec![
        ConversionRow {
            case: "a value whose type the column does not hold",
            columns: &[("id", "string"), ("count", "bigint")],
            catalogued: true,
            deserializer: json!({ "OpenXJsonSerDe": {} }),
            converted: &[
                ConvertedRecord {
                    put: r#"{"id": "a", "count": 1}"#,
                    row: &["a", "1"],
                },
                ConvertedRecord {
                    put: r#"{"id": "b", "count": 2}"#,
                    row: &["b", "2"],
                },
            ],
            failed: &[FailedRecord {
                put: r#"{"id": "c", "count": "many"}"#,
                error_code: MALFORMED_DATA,
                fault_mentions: &["count", "many"],
            }],
        },
        ConversionRow {
            case: "records that are not one JSON object",
            columns: &[("id", "string"), ("count", "bigint")],
            catalogued: true,
            deserializer: json!({ "OpenXJsonSerDe": {} }),
            converted: &[ConvertedRecord {
                put: r#"{"id": "d", "count": 4}"#,
                row: &["d", "4"],
            }],
            failed: &[
                FailedRecord {
                    put: "not json at all",
                    error_code: PARSE_ERROR,
                    fault_mentions: &["JSON"],
                },
                FailedRecord {
                    put: "[1, 2]",
                    error_code: PARSE_ERROR,
                    fault_mentions: &["object"],
                },
            ],
        },
        ConversionRow {
            case: "every column type the schema declares",
            columns: &[
                ("s", "string"),
                ("flag", "boolean"),
                ("tiny", "tinyint"),
                ("small", "smallint"),
                ("whole", "int"),
                ("big", "bigint"),
                ("ratio", "float"),
                ("precise", "double"),
                ("moment", "timestamp"),
                ("day", "date"),
                ("blob", "binary"),
            ],
            catalogued: true,
            deserializer: json!({ "OpenXJsonSerDe": {} }),
            converted: &[
                ConvertedRecord {
                    put: r#"{"s": "x", "flag": true, "tiny": 7, "small": 300,
                             "whole": 70000, "big": 9007199254740993, "ratio": 1.5,
                             "precise": 2.25, "moment": "2024-01-02 03:04:05",
                             "day": "2024-01-02", "blob": "aGVsbG8="}"#,
                    row: &[
                        "x",
                        "true",
                        "7",
                        "300",
                        "70000",
                        "9007199254740993",
                        "1.5",
                        "2.25",
                        "2024-01-02 03:04:05",
                        "2024-01-02",
                        "aGVsbG8=",
                    ],
                },
                // A column the record names no value for holds none: a Glue column is
                // nullable, so an omitted key is not a conversion failure.
                ConvertedRecord {
                    put: r#"{"s": "y"}"#,
                    row: &[
                        "y", "null", "null", "null", "null", "null", "null", "null", "null",
                        "null", "null",
                    ],
                },
            ],
            failed: &[
                FailedRecord {
                    put: r#"{"precise": true}"#,
                    error_code: MALFORMED_DATA,
                    fault_mentions: &["precise", "number"],
                },
                // A magnitude a 32-bit `float` leaf cannot carry is not a value that column
                // holds, exactly as a value outside an integer width's range is not.
                FailedRecord {
                    put: r#"{"ratio": 1e39}"#,
                    error_code: MALFORMED_DATA,
                    fault_mentions: &["ratio", "range"],
                },
            ],
        },
        ConversionRow {
            case: "the OpenX deserializer folds the case and maps a column to its JSON key",
            columns: &[("ts", "string"), ("id", "string")],
            catalogued: true,
            deserializer: json!({
                "OpenXJsonSerDe": { "ColumnToJsonKeyMappings": { "ts": "timestamp" } }
            }),
            converted: &[ConvertedRecord {
                put: r#"{"timestamp": "now", "ID": "e"}"#,
                row: &["now", "e"],
            }],
            failed: &[],
        },
        ConversionRow {
            case: "the native Hive deserializer matches its keys as written",
            columns: &[("id", "string")],
            catalogued: true,
            deserializer: json!({ "HiveJsonSerDe": {} }),
            converted: &[
                ConvertedRecord {
                    put: r#"{"id": "f"}"#,
                    row: &["f"],
                },
                // Matched as written, so a key of another case names no column — which is a
                // row holding no value, not a record that failed to convert.
                ConvertedRecord {
                    put: r#"{"ID": "g"}"#,
                    row: &["null"],
                },
            ],
            failed: &[],
        },
        ConversionRow {
            case: "a binding the Data Catalog holds no table for",
            columns: &[("id", "string")],
            catalogued: false,
            deserializer: json!({ "OpenXJsonSerDe": {} }),
            converted: &[],
            failed: &[FailedRecord {
                put: r#"{"id": "h"}"#,
                // The schema itself could not be read, so nothing about this record's own
                // values is what failed.
                error_code: INVALID_SCHEMA,
                fault_mentions: &[],
            }],
        },
    ]
}

#[tokio::test]
async fn a_converted_destination_writes_its_rows_and_diverts_what_it_cannot_convert() {
    let emulator = Emulator::new();
    let bucket = "format-conversion-bucket";
    emulator.create_bucket(bucket).await;
    let database = "conversion_db";
    create_database(&emulator, database).await;

    let rows = conversion_rows();
    let mut streams: Vec<(String, String)> = Vec::new();
    for (ordinal, row) in rows.iter().enumerate() {
        let stream = format!("conversion-{ordinal}");
        let table = format!("events_{ordinal}");
        if row.catalogued {
            create_table(&emulator, database, &table, row.columns).await;
        }
        emulator
            .firehose_ok(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": stream,
                    "DeliveryStreamType": "DirectPut",
                    "ExtendedS3DestinationConfiguration": {
                        "RoleARN": ROLE_ARN,
                        "BucketARN": format!("arn:aws:s3:::{bucket}"),
                        "Prefix": format!("{stream}-main/"),
                        "ErrorOutputPrefix":
                            format!("{stream}-err/!{{firehose:error-output-type}}/"),
                        "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
                        "DataFormatConversionConfiguration": {
                            "Enabled": true,
                            "SchemaConfiguration": {
                                "RoleARN": ROLE_ARN,
                                "DatabaseName": database,
                                "TableName": table,
                                "Region": REGION,
                            },
                            "InputFormatConfiguration": {
                                "Deserializer": row.deserializer.clone(),
                            },
                            "OutputFormatConfiguration": {
                                "Serializer": { "ParquetSerDe": {} },
                            },
                        },
                    },
                }),
            )
            .await;
        streams.push((stream, table));
    }
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    // One put per case, carrying the records that convert and the records that do not in
    // the SAME batch: what the error output takes must not keep its siblings from arriving.
    for ((stream, _), row) in streams.iter().zip(&rows) {
        let records: Vec<Vec<u8>> = row
            .converted
            .iter()
            .map(|record| record.put.as_bytes().to_vec())
            .chain(
                row.failed
                    .iter()
                    .map(|record| record.put.as_bytes().to_vec()),
            )
            .collect();
        let (status, answered) = emulator.put_batch(stream, &records).await;
        assert_eq!(
            status, 200,
            "case {:?}: the batch must be accepted, got {answered:?}",
            row.case
        );
    }

    for ((stream, table), row) in streams.iter().zip(&rows) {
        let case = row.case;
        let main_prefix = format!("{stream}-main/");
        let error_prefix = format!("{stream}-err/");

        if row.converted.is_empty() {
            // Nothing converted, so the only object of this flush is the error output.
            let error_keys = delivered(&emulator, case, bucket, &error_prefix, 1).await;
            assert_eq!(
                error_keys.len(),
                1,
                "case {case:?}: expected one error-output object, got {error_keys:?}"
            );
            let delivered_keys = emulator.keys_under(bucket, &main_prefix).await;
            assert!(
                delivered_keys.is_empty(),
                "case {case:?}: expected nothing under the main prefix, got {delivered_keys:?}"
            );
        } else {
            let keys = delivered(&emulator, case, bucket, &main_prefix, 1).await;
            assert_eq!(
                keys.len(),
                1,
                "case {case:?}: expected one converted object, got {keys:?}"
            );
            let body = emulator.object(bucket, &keys[0]).await;
            assert_converted(case, &body, row);
        }

        if row.failed.is_empty() {
            let error_keys = emulator.keys_under(bucket, &error_prefix).await;
            assert!(
                error_keys.is_empty(),
                "case {case:?}: expected no error output, got {error_keys:?}"
            );
            continue;
        }
        let error_keys = delivered(&emulator, case, bucket, &error_prefix, 1).await;
        assert_eq!(
            error_keys.len(),
            1,
            "case {case:?}: expected one error-output object, got {error_keys:?}"
        );
        let expected_prefix = format!("{error_prefix}{FAILED_OUTPUT_TYPE}/");
        assert!(
            error_keys[0].starts_with(&expected_prefix),
            "case {case:?}: expected the error output under {expected_prefix:?}, got {:?}",
            error_keys[0]
        );
        let documents = emulator.object(bucket, &error_keys[0]).await;
        assert_error_output(case, &documents, row, database, table);
    }
}

/// The Parquet object a case delivered holds the row of every record that converted, under
/// the Glue table's own columns, and nothing of the records that did not.
fn assert_converted(case: &str, body: &[u8], row: &ConversionRow) {
    let reader = SerializedFileReader::new(bytes::Bytes::copy_from_slice(body))
        .unwrap_or_else(|error| panic!("case {case:?}: the object must be Parquet: {error}"));
    let schema = reader.metadata().file_metadata().schema_descr();
    let columns: Vec<String> = (0..schema.num_columns())
        .map(|index| schema.column(index).name().to_string())
        .collect();
    let expected_columns: Vec<String> = row
        .columns
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect();
    assert_eq!(
        columns, expected_columns,
        "case {case:?}: expected the Glue table's own columns, got {columns:?}"
    );
    let written = reader.metadata().file_metadata().num_rows();
    assert_eq!(
        written,
        row.converted.len() as i64,
        "case {case:?}: expected one row per converted record, got {written}"
    );
    let read: Vec<Vec<String>> = reader
        .get_row_iter(None)
        .unwrap_or_else(|error| panic!("case {case:?}: the rows must read: {error}"))
        .filter_map(Result::ok)
        .map(|row| {
            row.get_column_iter()
                .map(|(_, field)| rendered(field))
                .collect()
        })
        .collect();
    let expected: Vec<Vec<String>> = row
        .converted
        .iter()
        .map(|record| record.row.iter().map(|value| (*value).to_string()).collect())
        .collect();
    assert_eq!(
        read, expected,
        "case {case:?}: expected the converted rows {expected:?}, got {read:?}"
    );
}

/// The error-output object a case delivered holds one document per failed record, naming how
/// the conversion failed, which schema it was measured against and the record's own bytes.
fn assert_error_output(
    case: &str,
    body: &[u8],
    row: &ConversionRow,
    database: &str,
    table: &str,
) {
    let documents: Vec<Value> = String::from_utf8_lossy(body)
        .lines()
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|error| {
                panic!("case {case:?}: an error-output document must be JSON: {error}")
            })
        })
        .collect();
    assert_eq!(
        documents.len(),
        row.failed.len(),
        "case {case:?}: expected one document per failed record, got {documents:?}"
    );
    for (document, failed) in documents.iter().zip(row.failed) {
        // The conversion's own error output names the LAST attempt's failure, beside the
        // Data Catalog table the record was measured against.
        assert_eq!(
            document["lastErrorCode"].as_str(),
            Some(failed.error_code),
            "case {case:?}: expected the error code {:?}, got {document:?}",
            failed.error_code
        );
        let message = document["lastErrorMessage"].as_str().unwrap_or_default();
        assert!(
            !message.is_empty(),
            "case {case:?}: expected a message beside the error code, got {document:?}"
        );
        // What failed for THIS record: the message names the column and the value that
        // could not be deserialized, not one wording for every failed record.
        for mentioned in failed.fault_mentions {
            assert!(
                message.contains(mentioned),
                "case {case:?}: expected the message to name {mentioned:?}, got {message:?}"
            );
        }
        assert_eq!(
            document["rawData"].as_str(),
            Some(base64(failed.put.as_bytes()).as_str()),
            "case {case:?}: expected the record's own bytes in the error output, got \
             {document:?}"
        );
        // WHICH schema the record did not match: the document names the bound Data Catalog
        // table, so a consumer of the error prefix can diagnose the failure.
        assert_eq!(
            document["dataCatalogTable"]["tableName"].as_str(),
            Some(table),
            "case {case:?}: expected the bound table {table:?} in the error output, got \
             {document:?}"
        );
        assert_eq!(
            document["dataCatalogTable"]["databaseName"].as_str(),
            Some(database),
            "case {case:?}: expected the bound database {database:?} in the error output, \
             got {document:?}"
        );
        // The binding names no VersionId, so the table was read at the version the member
        // documents as its default.
        assert_eq!(
            document["dataCatalogTable"]["versionId"].as_str(),
            Some("LATEST"),
            "case {case:?}: expected the default table version in the error output, got \
             {document:?}"
        );
    }
}

/// One Parquet field as the text the case table names it by.
///
/// Each arm is the Field variant the LEAF's own type reads back as, so this also pins the
/// leaf a Hive column type is written as: a `tinyint` column reads back as a byte and a
/// `date` column as a date, not as whatever number carried it.
fn rendered(field: &Field) -> String {
    match field {
        Field::Null => "null".to_string(),
        Field::Bool(value) => value.to_string(),
        Field::Byte(value) => value.to_string(),
        Field::Short(value) => value.to_string(),
        Field::Int(value) => value.to_string(),
        Field::Long(value) => value.to_string(),
        Field::Float(value) => value.to_string(),
        Field::Double(value) => value.to_string(),
        Field::Str(value) => value.clone(),
        Field::Bytes(value) => base64(value.data()),
        Field::Date(days) => {
            let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).expect("the epoch is a date");
            (epoch + chrono::Duration::days(i64::from(*days))).to_string()
        }
        Field::TimestampMillis(millis) => {
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(*millis)
                .map(|stamp| stamp.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| millis.to_string())
        }
        other => panic!("a converted leaf must be one of the types above, got {other:?}"),
    }
}

/// Every key under `prefix` once at least `want` of them are there, or a failure naming
/// `case` once [`DELIVERY_DEADLINE`] has passed.
pub(crate) async fn delivered(
    emulator: &Emulator,
    case: &str,
    bucket: &str,
    prefix: &str,
    want: usize,
) -> Vec<String> {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let keys = emulator.keys_under(bucket, prefix).await;
        if keys.len() >= want {
            return keys;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected at least {want} object(s) under {prefix:?}, got {keys:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Create the Glue database the bound tables live in, through Glue's own wire.
pub(crate) async fn create_database(emulator: &Emulator, database: &str) {
    let (status, answered) = emulator
        .json_rpc(
            "glue",
            "AWSGlue",
            "CreateDatabase",
            json!({ "DatabaseInput": { "Name": database } }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the Glue database {database} must be created, got {answered:?}"
    );
}

/// Create the Glue table whose columns the conversion reads, through Glue's own wire.
pub(crate) async fn create_table(
    emulator: &Emulator,
    database: &str,
    table: &str,
    columns: &[(&str, &str)],
) {
    write_table(emulator, "CreateTable", database, table, columns).await;
}

/// Rewrite that table's definition, which archives the version it replaces: the version a
/// `SchemaConfiguration` can then pin.
async fn update_table(
    emulator: &Emulator,
    database: &str,
    table: &str,
    columns: &[(&str, &str)],
) {
    write_table(emulator, "UpdateTable", database, table, columns).await;
}

/// One write of a table's definition through Glue's own wire.
async fn write_table(
    emulator: &Emulator,
    operation: &str,
    database: &str,
    table: &str,
    columns: &[(&str, &str)],
) {
    let declared: Vec<Value> = columns
        .iter()
        .map(|(name, declared)| json!({ "Name": name, "Type": declared }))
        .collect();
    let (status, answered) = emulator
        .json_rpc(
            "glue",
            "AWSGlue",
            operation,
            json!({
                "DatabaseName": database,
                "TableInput": {
                    "Name": table,
                    "StorageDescriptor": { "Columns": declared },
                },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "{operation} on the Glue table {table} must succeed, got {answered:?}"
    );
}

/// One table-version case: the `VersionId` the binding pins, and the columns the delivered
/// object must carry — `None` where the catalog holds no such version, so every record of
/// the buffer is a failed record instead.
struct VersionRow {
    case: &'static str,
    pinned: Option<&'static str>,
    columns: Option<&'static [&'static str]>,
}

/// The table-version cases.
///
/// The rows differ along which version the binding names: none at all and `LATEST` (the two
/// forms the member documents as the most recent version), the archived version the create
/// made, the live version named explicitly, and a version the catalog does not hold.
fn version_rows() -> Vec<VersionRow> {
    vec![
        VersionRow {
            case: "no version named reads the most recent",
            pinned: None,
            columns: Some(&["id", "count"]),
        },
        VersionRow {
            case: "LATEST reads the most recent",
            pinned: Some("LATEST"),
            columns: Some(&["id", "count"]),
        },
        VersionRow {
            case: "the archived version reads the columns it declared",
            pinned: Some("0"),
            columns: Some(&["id"]),
        },
        VersionRow {
            case: "the live version, named explicitly",
            pinned: Some("1"),
            columns: Some(&["id", "count"]),
        },
        VersionRow {
            case: "a version the catalog does not hold",
            pinned: Some("7"),
            columns: None,
        },
    ]
}

/// RULE: the columns the records are converted against are the bound table's AT THE VERSION
/// the binding pins — "If you don't specify this version ID, or if you set it to `LATEST`,
/// Firehose uses the most recent version" (`SchemaConfiguration$VersionId`) — and a version
/// the catalog does not hold is a schema the conversion cannot be made against, so every
/// record of the buffer lands under the error output prefix.
#[tokio::test]
async fn the_pinned_table_version_is_the_schema_the_records_convert_against() {
    let emulator = Emulator::new();
    let bucket = "format-conversion-version-bucket";
    emulator.create_bucket(bucket).await;
    let database = "conversion_versions";
    create_database(&emulator, database).await;
    let table = "events";
    // The create's version declares one column; the update archives it and makes a version
    // declaring two, so the two versions differ in what a converted row can carry.
    create_table(&emulator, database, table, &[("id", "string")]).await;
    update_table(
        &emulator,
        database,
        table,
        &[("id", "string"), ("count", "bigint")],
    )
    .await;

    let rows = version_rows();
    let mut streams: Vec<String> = Vec::new();
    for (ordinal, row) in rows.iter().enumerate() {
        let stream = format!("version-{ordinal}");
        let mut schema = json!({
            "RoleARN": ROLE_ARN,
            "DatabaseName": database,
            "TableName": table,
            "Region": REGION,
        });
        if let Some(pinned) = row.pinned {
            schema["VersionId"] = json!(pinned);
        }
        emulator
            .firehose_ok(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": stream,
                    "DeliveryStreamType": "DirectPut",
                    "ExtendedS3DestinationConfiguration": {
                        "RoleARN": ROLE_ARN,
                        "BucketARN": format!("arn:aws:s3:::{bucket}"),
                        "Prefix": format!("{stream}-main/"),
                        "ErrorOutputPrefix":
                            format!("{stream}-err/!{{firehose:error-output-type}}/"),
                        "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
                        "DataFormatConversionConfiguration": {
                            "Enabled": true,
                            "SchemaConfiguration": schema,
                            "InputFormatConfiguration": {
                                "Deserializer": { "OpenXJsonSerDe": {} },
                            },
                            "OutputFormatConfiguration": {
                                "Serializer": { "ParquetSerDe": {} },
                            },
                        },
                    },
                }),
            )
            .await;
        streams.push(stream);
    }
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    // The same record into every stream: what differs between the cases is the schema it is
    // converted against, not the data.
    for stream in &streams {
        let (status, answered) = emulator
            .put_batch(stream, &[br#"{"id": "a", "count": 5}"#.to_vec()])
            .await;
        assert_eq!(
            status, 200,
            "the batch into {stream} must be accepted, got {answered:?}"
        );
    }

    for (stream, row) in streams.iter().zip(&rows) {
        let case = row.case;
        let main_prefix = format!("{stream}-main/");
        let error_prefix = format!("{stream}-err/");
        let Some(expected) = row.columns else {
            let error_keys = delivered(&emulator, case, bucket, &error_prefix, 1).await;
            let documents = emulator.object(bucket, &error_keys[0]).await;
            let document: Value = serde_json::from_slice(&documents)
                .unwrap_or_else(|error| panic!("case {case:?}: the document must be JSON: {error}"));
            assert_eq!(
                document["lastErrorCode"].as_str(),
                Some(INVALID_SCHEMA),
                "case {case:?}: expected the schema failure, got {document:?}"
            );
            let delivered_keys = emulator.keys_under(bucket, &main_prefix).await;
            assert!(
                delivered_keys.is_empty(),
                "case {case:?}: expected nothing under the main prefix, got {delivered_keys:?}"
            );
            continue;
        };
        let keys = delivered(&emulator, case, bucket, &main_prefix, 1).await;
        let body = emulator.object(bucket, &keys[0]).await;
        let read = converted_columns(case, &body);
        let expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            read, expected,
            "case {case:?}: expected the columns {expected:?} of the pinned version, got \
             {read:?}"
        );
    }
}

/// The columns of the Parquet object delivered for a case, in file order.
fn converted_columns(case: &str, body: &[u8]) -> Vec<String> {
    let reader = SerializedFileReader::new(bytes::Bytes::copy_from_slice(body))
        .unwrap_or_else(|error| panic!("case {case:?}: the object must be Parquet: {error}"));
    let schema = reader.metadata().file_metadata().schema_descr();
    (0..schema.num_columns())
        .map(|index| schema.column(index).name().to_string())
        .collect()
}
