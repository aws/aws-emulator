// SPDX-License-Identifier: CC0-1.0

//! What a converted extended Amazon S3 destination DELIVERS, over every writer parameter its
//! `ParquetSerDe` carries.
//!
//! The rule under test: where a destination's `DataFormatConversionConfiguration` is in
//! force with a `ParquetSerDe` serializer, the object delivered under `Prefix` is a Parquet
//! FILE and not the records' own bytes — the format frames a file between the four magic
//! bytes `PAR1` at each end, so those are the object's first and last four bytes and a
//! Parquet reader reads it — holding one row per record that deserialized into the bound
//! Glue table's columns, whatever writer parameters the serializer names. A record that did
//! NOT deserialize into those columns is written under the destination's `ErrorOutputPrefix`
//! instead of landing in that file, carrying its own bytes and the table it was measured
//! against.
//!
//! The rows differ along what the rule generalises over: the serializer's own parameters —
//! each compression code, each row format version, the page size and the HDFS block size at
//! their published minimums, whether dictionary compression is enabled and what padding is
//! allowed — and how many conforming records share the flush with the one that does not.
//! The parameter names, their value sets and their bounds are the model's
//! (`smithy_query(query="shape", shape="ParquetSerDe,ParquetCompression,
//! ParquetWriterVersion,ParquetPageSizeBytes")`): `Compression` is `UNCOMPRESSED`, `GZIP` or
//! `SNAPPY`, `WriterVersion` is `V1` or `V2`, `PageSizeBytes` is at least 65536,
//! `BlockSizeBytes` at least 67108864 and `MaxPaddingBytes` at least 0. A value one step
//! past a published bound is refused at the request boundary, so no stream is created for it
//! at all and nothing is delivered.
//!
//! Firehose, Amazon S3 and Glue are mounted on ONE shared engine, exactly as the router
//! mounts them: the Glue table is created through Glue's own wire, the records are put one at
//! a time through Firehose's `PutRecord`, and the delivered objects are read back through
//! Amazon S3's.

use std::time::{Duration, Instant};

use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::Field;
use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::{at, error_code, error_message, REGION, ROLE_ARN};
use crate::destination_delivery::{base64, Emulator};
use crate::format_conversion_delivery::{
    create_database, create_table, delivered, FAILED_OUTPUT_TYPE, MALFORMED_DATA,
};
use crate::refusal_messages::value_at;

/// The four bytes the Parquet format frames a file between, at each end of it, which is what
/// a reader of the destination prefix identifies the object by.
const PARQUET_MAGIC: &[u8] = b"PAR1";

/// The least `ParquetSerDe$PageSizeBytes` the model admits: "The minimum value is 64 KiB"
/// (`ParquetPageSizeBytes` `@range min`).
const MIN_PAGE_SIZE_BYTES: i64 = 65_536;

/// The least `ParquetSerDe$BlockSizeBytes` the model admits (`BlockSizeBytes` `@range min`,
/// 64 MiB).
const MIN_BLOCK_SIZE_BYTES: i64 = 67_108_864;

/// How long a stream is waited on to reach `ACTIVE` before a case fails: far longer than the
/// emulator's own creation span, so a loaded machine does not fail a case the clock would
/// have served.
const ACTIVE_DEADLINE: Duration = Duration::from_secs(30);

/// How often the stream is polled while waiting for that status.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The `BufferingHints$IntervalInSeconds` every case's destination asks for: long enough that
/// one stream's records share ONE buffer however slowly the host makes the puts, and well
/// inside the deadline the delivery is then waited for with.
const BUFFER_INTERVAL_SECONDS: i64 = 2;

/// The bound Glue table's columns, as `(Name, Type)` with Hive's own data type names, which
/// is what a Glue column's `Type` carries (`Column$Type`). Two columns of different types, so
/// one record can conform in one column and contradict the other.
const COLUMNS: &[(&str, &str)] = &[("id", "string"), ("count", "bigint")];

/// The record that cannot be deserialized into [`COLUMNS`]: its `count` carries text, which
/// the `bigint` column does not hold.
const MISMATCHED_RECORD: &str = r#"{"id": "mismatched", "count": "not-a-number"}"#;

