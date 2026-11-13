// SPDX-License-Identifier: CC0-1.0

//! The error output of the records an HTTP endpoint destination refused, over the FORMS one
//! delivery can take: how the records were put, how many of them one buffer holds, and how
//! the backup location encodes the object they are parked in.
//!
//! The rule under test is the one [`crate::http_endpoint_lambda_error_output`] pins for the
//! answers an endpoint can give, read along the other axis: whatever path the records were
//! accepted on and whatever form the parked object takes, the error-output document of each
//! refused record states the message the destination's own answer REPORTED about the refusal
//! ("only the documents that Firehose could not deliver to the specified HTTP endpoint
//! destination (`FailedDataOnly`)" — `S3BackupMode` — are parked, and a consumer of the
//! `ErrorOutputPrefix` diagnoses a refusal by the destination's own account of it), beside the
//! attempts the buffer really cost over `RetryOptions$DurationInSeconds` ("the total amount of
//! time that Firehose spends on retries") and the record's own bytes.
//!
//! The rows differ along what the rule generalises over:
//!
//! * the put the records arrived on — one `PutRecord` per record, or one `PutRecordBatch`
//!   carrying them all, since "Firehose buffers records before delivering them to the
//!   destination" (`PutRecord`) whichever call accepted them;
//! * how many records one buffer holds (one, two, three), since the destination's account is
//!   copied into the document of EVERY record of the buffer;
//! * the backup's `CompressionFormat` — left unset, `UNCOMPRESSED`, or `GZIP`, which also
//!   appends that format's own file extension to the object's name ("Specify a file
//!   extension. It will override the default file extension" — `FileExtension`) — since the
//!   message has to survive the encoding the object is written in;
//! * what the endpoint's answer reports, and the status it reports it with.
//!
//! The endpoint is a Lambda function URL served by this same emulator, so the POST Firehose
//! renders is the request a handler RUNS on and the answer Firehose reads is that handler's
//! own; the parked objects are read back out of Amazon S3 over its own wire.

use std::io::Read as _;
use std::time::Instant;

use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::ROLE_ARN;
use crate::http_endpoint_lambda_error_output::{
    await_active, given_endpoint, Emulator, BUCKET_ARN, DEADLINE, DESTINATION_EXCEPTION,
    FAILED_OUTPUT_TYPE, POLL_INTERVAL, RETRY_SECONDS,
};

/// The `CompressionFormat` value asking for a gzipped object, and the file extension such an
/// object's name carries.
const GZIP: &str = "GZIP";
const GZIP_EXTENSION: &str = ".gz";

/// The `CompressionFormat` value asking for an object that is not compressed at all, which is
/// also what a destination naming no format writes.
const UNCOMPRESSED: &str = "UNCOMPRESSED";

/// How the records of one row reach the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Put {
    /// One `PutRecord` per record.
    OneCallPerRecord,
    /// One `PutRecordBatch` carrying every record of the row.
    OneBatch,
}

/// One row: the delivery's form, and how the endpoint refuses it.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// The status the endpoint answers, which the row's access key instructs it with.
    status: u16,
    /// The account the endpoint's answer gives of the refusal.
    reported: &'static str,
    /// How many records this row puts, which is how many documents its error output carries.
    records: usize,
    put: Put,
    /// The backup's `CompressionFormat`, and `None` for a destination naming none.
    compression: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one-record-put-on-its-own",
            status: 500,
            reported: "the sink rejected the record: its tenant is not registered",
            records: 1,
            put: Put::OneCallPerRecord,
            compression: None,
        },
        Row {
            case: "a-batch-of-three-uncompressed",
            status: 503,
            reported: "the sink refused the batch: schema version 7 is not accepted",
            records: 3,
            put: Put::OneBatch,
            compression: Some(UNCOMPRESSED),
        },
        Row {
            case: "a-batch-of-three-gzipped",
            status: 502,
            reported: "the sink throttled this delivery: its own store answered 429",
            records: 3,
            put: Put::OneBatch,
            compression: Some(GZIP),
        },
        Row {
            case: "two-records-put-one-by-one-gzipped",
            status: 500,
            reported: "the sink refused this delivery: the signing key it carries expired",
            records: 2,
            put: Put::OneCallPerRecord,
            compression: Some(GZIP),
        },
    ]
}

