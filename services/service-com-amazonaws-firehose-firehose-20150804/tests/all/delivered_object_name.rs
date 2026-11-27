// SPDX-License-Identifier: CC0-1.0

//! The NAME a delivered Amazon S3 object carries.
//!
//! The rule under test: the key of a delivered object is the evaluated prefix followed by
//! `<stream name>-<version>-yyyy-MM-dd-HH-mm-ss-<uuid><file extension>` — the stream's own
//! name, the decimal stream version `DescribeDeliveryStream` reports as `VersionId` for the
//! configuration the delivery was made under, the delivery's own second, and a value unique
//! to the object, with nothing else between them. So the tail of a key parses into exactly
//! those four parts, the version part tracks the `VersionId` across a configuration change,
//! and two objects of one stream delivered in the same second are told apart by the unique
//! part alone.

use std::time::{Duration, Instant, SystemTime};

use chrono::NaiveDateTime;
use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::{PAST_THE_TRANSITION, ROLE_ARN};
use crate::destination_delivery::Emulator;

/// How long a delivery is waited for before a case fails.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(20);

/// How often the destination is polled while waiting for a delivery.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The buffering hints every destination here configures: the shortest wait
/// `IntervalInSeconds` admits (published range 0..900), because these cases are about the
/// NAME an object is written under and not about how long its buffer is held.
fn prompt_hints() -> Value {
    json!({ "SizeInMBs": 1, "IntervalInSeconds": 0 })
}

/// One name case: how the destination states where its objects go, and what the name's own
/// tail must then end with.
struct Row {
    case: &'static str,
    /// The `Prefix` the destination configures, `None` to leave the member out.
    prefix: Option<&'static str>,
    /// The `CompressionFormat` the destination configures, `None` to leave the member out
    /// (the documented default, `UNCOMPRESSED`).
    compression: Option<&'static str>,
    /// The `FileExtension` the destination configures, `None` to leave the member out.
    file_extension: Option<&'static str>,
    /// The extension the delivered key must end with, empty for none at all.
    extension: &'static str,
    /// How many records are put, all into the one buffer.
    records: usize,
}

/// The name cases.
///
/// The rows differ along what the rule generalises over: whether the destination names a
/// `Prefix` at all and what form it takes (a plain folder, a nested one, one not ending in
/// the delimiter, and one whose own `!{timestamp:...}` expression replaces the automatic
/// segments), the `CompressionFormat` whose default extension the name takes (every value
/// `smithy_query(query="shape", shape="CompressionFormat")` lists: `UNCOMPRESSED` with none,
/// `GZIP` with `.gz`, `ZIP` with `.zip`, and `Snappy` and `HADOOP_SNAPPY`
/// sharing `.snappy`, plus the member left out), a `FileExtension` overriding that default,
/// and how many records the flushed buffer holds.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no prefix at all, so the automatic segments are the whole prefix",
            prefix: None,
            compression: None,
            file_extension: None,
            extension: "",
            records: 1,
        },
        Row {
            case: "a plain folder prefix and several records in the one buffer",
            prefix: Some("landing/"),
            compression: Some("UNCOMPRESSED"),
            file_extension: None,
            extension: "",
            records: 3,
        },
        Row {
            case: "a nested prefix with GZIP, whose default extension the name takes",
            prefix: Some("raw/events/json/"),
            compression: Some("GZIP"),
            file_extension: None,
            extension: ".gz",
            records: 2,
        },
        Row {
            case: "a prefix not ending in the delimiter, with ZIP",
            prefix: Some("tail-"),
            compression: Some("ZIP"),
            file_extension: None,
            extension: ".zip",
            records: 1,
        },
        // Both formats that compress with Snappy carry the SAME extension: the container
        // the bytes are framed in differs, the codec a consumer selects by the suffix does
        // not.
        Row {
            case: "a prefix with Snappy, whose default extension the name takes",
            prefix: Some("snp/"),
            compression: Some("Snappy"),
            file_extension: None,
            extension: ".snappy",
            records: 2,
        },
        Row {
            case: "a prefix with HADOOP_SNAPPY, which shares the Snappy extension",
            prefix: Some("hsn/"),
            compression: Some("HADOOP_SNAPPY"),
            file_extension: None,
            extension: ".snappy",
            records: 1,
        },
        Row {
            case: "a FileExtension overriding the Snappy default",
            prefix: Some("snp-ext/"),
            compression: Some("Snappy"),
            file_extension: Some(".snappy.jsonl"),
            extension: ".snappy.jsonl",
            records: 1,
        },
        Row {
            case: "a prefix whose own timestamp expression replaces the automatic segments",
            prefix: Some("stamped/!{timestamp:yyyy-MM-dd}/"),
            compression: None,
            file_extension: None,
            extension: "",
            records: 1,
        },
        Row {
            case: "a FileExtension overriding the compression format's own",
            prefix: Some("ext/"),
            compression: Some("GZIP"),
            file_extension: Some(".jsonl"),
            extension: ".jsonl",
            records: 1,
        },
    ]
}

