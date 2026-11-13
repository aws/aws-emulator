// SPDX-License-Identifier: CC0-1.0

//! Conversion of a delivery stream's records against a Glue column whose declared type
//! NESTS, driven over the wire.
//!
//! The rule under test: a destination with `DataFormatConversionConfiguration` in force
//! converts each record against "the column information from the Amazon Web Services Glue
//! table" its `SchemaConfiguration` binds, WHATEVER Hive data type each column declares —
//! including the complex ones, `array<…>`, `struct<…>` and `map<…,…>`, which a Glue column's
//! `Type` carries exactly as it carries a scalar — so the object under `Prefix` is a columnar
//! file holding the whole tree of every record that converted. A record whose value at any
//! point of that tree is not one the column there holds does NOT convert: it is written under
//! the destination's `ErrorOutputPrefix` ("a prefix that Firehose evaluates and adds to failed
//! records before writing them to S3") with the conversion error code for data that does not
//! match the schema, which is NOT the code of a record whose bytes are not parseable JSON at
//! all — and the failed records of one flush share one error-output object, one document per
//! line, each under its own code. The mismatched record's own bytes never reach the converted
//! object.
//!
//! Firehose, Amazon S3 and Glue are mounted on ONE shared engine, exactly as the router mounts
//! them: the Glue table is created through Glue's own wire, the records are put through
//! Firehose's, and the delivered objects are read back through Amazon S3's.

use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::Field;
use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::{REGION, ROLE_ARN};
use crate::destination_delivery::{base64, contains, Emulator};
use crate::format_conversion_delivery::{
    create_database, create_table, delivered, FAILED_OUTPUT_TYPE, MALFORMED_DATA, PARSE_ERROR,
};
use crate::parquet_serializer_parameters::await_active;

/// The magic a Parquet file begins and ends with, which is what makes the delivered bytes a
/// file a Parquet reader reads rather than the records' own JSON.
const PARQUET_MAGIC: &[u8] = b"PAR1";

/// What must become of one record a case puts.
enum Expected {
    /// The record converts, and its row reads back as this — one entry per column, each
    /// rendered by [`rendered`].
    Row(&'static [&'static str]),
    /// The record does not convert: it is diverted under the error output prefix.
    Diverted {
        /// The conversion error code its own error-output document must name.
        error_code: &'static str,
        /// A word unique to this record, which must appear NOWHERE in the converted object.
        marker: &'static str,
        /// What the document's message must name: where in the column's tree the record
        /// failed.
        mentions: &'static [&'static str],
    },
}

/// One record a case puts, with what must become of it.
struct PutRecord {
    put: &'static str,
    expected: Expected,
}

/// One case: the Glue table's columns and the records put against them.
struct NestedRow {
    case: &'static str,
    /// The table's columns as `(Name, Type)`. The types are Hive's own data types, which is
    /// what a Glue column's `Type` carries (`Column$Type`: "the data type of the Column") —
    /// `array<…>`, `struct<…>` and `map<…,…>` among them.
    columns: &'static [(&'static str, &'static str)],
    records: &'static [PutRecord],
}

