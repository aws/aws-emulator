// SPDX-License-Identifier: CC0-1.0

//! Which compression formats a destination can ask for: the ones `CompressionFormat`
//! declares, and no others.
//!
//! The rule under test: the values a member of an enumerated type admits are exactly the
//! ones that type declares in the model, so a destination configured with ANY value the
//! type declares is taken, reports that value back and delivers its records encoded that
//! way, while a value the type does not declare is refused with `ValidationException` and
//! the type's whole value set, leaving no stream behind.
//!
//! Both halves are driven from the GENERATED catalogue of the type
//! (`ops::types::shared_types::COMPRESSION_FORMAT_VALUES`, rendered from
//! `smithy_query(query="shape", shape="CompressionFormat")`) rather than from a list
//! written out here, so a model revision moves this suite with it: a value the model gains
//! has to be taken and delivered, and a value it drops has to be refused.
//!
//! A child of the delivery suite, on its harness: the Firehose calls as `POST /` with
//! `X-Amz-Target`, the destination read as the path-style Amazon S3 requests a client
//! sends.

use std::time::{Duration, Instant};

use serde_json::json;
use service_com_amazonaws_firehose_firehose_20150804::ops::types::shared_types::COMPRESSION_FORMAT_VALUES;

use crate::delivery_stream_lifecycle::{error_code, error_message, ROLE_ARN};
use crate::destination_delivery::Emulator;

/// The bucket every delivery here lands in.
const BUCKET: &str = "modelled-compression-bucket";

/// The wire error code a refused request carries.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// How long a stream is given to become `ACTIVE`, and a delivery to land, before a case
/// fails. Generous: a case waits for the CONDITION it is about — the status it reads back,
/// the object being there — and this only stops a wait that will never end.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(30);

/// How often the service is asked again while waiting for one of those conditions.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The record every case puts, long enough that an encoder has something to do with it.
const RECORD: &[u8] = b"{\"event\":\"compressed\",\"payload\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbb\"}";

#[tokio::test]
async fn every_compression_format_the_model_declares_is_taken_and_delivered() {
    let emulator = Emulator::new();
    emulator.create_bucket(BUCKET).await;

    // Every stream is created before any of them is awaited, so the suite waits out ONE
    // clock-owed CREATING -> ACTIVE span rather than one per format.
    let mut legs: Vec<(String, String, &str)> = Vec::new();
    for (ordinal, format) in COMPRESSION_FORMAT_VALUES.iter().enumerate() {
        let stream = format!("modelled-compression-{ordinal}");
        let prefix = format!("fmt-{ordinal}/");
        emulator
            .firehose_ok(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": &stream,
                    "DeliveryStreamType": "DirectPut",
                    "ExtendedS3DestinationConfiguration": {
                        "RoleARN": ROLE_ARN,
                        "BucketARN": format!("arn:aws:s3:::{BUCKET}"),
                        "Prefix": &prefix,
                        "CompressionFormat": format,
                        // The shortest wait `IntervalInSeconds` admits (published range
                        // 0..900): this suite is about WHICH formats are served, not about
                        // how long a buffer is held.
                        "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
                    },
                }),
            )
            .await;
        legs.push((stream, prefix, format));
    }
    // The stream reports back the format it was created with, so the value was taken rather
    // than dropped on the way in. Read once the stream is ACTIVE, which is the condition a
    // put needs and the one each leg waits for.
    for (stream, _, format) in &legs {
        let described = active(&emulator, format, stream).await;
        let reported = &described["DeliveryStreamDescription"]["Destinations"][0]
            ["ExtendedS3DestinationDescription"]["CompressionFormat"];
        assert_eq!(
            reported,
            &json!(format),
            "case {format:?}: expected the stream to report the format it was created with, \
             got {reported:?}"
        );
    }

    for (stream, _, format) in &legs {
        let (status, answered) = emulator.put_record(stream, RECORD).await;
        assert_eq!(
            status, 200,
            "case {format:?}: the put must be accepted, got {answered:?}"
        );
    }

    // Every declared format has an encoder behind it: a format this emulator could not
    // write would deliver NOTHING, so the object's being there is the assertion.
    for (_, prefix, format) in &legs {
        let key = delivered_key(&emulator, format, prefix).await;
        let body = emulator.object(BUCKET, &key).await;
        assert!(
            !body.is_empty(),
            "case {format:?}: expected {key} to hold the delivered buffer, got no bytes"
        );
        let plain = body == RECORD;
        assert_eq!(
            plain,
            *format == "UNCOMPRESSED",
            "case {format:?}: the body must be encoded exactly when the format compresses, \
             got a {}-byte body",
            body.len()
        );
    }
}