impl Row {
    /// The `AccessKey` this row's destination carries, which is the instruction the endpoint's
    /// handler answers it by — the status, how many times over the message is reported, and
    /// the message itself.
    fn access_key(&self) -> String {
        format!("{}:1:{}", self.status, self.reported)
    }

    /// The records this row puts: distinct per row, so a document found under one row's prefix
    /// can only have come from that row's buffer.
    fn payloads(&self) -> Vec<Vec<u8>> {
        (0..self.records)
            .map(|index| format!("{}-record-{index}", self.case).into_bytes())
            .collect()
    }

    /// The prefix the row's refused records are written under: its own `ErrorOutputPrefix`
    /// with `!{firehose:error-output-type}` expanded.
    fn error_prefix(&self) -> String {
        format!("{}/err/{FAILED_OUTPUT_TYPE}/", self.case)
    }

    /// Whether this row's parked object is gzipped.
    fn gzipped(&self) -> bool {
        self.compression == Some(GZIP)
    }

    /// The bytes of one parked object of this row, in the encoding its `CompressionFormat`
    /// asked for.
    fn decoded(&self, body: &[u8]) -> Vec<u8> {
        if !self.gzipped() {
            return body.to_vec();
        }
        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(body)
            .read_to_end(&mut decoded)
            .unwrap_or_else(|error| {
                panic!(
                    "case {:?}: a gzipped object must inflate, got {error}",
                    self.case
                )
            });
        decoded
    }
}

/// The destination configuration of `row`: the HTTP endpoint at `url`, answered by the access
/// key the row instructs the handler with, backing its refused records up under the row's own
/// error output prefix in the row's own compression format.
fn destination(row: &Row, url: &str) -> Value {
    let mut backup = json!({
        "RoleARN": ROLE_ARN,
        "BucketARN": BUCKET_ARN,
        "Prefix": format!("{}/backup/", row.case),
        "ErrorOutputPrefix": format!("{}/err/!{{firehose:error-output-type}}/", row.case),
    });
    if let (Some(members), Some(format)) = (backup.as_object_mut(), row.compression) {
        members.insert("CompressionFormat".to_string(), format.into());
    }
    json!({
        "EndpointConfiguration": {
            "Url": url,
            "Name": "the-lambda-sink",
            "AccessKey": row.access_key(),
        },
        // The shortest buffering the hints admit, so a row waits for the emulator's own
        // delivery span and nothing longer.
        "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
        "RetryOptions": { "DurationInSeconds": RETRY_SECONDS },
        "S3BackupMode": "FailedDataOnly",
        "S3Configuration": backup,
    })
}

/// Put every record of `row` on the path the row names, failing if any put is not accepted.
async fn put_records(emulator: &Emulator, row: &Row) {
    let payloads = row.payloads();
    match row.put {
        Put::OneCallPerRecord => {
            for payload in &payloads {
                let answered = emulator
                    .firehose_ok(
                        row.case,
                        "PutRecord",
                        json!({
                            "DeliveryStreamName": row.case,
                            "Record": { "Data": aws_smithy_types::base64::encode(payload) },
                        }),
                    )
                    .await;
                assert!(
                    answered["RecordId"]
                        .as_str()
                        .is_some_and(|id| !id.is_empty()),
                    "case {:?}: the put must answer the record's id, got {answered}",
                    row.case
                );
            }
        }
        Put::OneBatch => {
            let entries: Vec<Value> = payloads
                .iter()
                .map(|payload| json!({ "Data": aws_smithy_types::base64::encode(payload) }))
                .collect();
            let answered = emulator
                .firehose_ok(
                    row.case,
                    "PutRecordBatch",
                    json!({ "DeliveryStreamName": row.case, "Records": entries }),
                )
                .await;
            assert_eq!(
                answered["FailedPutCount"].as_i64(),
                Some(0),
                "case {:?}: every record of the batch must be accepted, got {answered}",
                row.case
            );
        }
    }
}