/// The parts the tail of a delivered key states.
#[derive(Debug)]
struct Tail {
    /// The decimal stream version.
    version: String,
    /// The delivery's own second, as the key's six calendar fields state it.
    stamp: NaiveDateTime,
    /// The value unique to the object.
    unique: String,
}

#[tokio::test]
async fn a_delivered_key_carries_the_documented_stream_version_timestamp_and_unique_part() {
    let emulator = Emulator::new();
    let bucket = "object-name-bucket";
    emulator.create_bucket(bucket).await;

    // Every stream is created before any of them is awaited, so the suite spends ONE
    // clock-owed CREATING -> ACTIVE span rather than one per row. Each name carries the
    // hyphens a Firehose stream name admits, which is what makes the stream part of a key
    // something a reader has to strip rather than split on.
    let mut legs: Vec<(usize, String, Row)> = Vec::new();
    for (ordinal, row) in rows().into_iter().enumerate() {
        let stream = format!("object-name-stream-{ordinal}");
        let mut config = json!({
            "RoleARN": ROLE_ARN,
            "BucketARN": format!("arn:aws:s3:::{bucket}"),
            "BufferingHints": prompt_hints(),
        });
        if let Some(prefix) = row.prefix {
            config["Prefix"] = json!(prefix);
        }
        if let Some(compression) = row.compression {
            config["CompressionFormat"] = json!(compression);
        }
        if let Some(extension) = row.file_extension {
            config["FileExtension"] = json!(extension);
        }
        emulator
            .firehose_ok(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": &stream,
                    "DeliveryStreamType": "DirectPut",
                    "ExtendedS3DestinationConfiguration": config,
                }),
            )
            .await;
        legs.push((ordinal, stream, row));
    }
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    let put_at = SystemTime::now();
    for (ordinal, stream, row) in &legs {
        for index in 0..row.records {
            let record = format!("{{\"row\":{ordinal},\"record\":{index}}}");
            let (status, answered) = emulator.put_record(stream, record.as_bytes()).await;
            assert_eq!(
                status, 200,
                "case {:?}: the put must be accepted, got {answered:?}",
                row.case
            );
        }
    }

    for (_, stream, row) in &legs {
        let version = version_of(&emulator, stream).await;
        let key = first_key(&emulator, row.case, bucket, stream).await;
        let read_at = SystemTime::now();
        let tail = parsed_tail(&key, stream, row.case, row.extension);
        assert_eq!(
            tail.version, version,
            "case {:?}: the version part is the VersionId DescribeDeliveryStream reports, \
             got {:?} in key {key:?}",
            row.case, tail.version
        );
        assert_in_window(row.case, tail.stamp, &key, (put_at, read_at));
    }
}