/// The member path of the `ParquetSerDe` a refusal of one of its parameters reports under.
const SERIALIZER_PATH: &str = "extendedS3DestinationConfiguration.\
                               dataFormatConversionConfiguration.outputFormatConfiguration.\
                               serializer.parquetSerDe";

/// One case: the `ParquetSerDe` the destination's `OutputFormatConfiguration` names, how many
/// conforming records share the flush with the one that cannot be converted, and whether the
/// create takes the serializer at all.
struct SerializerRow {
    case: &'static str,
    serializer: Value,
    /// How many schema-conforming records are put before the one that is not.
    conforming: usize,
    /// `None` where the serializer is admitted; the parameter's own member name and the
    /// constraint clause where the request boundary refuses it.
    refused: Option<(&'static str, String)>,
}

/// Every case.
fn serializer_rows() -> Vec<SerializerRow> {
    vec![
        SerializerRow {
            case: "no parameter named, so the serializer's own defaults",
            serializer: json!({}),
            conforming: 1,
            refused: None,
        },
        SerializerRow {
            case: "the UNCOMPRESSED compression code",
            serializer: json!({ "Compression": "UNCOMPRESSED" }),
            conforming: 2,
            refused: None,
        },
        SerializerRow {
            case: "the GZIP compression code",
            serializer: json!({ "Compression": "GZIP" }),
            conforming: 3,
            refused: None,
        },
        SerializerRow {
            case: "the SNAPPY compression code named explicitly",
            serializer: json!({ "Compression": "SNAPPY" }),
            conforming: 1,
            refused: None,
        },
        SerializerRow {
            case: "the V2 row format",
            serializer: json!({ "WriterVersion": "V2" }),
            conforming: 4,
            refused: None,
        },
        SerializerRow {
            case: "the V1 row format, the page size at its minimum, no dictionary compression",
            serializer: json!({
                "WriterVersion": "V1",
                "PageSizeBytes": MIN_PAGE_SIZE_BYTES,
                "EnableDictionaryCompression": false,
            }),
            conforming: 2,
            refused: None,
        },
        SerializerRow {
            case: "dictionary compression over a page far above the minimum, with no padding",
            serializer: json!({
                "PageSizeBytes": 1_048_576,
                "EnableDictionaryCompression": true,
                "MaxPaddingBytes": 0,
            }),
            conforming: 3,
            refused: None,
        },
        SerializerRow {
            case: "the block size at its minimum, with GZIP over the V2 row format",
            serializer: json!({
                "BlockSizeBytes": MIN_BLOCK_SIZE_BYTES,
                "Compression": "GZIP",
                "WriterVersion": "V2",
            }),
            conforming: 2,
            refused: None,
        },
        SerializerRow {
            case: "a page size one byte below its published minimum",
            serializer: json!({ "PageSizeBytes": MIN_PAGE_SIZE_BYTES - 1 }),
            conforming: 1,
            refused: Some((
                "pageSizeBytes",
                format!("Member must have value greater than or equal to {MIN_PAGE_SIZE_BYTES}"),
            )),
        },
        SerializerRow {
            case: "a block size one byte below its published minimum",
            serializer: json!({ "BlockSizeBytes": MIN_BLOCK_SIZE_BYTES - 1 }),
            conforming: 1,
            refused: Some((
                "blockSizeBytes",
                format!("Member must have value greater than or equal to {MIN_BLOCK_SIZE_BYTES}"),
            )),
        },
    ]
}

#[tokio::test]
async fn a_converted_destination_delivers_a_parquet_file_whatever_the_serializer_asks_for() {
    let emulator = Emulator::new();
    let bucket = "parquet-serializer-bucket";
    emulator.create_bucket(bucket).await;
    let database = "parquet_serializer_db";
    create_database(&emulator, database).await;

    let rows = serializer_rows();
    // The streams a case was created for: the ones whose serializer the request boundary
    // admitted, each with the Glue table its `SchemaConfiguration` binds.
    let mut live: Vec<(String, String, &SerializerRow)> = Vec::new();
    for (ordinal, row) in rows.iter().enumerate() {
        let case = row.case;
        let stream = format!("parquet-serde-{ordinal}");
        let table = format!("events_{ordinal}");
        create_table(&emulator, database, &table, COLUMNS).await;
        let (status, answered) = emulator
            .firehose(
                "CreateDeliveryStream",
                create_request(&stream, bucket, database, &table, &row.serializer),
            )
            .await;
        if let Some((member, requirement)) = row.refused.as_ref() {
            assert_eq!(
                status, 400,
                "case {case:?}: the create must be refused, got {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "ValidationException",
                "case {case:?}: got {answered:?}"
            );
            let expected = value_at(&format!("{SERIALIZER_PATH}.{member}"), requirement);
            let message = error_message(&answered);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: the create must be accepted, got {answered:?}"
        );
        live.push((stream, table, row));
    }

    // A put is accepted only once the stream is ACTIVE, which is a clock-owed change: wait
    // for the STATUS rather than for a span.
    for (stream, _, row) in &live {
        await_active(&emulator, row.case, stream).await;
    }

    // One flush per case, holding the records that convert and the record that does not: what
    // the error output takes must not keep its siblings out of the converted object. Each
    // record goes in its own `PutRecord`, the way a producer writing one record at a time
    // delivers it.
    for (stream, _, row) in &live {
        let case = row.case;
        for ordinal in 0..row.conforming {
            let record = conforming_record(ordinal);
            let (status, answered) = emulator.put_record(stream, record.as_bytes()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the put of {record:?} must be accepted, got {answered:?}"
            );
        }
        let (status, answered) = emulator
            .put_record(stream, MISMATCHED_RECORD.as_bytes())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the put of the mismatched record must be accepted, got {answered:?}"
        );
    }