/// The cases.
///
/// The rows differ along what the rule generalises over: WHICH complex Hive type the column
/// declares (a list of structs, a map, a struct within a struct, a list of lists, a list of
/// scalars), HOW MANY entries a record carries in it (several, one, none at all, and a null
/// entry among them), and WHERE in the tree a record fails to deserialize (a scalar deep
/// inside a list of structs, a map's value, a struct where a scalar stands, a list where a
/// scalar stands) beside a record that is not parseable JSON at all.
fn nested_rows() -> Vec<NestedRow> {
    vec![
        NestedRow {
            case: "a list of structs",
            columns: &[
                ("marker", "string"),
                ("n", "int"),
                ("tags", "array<struct<sku:string,qty:int>>"),
            ],
            records: &[
                PutRecord {
                    put: r#"{"marker": "conforming", "n": 1,
                             "tags": [{"sku": "a", "qty": 1}, {"sku": "b", "qty": 2}]}"#,
                    expected: Expected::Row(&[
                        "conforming",
                        "1",
                        "[{sku=a,qty=1},{sku=b,qty=2}]",
                    ]),
                },
                // A list a record carries NO entries in is not an absent one, and a struct
                // field it names no value for holds none: both are rows, not failures.
                PutRecord {
                    put: r#"{"marker": "empty", "n": 2, "tags": []}"#,
                    expected: Expected::Row(&["empty", "2", "[]"]),
                },
                PutRecord {
                    put: r#"{"marker": "absent", "n": 3}"#,
                    expected: Expected::Row(&["absent", "3", "null"]),
                },
                PutRecord {
                    put: r#"{"marker": "partial", "n": 4, "tags": [{"sku": "c"}]}"#,
                    expected: Expected::Row(&["partial", "4", "[{sku=c,qty=null}]"]),
                },
                // The value a column declares an `int` for, deep inside the list, is not an
                // integer: the record does not match the schema.
                PutRecord {
                    put: r#"{"marker": "pqmismatch-nested", "n": 5,
                             "tags": [{"sku": "d", "qty": "not-an-int"}]}"#,
                    expected: Expected::Diverted {
                        error_code: MALFORMED_DATA,
                        marker: "pqmismatch-nested",
                        mentions: &["tags", "element 0", "qty"],
                    },
                },
                // Bytes that are not JSON at all are the OTHER failure class.
                PutRecord {
                    put: "not-json-at-all pqparse-raw",
                    expected: Expected::Diverted {
                        error_code: PARSE_ERROR,
                        marker: "pqparse-raw",
                        mentions: &["JSON"],
                    },
                },
            ],
        },
        NestedRow {
            case: "a map of integers",
            columns: &[("labels", "map<string,int>")],
            records: &[
                PutRecord {
                    put: r#"{"labels": {"x": 1, "y": 2}}"#,
                    expected: Expected::Row(&["{x=1,y=2}"]),
                },
                PutRecord {
                    put: r#"{"labels": {}}"#,
                    expected: Expected::Row(&["{}"]),
                },
                PutRecord {
                    put: r#"{"labels": {"z": "pqmismatch-map"}}"#,
                    expected: Expected::Diverted {
                        error_code: MALFORMED_DATA,
                        marker: "pqmismatch-map",
                        mentions: &["labels", "z"],
                    },
                },
            ],
        },
        NestedRow {
            case: "a struct within a struct",
            columns: &[("outer", "struct<inner:struct<name:string>,count:bigint>")],
            records: &[
                PutRecord {
                    put: r#"{"outer": {"inner": {"name": "deep"}, "count": 9}}"#,
                    expected: Expected::Row(&["{inner={name=deep},count=9}"]),
                },
                PutRecord {
                    put: r#"{"outer": {"count": 10}}"#,
                    expected: Expected::Row(&["{inner=null,count=10}"]),
                },
                // A struct is not a scalar: a record naming one where the column declares a
                // group of fields does not match the schema.
                PutRecord {
                    put: r#"{"outer": {"inner": "pqmismatch-struct", "count": 11}}"#,
                    expected: Expected::Diverted {
                        error_code: MALFORMED_DATA,
                        marker: "pqmismatch-struct",
                        mentions: &["outer", "inner", "struct"],
                    },
                },
            ],
        },
        NestedRow {
            case: "a list of lists",
            columns: &[("rows", "array<array<bigint>>")],
            records: &[
                PutRecord {
                    put: r#"{"rows": [[1, 2], [3], []]}"#,
                    expected: Expected::Row(&["[[1,2],[3],[]]"]),
                },
                PutRecord {
                    put: r#"{"rows": [1, 2]}"#,
                    expected: Expected::Diverted {
                        error_code: MALFORMED_DATA,
                        marker: r#"{"rows": [1, 2]}"#,
                        mentions: &["rows", "element 0", "array"],
                    },
                },
            ],
        },
        NestedRow {
            case: "a list of scalars holding a null entry",
            columns: &[("names", "array<string>")],
            records: &[PutRecord {
                put: r#"{"names": ["a", null, "b"]}"#,
                expected: Expected::Row(&["[a,null,b]"]),
            }],
        },
    ]
}