/// The version part follows the stream's `VersionId`, which "increments" on a configuration
/// change — "each time the destination is updated for a delivery stream"
/// (`DeliveryStreamDescription$VersionId`) — and the unique part is what tells two objects of
/// one stream apart.
#[tokio::test]
async fn the_version_part_tracks_a_configuration_change_and_the_unique_part_tells_objects_apart() {
    let emulator = Emulator::new();
    let bucket = "object-name-version-bucket";
    let stream = "object-name-versioned-stream";
    let prefix = "versioned/";
    emulator.create_bucket(bucket).await;
    emulator
        .firehose_ok(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": stream,
                "DeliveryStreamType": "DirectPut",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": format!("arn:aws:s3:::{bucket}"),
                    "Prefix": prefix,
                    "CompressionFormat": "UNCOMPRESSED",
                    "BufferingHints": prompt_hints(),
                },
            }),
        )
        .await;
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    let created_version = version_of(&emulator, stream).await;
    let (status, answered) = emulator.put_record(stream, b"first").await;
    assert_eq!(status, 200, "the first put got {answered:?}");
    let first = keys_once(&emulator, "the first delivery", bucket, prefix, 1).await[0].clone();
    let first_tail = parsed_tail(&first, stream, "the first delivery", "");
    assert_eq!(
        first_tail.version, created_version,
        "the first delivery names the created stream's VersionId, got {first:?}"
    );

    // A second buffer of the SAME configuration: same stream, same version, and an object
    // told apart from the first by its unique part alone.
    let (status, answered) = emulator.put_record(stream, b"second").await;
    assert_eq!(status, 200, "the second put got {answered:?}");
    let two = keys_once(&emulator, "the second delivery", bucket, prefix, 2).await;
    let second = two
        .iter()
        .find(|key| **key != first)
        .expect("the second delivery is a key of its own")
        .clone();
    let second_tail = parsed_tail(&second, stream, "the second delivery", "");
    assert_eq!(
        second_tail.version, created_version,
        "a delivery under an unchanged configuration keeps its version, got {second:?}"
    );
    assert_ne!(
        second_tail.unique, first_tail.unique,
        "two objects of one stream are told apart by their unique part, got {first:?} and \
         {second:?}"
    );

    // A configuration change bumps the VersionId, and the next delivery names the bumped one.
    let description = describe(&emulator, stream).await;
    let destination = description["Destinations"][0]["DestinationId"]
        .as_str()
        .expect("the description carries a destination id")
        .to_string();
    emulator
        .firehose_ok(
            "UpdateDestination",
            json!({
                "DeliveryStreamName": stream,
                "CurrentDeliveryStreamVersionId": created_version,
                "DestinationId": destination,
                "ExtendedS3DestinationUpdate": { "Prefix": prefix, "ErrorOutputPrefix": "err/" },
            }),
        )
        .await;
    let updated_version = version_of(&emulator, stream).await;
    assert_ne!(
        updated_version, created_version,
        "an UpdateDestination bumps the VersionId, got {updated_version:?}"
    );

    let (status, answered) = emulator.put_record(stream, b"third").await;
    assert_eq!(status, 200, "the third put got {answered:?}");
    let three = keys_once(&emulator, "the delivery after the update", bucket, prefix, 3).await;
    let third = three
        .iter()
        .find(|key| **key != first && **key != second)
        .expect("the third delivery is a key of its own")
        .clone();
    let third_tail = parsed_tail(&third, stream, "the delivery after the update", "");
    assert_eq!(
        third_tail.version, updated_version,
        "the delivery after the update names the bumped VersionId, got {third:?}"
    );
}

/// One stream's description.
async fn describe(emulator: &Emulator, stream: &str) -> Value {
    emulator
        .firehose_ok(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": stream }),
        )
        .await["DeliveryStreamDescription"]
        .clone()
}

/// The `VersionId` `DescribeDeliveryStream` reports for `stream`.
async fn version_of(emulator: &Emulator, stream: &str) -> String {
    describe(emulator, stream).await["VersionId"]
        .as_str()
        .expect("the description carries a version id")
        .to_string()
}