    for (stream, table, row) in &live {
        let case = row.case;
        let main_prefix = format!("{stream}-main/");
        let error_prefix = format!("{stream}-err/");

        let keys = delivered(&emulator, case, bucket, &main_prefix, 1).await;
        assert_eq!(
            keys.len(),
            1,
            "case {case:?}: expected one converted object, got {keys:?}"
        );
        let body = emulator.object(bucket, &keys[0]).await;
        assert_parquet_framed(case, &body);
        assert_converted_rows(case, &body, row.conforming);

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
        assert_quarantined(case, &documents, database, table);
    }
}

/// The `CreateDeliveryStream` request for one case: an extended Amazon S3 destination bound to
/// `table`, converting its records with `serializer`, with an error output prefix of its own.
fn create_request(
    stream: &str,
    bucket: &str,
    database: &str,
    table: &str,
    serializer: &Value,
) -> Value {
    json!({
        "DeliveryStreamName": stream,
        "DeliveryStreamType": "DirectPut",
        "ExtendedS3DestinationConfiguration": {
            "RoleARN": ROLE_ARN,
            "BucketARN": format!("arn:aws:s3:::{bucket}"),
            "Prefix": format!("{stream}-main/"),
            "ErrorOutputPrefix": format!("{stream}-err/!{{firehose:error-output-type}}/"),
            "BufferingHints": {
                "SizeInMBs": 1,
                "IntervalInSeconds": BUFFER_INTERVAL_SECONDS,
            },
            "DataFormatConversionConfiguration": {
                "Enabled": true,
                "SchemaConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "DatabaseName": database,
                    "TableName": table,
                    "Region": REGION,
                },
                "InputFormatConfiguration": {
                    "Deserializer": { "OpenXJsonSerDe": {} },
                },
                "OutputFormatConfiguration": {
                    "Serializer": { "ParquetSerDe": serializer.clone() },
                },
            },
        },
    })
}

/// The JSON document the `ordinal`-th conforming record carries: a value for each of
/// [`COLUMNS`], of the type that column holds.
fn conforming_record(ordinal: usize) -> String {
    format!(
        "{{\"id\": \"{}\", \"count\": {ordinal}}}",
        identifier(ordinal)
    )
}

/// The `id` the `ordinal`-th conforming record carries.
fn identifier(ordinal: usize) -> String {
    format!("record-{ordinal}")
}