/// A column that NESTS converts against its whole tree, and a record whose value anywhere in
/// that tree the column does not hold is diverted under the error output prefix with the
/// malformed-data code — beside, and told apart from, a record that is not parseable JSON.
#[tokio::test]
async fn a_nesting_column_converts_its_tree_and_diverts_what_does_not_match_it() {
    let emulator = Emulator::new();
    let bucket = "nested-conversion-bucket";
    emulator.create_bucket(bucket).await;
    let database = "nested_db";
    create_database(&emulator, database).await;

    let rows = nested_rows();
    let mut streams: Vec<(String, String)> = Vec::new();
    for (ordinal, row) in rows.iter().enumerate() {
        let stream = format!("nested-{ordinal}");
        let table = format!("nested_events_{ordinal}");
        create_table(&emulator, database, &table, row.columns).await;
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
                        "CompressionFormat": "UNCOMPRESSED",
                        "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
                        "DataFormatConversionConfiguration": {
                            "Enabled": true,
                            "SchemaConfiguration": {
                                "RoleARN": ROLE_ARN,
                                "DatabaseName": database,
                                "TableName": table,
                                "Region": REGION,
                                "VersionId": "LATEST",
                            },
                            "InputFormatConfiguration": {
                                "Deserializer": { "OpenXJsonSerDe": {} },
                            },
                            "OutputFormatConfiguration": {
                                "Serializer": { "ParquetSerDe": { "Compression": "UNCOMPRESSED" } },
                            },
                        },
                    },
                }),
            )
            .await;
        streams.push((stream, table));
    }
    // Waited for, not slept through: a put is accepted only once the stream is ACTIVE, and
    // how long the clock takes to get it there is not what this suite is about.
    for ((stream, _), row) in streams.iter().zip(&rows) {
        await_active(&emulator, row.case, stream).await;
    }

    // One put per case, carrying the records that convert and the records that do not in the
    // SAME batch: what the error output takes must not keep its siblings from arriving.
    for ((stream, _), row) in streams.iter().zip(&rows) {
        let records: Vec<Vec<u8>> = row
            .records
            .iter()
            .map(|record| record.put.as_bytes().to_vec())
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

        let keys = delivered(&emulator, case, bucket, &main_prefix, 1).await;
        assert_eq!(
            keys.len(),
            1,
            "case {case:?}: expected one converted object, got {keys:?}"
        );
        let body = emulator.object(bucket, &keys[0]).await;
        assert_converted(case, &body, row);

        let diverted: Vec<&PutRecord> = row
            .records
            .iter()
            .filter(|record| matches!(record.expected, Expected::Diverted { .. }))
            .collect();
        if diverted.is_empty() {
            let error_keys = emulator.keys_under(bucket, &error_prefix).await;
            assert!(
                error_keys.is_empty(),
                "case {case:?}: expected no error output, got {error_keys:?}"
            );
            continue;
        }
        // ONE object for every failed record of the flush, under the error output type the
        // prefix's own namespace expands to.
        let error_keys = delivered(&emulator, case, bucket, &error_prefix, 1).await;
        assert_eq!(
            error_keys.len(),
            1,
            "case {case:?}: expected one error-output object for the whole flush, got \
             {error_keys:?}"
        );
        let expected_prefix = format!("{error_prefix}{FAILED_OUTPUT_TYPE}/");
        assert!(
            error_keys[0].starts_with(&expected_prefix),
            "case {case:?}: expected the error output under {expected_prefix:?}, got {:?}",
            error_keys[0]
        );
        let documents = emulator.object(bucket, &error_keys[0]).await;
        assert_diverted(case, &documents, &diverted, database, table);
    }
}