/// The first key delivered for `stream`, or a failure naming `case` once the deadline passes.
async fn first_key(emulator: &Emulator, case: &str, bucket: &str, stream: &str) -> String {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let keys = emulator.keys_under(bucket, "").await;
        if let Some(key) = keys.into_iter().find(|key| key.contains(stream)) {
            return key;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected a delivered object for {stream} in {bucket}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The keys under `prefix` once there are at least `want` of them.
async fn keys_once(
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
            "case {case:?}: expected {want} object(s) under {prefix:?}, got {keys:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The documented parts of `key`'s tail, or a failure naming `case`.
///
/// The tail is the last `/`-separated segment: everything before it is the evaluated prefix,
/// which the automatic `YYYY/MM/DD/HH` segments always end with a delimiter. What follows the
/// stream's own name — which carries hyphens of its own, so it is STRIPPED rather than split
/// on — must be exactly the version, the six calendar fields of the delivery's second and the
/// five groups of the unique part: twelve hyphen-separated fields and no more, which is what
/// an extra segment between the timestamp and the unique part would break.
fn parsed_tail(key: &str, stream: &str, case: &str, extension: &str) -> Tail {
    let name = key.rsplit('/').next().unwrap_or(key);
    let name = name.strip_suffix(extension).unwrap_or_else(|| {
        panic!("case {case:?}: the key must end in {extension:?}, got {key:?}")
    });
    let rest = name.strip_prefix(&format!("{stream}-")).unwrap_or_else(|| {
        panic!("case {case:?}: the object's name must start with its stream, got {key:?}")
    });
    let fields: Vec<&str> = rest.split('-').collect();
    assert_eq!(
        fields.len(),
        12,
        "case {case:?}: the tail is the version, six calendar fields and the five groups of \
         the unique part — nothing else, got {rest:?} in key {key:?}"
    );
    assert!(
        !fields[0].is_empty() && fields[0].bytes().all(|byte| byte.is_ascii_digit()),
        "case {case:?}: the version part is a decimal number, got {:?} in key {key:?}",
        fields[0]
    );
    let calendar = &fields[1..7];
    assert!(
        [4, 2, 2, 2, 2, 2]
            .iter()
            .zip(calendar)
            .all(|(width, field)| field.len() == *width
                && field.bytes().all(|byte| byte.is_ascii_digit())),
        "case {case:?}: the timestamp is yyyy-MM-dd-HH-mm-ss, got {calendar:?} in key {key:?}"
    );
    let unique = &fields[7..12];
    assert!(
        [8, 4, 4, 4, 12]
            .iter()
            .zip(unique)
            .all(|(width, field)| field.len() == *width
                && field.bytes().all(|byte| byte.is_ascii_hexdigit())),
        "case {case:?}: the unique part is a hyphenated hexadecimal value, got {unique:?} in \
         key {key:?}"
    );
    let stamp = format!(
        "{}-{}-{} {}:{}:{}",
        calendar[0], calendar[1], calendar[2], calendar[3], calendar[4], calendar[5]
    );
    Tail {
        version: fields[0].to_string(),
        stamp: NaiveDateTime::parse_from_str(&stamp, "%Y-%m-%d %H:%M:%S").unwrap_or_else(
            |error| panic!("case {case:?}: {stamp:?} out of {key:?} is a calendar second: {error}"),
        ),
        unique: unique.join("-"),
    }
}

/// Require `stamp` to be the delivery's own second: the delivery happened inside `window`,
/// and the destination names no `CustomTimeZone`, so UTC's reading of an instant of that
/// window is what the key states — truncated to the second the key's digits carry.
fn assert_in_window(case: &str, stamp: NaiveDateTime, key: &str, window: (SystemTime, SystemTime)) {
    let (from, to) = window;
    let earliest = utc_second(from);
    let latest = utc_second(to);
    assert!(
        stamp >= earliest && stamp <= latest,
        "case {case:?}: the timestamp states the delivery's own second, i.e. between \
         {earliest} and {latest}, got {stamp} in key {key:?}"
    );
}

/// `at` as UTC reads it, truncated to the second.
fn utc_second(at: SystemTime) -> NaiveDateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is after the Unix epoch");
    let seconds =
        i64::try_from(since_epoch.as_secs()).expect("the instant fits the calendar's range");
    chrono::DateTime::from_timestamp(seconds, 0)
        .expect("the instant is one the calendar places")
        .naive_utc()
}