/// The delivered object is a Parquet FILE: the format's magic bytes frame it at each end.
fn assert_parquet_framed(case: &str, body: &[u8]) {
    assert!(
        body.len() > 2 * PARQUET_MAGIC.len(),
        "case {case:?}: expected a Parquet file, got {} byte(s)",
        body.len()
    );
    let head = &body[..PARQUET_MAGIC.len()];
    assert_eq!(
        head,
        PARQUET_MAGIC,
        "case {case:?}: expected the Parquet magic at the head of the object, got {:?}",
        String::from_utf8_lossy(head)
    );
    let tail = &body[body.len() - PARQUET_MAGIC.len()..];
    assert_eq!(
        tail,
        PARQUET_MAGIC,
        "case {case:?}: expected the Parquet magic at the tail of the object, got {:?}",
        String::from_utf8_lossy(tail)
    );
}

/// That file holds the bound table's columns and one row per converted record, and nothing of
/// the record that could not be converted.
fn assert_converted_rows(case: &str, body: &[u8], conforming: usize) {
    let reader = SerializedFileReader::new(bytes::Bytes::copy_from_slice(body))
        .unwrap_or_else(|error| panic!("case {case:?}: the object must be Parquet: {error}"));
    let schema = reader.metadata().file_metadata().schema_descr();
    let columns: Vec<String> = (0..schema.num_columns())
        .map(|index| schema.column(index).name().to_string())
        .collect();
    let expected_columns: Vec<String> = COLUMNS
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect();
    assert_eq!(
        columns, expected_columns,
        "case {case:?}: expected the Glue table's own columns, got {columns:?}"
    );
    let read: Vec<Vec<String>> = reader
        .get_row_iter(None)
        .unwrap_or_else(|error| panic!("case {case:?}: the rows must read: {error}"))
        .filter_map(Result::ok)
        .map(|row| row.get_column_iter().map(|(_, field)| cell(field)).collect())
        .collect();
    let expected: Vec<Vec<String>> = (0..conforming)
        .map(|ordinal| vec![identifier(ordinal), ordinal.to_string()])
        .collect();
    assert_eq!(
        read, expected,
        "case {case:?}: expected the converted rows {expected:?}, got {read:?}"
    );
}

/// One leaf of a converted row as the text the case compares it by.
fn cell(field: &Field) -> String {
    match field {
        Field::Str(value) => value.clone(),
        Field::Long(value) => value.to_string(),
        other => panic!("a leaf of a string or bigint column must read back as one, got {other:?}"),
    }
}

/// The error-output object holds the record the conversion could not read: how it failed, its
/// own bytes, and the Data Catalog table it was measured against.
fn assert_quarantined(case: &str, body: &[u8], database: &str, table: &str) {
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
        1,
        "case {case:?}: expected one document for the one mismatched record, got {documents:?}"
    );
    let document = &documents[0];
    assert_eq!(
        document["lastErrorCode"].as_str(),
        Some(MALFORMED_DATA),
        "case {case:?}: expected the malformed-data code, got {document:?}"
    );
    assert_eq!(
        document["rawData"].as_str(),
        Some(base64(MISMATCHED_RECORD.as_bytes()).as_str()),
        "case {case:?}: expected the record's own bytes in the error output, got {document:?}"
    );
    assert_eq!(
        document["dataCatalogTable"]["databaseName"].as_str(),
        Some(database),
        "case {case:?}: expected the bound database {database:?}, got {document:?}"
    );
    assert_eq!(
        document["dataCatalogTable"]["tableName"].as_str(),
        Some(table),
        "case {case:?}: expected the bound table {table:?}, got {document:?}"
    );
}

/// Wait for `stream` to be `ACTIVE`, which is when a put is accepted, or fail naming `case`
/// once [`ACTIVE_DEADLINE`] has passed.
///
/// `pub(crate)` so the sibling suite over a nesting schema waits for its streams the same way
/// rather than re-spelling the poll.
pub(crate) async fn await_active(emulator: &Emulator, case: &str, stream: &str) {
    let deadline = Instant::now() + ACTIVE_DEADLINE;
    loop {
        let described = emulator
            .firehose_ok(
                "DescribeDeliveryStream",
                json!({ "DeliveryStreamName": stream }),
            )
            .await;
        let status = at(&described, "DeliveryStreamDescription/DeliveryStreamStatus")
            .as_str()
            .unwrap_or_default()
            .to_string();
        if status == "ACTIVE" {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {stream:?} to become ACTIVE, it is {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