/// The object delivered under the main prefix is a Parquet file holding the row of every
/// record that converted — the whole tree of each one — and nothing of the records that did
/// not.
fn assert_converted(case: &str, body: &[u8], row: &NestedRow) {
    assert!(
        body.starts_with(PARQUET_MAGIC) && body.ends_with(PARQUET_MAGIC),
        "case {case:?}: expected a Parquet file, got {:?}..{:?}",
        String::from_utf8_lossy(&body[..body.len().min(8)]),
        String::from_utf8_lossy(&body[body.len().saturating_sub(8)..])
    );
    for record in row.records {
        if let Expected::Diverted { marker, .. } = record.expected {
            assert!(
                !contains(body, marker.as_bytes()),
                "case {case:?}: expected nothing of the diverted record {marker:?} in the \
                 converted object"
            );
        }
    }
    let reader = SerializedFileReader::new(bytes::Bytes::copy_from_slice(body))
        .unwrap_or_else(|error| panic!("case {case:?}: the object must be Parquet: {error}"));
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
        .records
        .iter()
        .filter_map(|record| match record.expected {
            Expected::Row(cells) => {
                Some(cells.iter().map(|cell| (*cell).to_string()).collect())
            }
            Expected::Diverted { .. } => None,
        })
        .collect();
    assert_eq!(
        read, expected,
        "case {case:?}: expected the converted rows {expected:?}, got {read:?}"
    );
}

/// The error-output object holds one document per diverted record, each under ITS OWN
/// conversion error code, with the record's own bytes and the table it was measured against.
fn assert_diverted(
    case: &str,
    body: &[u8],
    diverted: &[&PutRecord],
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
        diverted.len(),
        "case {case:?}: expected one document per diverted record, got {documents:?}"
    );
    for (document, record) in documents.iter().zip(diverted) {
        let Expected::Diverted {
            error_code,
            marker,
            mentions,
        } = record.expected
        else {
            continue;
        };
        assert_eq!(
            document["lastErrorCode"].as_str(),
            Some(error_code),
            "case {case:?}: expected the record {marker:?} under {error_code:?}, got {document:?}"
        );
        let message = document["lastErrorMessage"].as_str().unwrap_or_default();
        for mentioned in mentions {
            assert!(
                message.contains(mentioned),
                "case {case:?}: expected the message of {marker:?} to name {mentioned:?}, got \
                 {message:?}"
            );
        }
        assert_eq!(
            document["rawData"].as_str(),
            Some(base64(record.put.as_bytes()).as_str()),
            "case {case:?}: expected the record's own bytes in the error output, got {document:?}"
        );
        assert_eq!(
            document["dataCatalogTable"]["tableName"].as_str(),
            Some(table),
            "case {case:?}: expected the bound table {table:?} in the error output, got \
             {document:?}"
        );
        assert_eq!(
            document["dataCatalogTable"]["databaseName"].as_str(),
            Some(database),
            "case {case:?}: expected the bound database {database:?} in the error output, got \
             {document:?}"
        );
    }
}

/// One Parquet field as the text a case names it by, a group, list or map rendered as the tree
/// it is: each arm is the variant the LEAF's or the GROUP's own annotation reads back as, so
/// this also pins that a Hive `array<…>` is written as a LIST and a `map<…,…>` as a MAP rather
/// than as some group a reader cannot place.
fn rendered(field: &Field) -> String {
    match field {
        Field::Null => "null".to_string(),
        Field::Bool(value) => value.to_string(),
        Field::Int(value) => value.to_string(),
        Field::Long(value) => value.to_string(),
        Field::Str(value) => value.clone(),
        Field::Group(row) => format!(
            "{{{}}}",
            row.get_column_iter()
                .map(|(name, field)| format!("{name}={}", rendered(field)))
                .collect::<Vec<String>>()
                .join(",")
        ),
        Field::ListInternal(list) => format!(
            "[{}]",
            list.elements()
                .iter()
                .map(rendered)
                .collect::<Vec<String>>()
                .join(",")
        ),
        Field::MapInternal(map) => format!(
            "{{{}}}",
            map.entries()
                .iter()
                .map(|(key, value)| format!("{}={}", rendered(key), rendered(value)))
                .collect::<Vec<String>>()
                .join(",")
        ),
        other => panic!("a converted leaf must be one of the types above, got {other:?}"),
    }
}