/// Every error-output document of `row`, read back out of Amazon S3 once the row's records
/// have all been parked.
///
/// Nothing here drives a delivery: what makes the documents appear is the emulator's own
/// delivery clock, which is what a producer that makes no further call observes. The objects
/// are polled for until they hold a document per record of the row, since a row whose records
/// were put one at a time can have them parked in more than one object.
async fn parked_documents(emulator: &Emulator, row: &Row) -> Vec<Value> {
    let prefix = row.error_prefix();
    let case = row.case;
    let deadline = Instant::now() + DEADLINE;
    loop {
        let keys = emulator.keys_under(case, &prefix).await;
        let mut documents = Vec::new();
        for key in &keys {
            assert!(
                key.starts_with(&prefix),
                "case {case:?}: the refused records are written under {prefix:?}, got {key:?}"
            );
            assert_eq!(
                key.ends_with(GZIP_EXTENSION),
                row.gzipped(),
                "case {case:?}: the object's name carries the extension its compression \
                 format ({:?}) names, got {key:?}",
                row.compression
            );
            let body = emulator.object(case, key).await;
            let decoded = row.decoded(&body);
            for line in String::from_utf8_lossy(&decoded).lines() {
                documents.push(serde_json::from_str::<Value>(line).unwrap_or_else(|error| {
                    panic!("case {case:?}: every parked line is a JSON document, got {error}")
                }));
            }
        }
        if documents.len() >= row.records {
            return documents;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {} parked document(s) under {prefix:?} within \
             {DEADLINE:?}, got {}",
            row.records,
            documents.len()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn every_form_of_a_refused_delivery_parks_the_message_the_endpoint_reported() {
    let emulator = Emulator::new();
    emulator.create_bucket().await;
    let url = given_endpoint(&emulator).await;
    let rows = rows();

    // Every row's stream is created first, so the one `CREATING` -> `ACTIVE` span covers the
    // whole table.
    for row in &rows {
        emulator
            .firehose_ok(
                row.case,
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": row.case,
                    "DeliveryStreamType": "DirectPut",
                    "HttpEndpointDestinationConfiguration": destination(row, &url),
                }),
            )
            .await;
    }
    for row in &rows {
        await_active(&emulator, row.case).await;
    }

    // Every row's records are put before any outcome is read, so the rows' retry windows run
    // beside each other rather than one after the other.
    for row in &rows {
        put_records(&emulator, row).await;
    }

    for row in &rows {
        let case = row.case;
        let documents = parked_documents(&emulator, row).await;
        assert_eq!(
            documents.len(),
            row.records,
            "case {case:?}: one document per refused record, got {documents:?}"
        );
        let mut parked_payloads = Vec::new();
        for document in &documents {
            let code = document["errorCode"].as_str().unwrap_or_default();
            assert_eq!(
                code, DESTINATION_EXCEPTION,
                "case {case:?}: the document names how the delivery failed, expected \
                 {DESTINATION_EXCEPTION:?}, got {document}"
            );
            let message = document["errorMessage"].as_str().unwrap_or_default();
            assert_eq!(
                message, row.reported,
                "case {case:?}: the document carries the message the endpoint reported, \
                 expected {:?}, got {message:?}",
                row.reported
            );
            // The buffer was attempted when it came due and again inside the retry window
            // this row configured, so the document states more than the one attempt a
            // first-try failure costs.
            let attempts = document["attemptsMade"].as_u64().unwrap_or_default();
            assert!(
                attempts >= 2,
                "case {case:?}: the document states the attempts the buffer cost over its \
                 {RETRY_SECONDS}s retry window, expected at least 2, got {attempts}"
            );
            assert!(
                document["arrivalTimestamp"].as_u64().unwrap_or_default() > 0
                    && document["attemptEndingTimestamp"]
                        .as_u64()
                        .unwrap_or_default()
                        > 0,
                "case {case:?}: the document says when the record arrived and when the \
                 attempt ended, got {document}"
            );
            let raw = document["rawData"].as_str().unwrap_or_default();
            parked_payloads.push(
                aws_smithy_types::base64::decode(raw)
                    .unwrap_or_else(|error| panic!("case {case:?}: rawData is base64, got {error}")),
            );
        }
        // Every record the row put is parked, whichever call accepted it and however many
        // objects the parked documents were written as.
        parked_payloads.sort();
        let mut expected = row.payloads();
        expected.sort();
        assert_eq!(
            parked_payloads,
            expected,
            "case {case:?}: the documents carry the records' own bytes, got {:?}",
            parked_payloads
                .iter()
                .map(|payload| String::from_utf8_lossy(payload).to_string())
                .collect::<Vec<String>>()
        );
    }
}