/// The compression-format names a destination is refused for.
///
/// Real compression formats, named the way a caller expecting one of them would spell it,
/// and each checked against the generated catalogue below so a row cannot silently test a
/// value the type DOES declare: the type is a closed set, and a name outside it is a name
/// the service does not take, however widely the format itself is used.
const FORMATS_THE_TYPE_DOES_NOT_DECLARE: [&str; 6] =
    ["ZSTD", "zstd", "BROTLI", "LZ4", "DEFLATE", "SNAPPY"];

#[tokio::test]
async fn a_compression_format_the_model_does_not_declare_is_refused_with_the_whole_set() {
    let emulator = Emulator::new();

    for (ordinal, format) in FORMATS_THE_TYPE_DOES_NOT_DECLARE.iter().enumerate() {
        assert!(
            !COMPRESSION_FORMAT_VALUES.contains(format),
            "case {format:?}: this row must name a value the type does not declare, and the \
             catalogue holds {COMPRESSION_FORMAT_VALUES:?}"
        );
        let stream = format!("unmodelled-compression-{ordinal}");
        let (status, answered) = emulator
            .firehose(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": &stream,
                    "DeliveryStreamType": "DirectPut",
                    "ExtendedS3DestinationConfiguration": {
                        "RoleARN": ROLE_ARN,
                        "BucketARN": format!("arn:aws:s3:::{BUCKET}"),
                        "CompressionFormat": format,
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {format:?}: expected the create to be refused, got {answered:?}"
        );
        let code = error_code(&answered);
        assert_eq!(
            code, VALIDATION_EXCEPTION,
            "case {format:?}: expected {VALIDATION_EXCEPTION}, got {code:?} with {answered:?}"
        );

        // The refusal enumerates exactly the values the type declares: the listing is that
        // membership re-ordered, not a set of its own.
        let message = error_message(&answered);
        let mut listed = stated_set(&message).unwrap_or_else(|| {
            panic!("case {format:?}: expected an enum value set in {message:?}")
        });
        listed.sort_unstable();
        let mut modelled: Vec<&str> = COMPRESSION_FORMAT_VALUES.to_vec();
        modelled.sort_unstable();
        assert_eq!(
            listed, modelled,
            "case {format:?}: expected the clause to state {modelled:?}, got {message:?}"
        );

        // And the refused create left nothing behind under the name it carried.
        let (status, described) = emulator
            .firehose(
                "DescribeDeliveryStream",
                json!({ "DeliveryStreamName": &stream }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {format:?}: expected no stream named {stream}, got {described:?}"
        );
        let code = error_code(&described);
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {format:?}: expected no stream named {stream}, got {code:?} with \
             {described:?}"
        );
    }
}

/// The values an `enum value set: [ … ]` clause enumerates, as it enumerates them.
fn stated_set(message: &str) -> Option<Vec<&str>> {
    let (_, tail) = message.split_once("Member must satisfy enum value set: [")?;
    let (set, _) = tail.split_once(']')?;
    Some(set.split(", ").collect())
}

/// What `DescribeDeliveryStream` reports for `stream` once Firehose reports it `ACTIVE`,
/// which is when it takes a put: the create leaves it `CREATING` and the clock settles it.
/// The wait is for that STATUS and not for a span, so a slow and a fast host read the same.
async fn active(emulator: &Emulator, case: &str, stream: &str) -> serde_json::Value {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let described = emulator
            .firehose_ok(
                "DescribeDeliveryStream",
                json!({ "DeliveryStreamName": stream }),
            )
            .await;
        let status = described["DeliveryStreamDescription"]["DeliveryStreamStatus"].as_str();
        if status == Some("ACTIVE") {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {stream} to become ACTIVE, it is {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The one key delivered under `prefix`, or a failure naming `case` once the deadline passes.
async fn delivered_key(emulator: &Emulator, case: &str, prefix: &str) -> String {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let keys = emulator.keys_under(BUCKET, prefix).await;
        if let Some(key) = keys.into_iter().next() {
            return key;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected a delivered object under {prefix:?} in {BUCKET}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
