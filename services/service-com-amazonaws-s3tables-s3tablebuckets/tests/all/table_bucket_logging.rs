// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `PutTableBucketLogging` and
//! `GetTableBucketLogging`.
//!
//! The rule under test: a table bucket's server access logging configuration is
//! whatever the last put set on it — one put SETS, UPDATES or REMOVES the whole
//! configuration — and a read reports the status with `loggingEnabled` carrying
//! that configuration, or with the member absent when the bucket carries none,
//! which is what a put of an EMPTY status leaves behind. A bucket the store does
//! not hold is refused with the modeled not-found by both.
//!
//! The table spans what the rule generalises over: each value of the enumerated
//! member the configuration carries, the prefix (absent, short, and one at the
//! published maximum), and the order of writes — a first put, a put over an
//! existing configuration, and a put that removes one.

use serde_json::{json, Value};

use super::harness::{assert_served, served, Emulator};

/// The fixed sentence this service states a table bucket the store does not hold
/// with.
const NO_BUCKET: &str = "The specified bucket does not exist.";

/// The values `PartitionDateSource` accepts, from the model
/// (`smithy_query(query="shape", shape="PartitionDateSource")`).
const PARTITION_DATE_SOURCES: [&str; 2] = ["EventTime", "DeliveryTime"];

/// The widest `targetPrefix` the model admits: `LoggingTargetPrefix` is
/// `@length(max: 1024)` (`smithy_query(query="shape",
/// shape="LoggingTargetPrefix")`).
const TARGET_PREFIX_MAX: usize = 1024;

/// A `loggingEnabled` configuration delivering to `target_bucket` under
/// `target_prefix`, partitioned by `partition_date_source`.
fn logging_enabled(
    target_bucket: &str,
    target_prefix: &str,
    partition_date_source: &str,
) -> Value {
    json!({
        "loggingEnabled": {
            "targetBucket": target_bucket,
            "targetPrefix": target_prefix,
            "targetObjectKeyFormat": {
                "partitionedPrefix": { "partitionDateSource": partition_date_source }
            }
        }
    })
}

/// Create a table bucket and answer its ARN.
async fn create_bucket(emulator: &Emulator, region: &str, name: &str) -> String {
    let created = served(
        emulator
            .create_table_bucket(region, &json!({ "name": name }))
            .await,
        "CreateTableBucket",
        name,
    );
    created.text("arn")
}

/// The `loggingEnabled` member a read reports, or `None` when the status carries
/// none.
fn reported_logging(reply: &Value, case: &str) -> Option<Value> {
    let status = reply
        .get("tableBucketLoggingStatus")
        .unwrap_or_else(|| panic!("case {case:?}: a read reports a logging status, got {reply}"));
    status.get("loggingEnabled").cloned()
}

/// RULE: a put stores the whole configuration the request carries and a read
/// reports it back, for every value the enumerated member takes and every prefix
/// the model admits.
#[tokio::test]
async fn a_logging_configuration_a_put_sets_is_what_a_read_reports() {
    let region = "us-east-1";
    // The rows differ along what the rule generalises over: the value of the
    // enumerated member, and the width of the prefix — absent, a short one, and
    // one at the maximum the model publishes for the member.
    let cases: Vec<(&str, &str, String)> = vec![
        ("event time, no prefix", PARTITION_DATE_SOURCES[0], String::new()),
        (
            "delivery time, a prefix",
            PARTITION_DATE_SOURCES[1],
            "table-bucket-logs/".to_string(),
        ),
        (
            "event time, the widest prefix",
            PARTITION_DATE_SOURCES[0],
            "p".repeat(TARGET_PREFIX_MAX),
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, partition_date_source, prefix)) in cases.iter().enumerate() {
        let bucket_arn = create_bucket(&emulator, region, &format!("logging-{index}")).await;

        // A bucket nothing was put on delivers no logs, which the read reports by
        // leaving the member out rather than by refusing.
        let before = served(
            emulator.get_table_bucket_logging(region, &bucket_arn).await,
            "GetTableBucketLogging",
            case,
        );
        assert_eq!(
            reported_logging(&before.body, case),
            None,
            "case {case:?}: a bucket nothing was put on reports no loggingEnabled, got {body}",
            body = before.body
        );

        let target_bucket = format!("logs-for-{index}");
        let status = logging_enabled(&target_bucket, prefix, partition_date_source);
        let put = emulator
            .put_table_bucket_logging(region, &bucket_arn, &status)
            .await;
        assert_eq!(
            put.status, 204,
            "case {case:?}: PutTableBucketLogging answers its modelled 204, got {status} {body}",
            status = put.status,
            body = put.body
        );

        let read = served(
            emulator.get_table_bucket_logging(region, &bucket_arn).await,
            "GetTableBucketLogging",
            case,
        );
        let reported = reported_logging(&read.body, case);
        assert_eq!(
            reported.as_ref(),
            status.get("loggingEnabled"),
            "case {case:?}: the read reports the configuration the put set, got {reported:?}"
        );
        // A read changes nothing a later caller sees: the second read answers the
        // same configuration as the first.
        let again = served(
            emulator.get_table_bucket_logging(region, &bucket_arn).await,
            "GetTableBucketLogging",
            case,
        );
        assert_eq!(
            reported_logging(&again.body, case),
            reported,
            "case {case:?}: a read must not change what a later read sees"
        );
    }
}

/// RULE: a put over an existing configuration replaces it whole, and a put of an
/// EMPTY status removes it — which is how logging is turned off.
#[tokio::test]
async fn a_put_replaces_the_whole_configuration_and_an_empty_status_removes_it() {
    let region = "eu-west-1";
    let case = "replace then remove";
    let emulator = Emulator::new();
    let bucket_arn = create_bucket(&emulator, region, "logging-writes").await;

    let first = logging_enabled("logs-one", "one/", PARTITION_DATE_SOURCES[0]);
    assert_eq!(
        emulator
            .put_table_bucket_logging(region, &bucket_arn, &first)
            .await
            .status,
        204,
        "case {case:?}: the first put must be served"
    );

    // Every member of the second configuration differs from the first, so a read
    // that answered any member of the first would be visible.
    let second = logging_enabled("logs-two", "two/", PARTITION_DATE_SOURCES[1]);
    assert_eq!(
        emulator
            .put_table_bucket_logging(region, &bucket_arn, &second)
            .await
            .status,
        204,
        "case {case:?}: the second put must be served"
    );
    let read = served(
        emulator.get_table_bucket_logging(region, &bucket_arn).await,
        "GetTableBucketLogging",
        case,
    );
    assert_eq!(
        reported_logging(&read.body, case).as_ref(),
        second.get("loggingEnabled"),
        "case {case:?}: the read reports the configuration the LAST put set, got {body}",
        body = read.body
    );

    // "To disable logging, specify an empty TableBucketLoggingStatus."
    assert_eq!(
        emulator
            .put_table_bucket_logging(region, &bucket_arn, &json!({}))
            .await
            .status,
        204,
        "case {case:?}: the put that disables logging must be served"
    );
    let disabled = served(
        emulator.get_table_bucket_logging(region, &bucket_arn).await,
        "GetTableBucketLogging",
        case,
    );
    assert_eq!(
        reported_logging(&disabled.body, case),
        None,
        "case {case:?}: a bucket logging was disabled on reports no loggingEnabled, got {body}",
        body = disabled.body
    );
}

/// RULE: the configuration belongs to a table bucket, so a request naming one the
/// store does not hold is refused with the modeled not-found — the read and the
/// write alike, and whichever Region the request is made against.
#[tokio::test]
async fn logging_calls_on_a_bucket_the_store_does_not_hold_are_not_found() {
    let emulator = Emulator::new();
    // The rows differ along the Region the request addresses and along whether
    // the account holds any bucket at all in it.
    let cases: [(&str, &str); 2] = [
        ("no bucket at all", "us-east-1"),
        ("a bucket of another name", "ap-south-1"),
    ];
    for (case, region) in cases {
        if case == "a bucket of another name" {
            create_bucket(&emulator, region, "logging-neighbour").await;
        }
        let absent = format!("arn:aws:s3tables:{region}:000000000000:bucket/logging-absent");
        let read = emulator.get_table_bucket_logging(region, &absent).await;
        assert_eq!(
            (read.status, read.message()),
            (404, NO_BUCKET.to_string()),
            "case {case:?}: GetTableBucketLogging on an absent bucket is the modelled \
             not-found, got {status} {body}",
            status = read.status,
            body = read.body
        );
        let written = emulator
            .put_table_bucket_logging(
                region,
                &absent,
                &logging_enabled("logs", "", PARTITION_DATE_SOURCES[0]),
            )
            .await;
        assert_eq!(
            (written.status, written.message()),
            (404, NO_BUCKET.to_string()),
            "case {case:?}: PutTableBucketLogging on an absent bucket is the modelled \
             not-found, got {status} {body}",
            status = written.status,
            body = written.body
        );
    }

    // And a put refused for an absent bucket leaves the buckets the account does
    // hold exactly as they were.
    let held = create_bucket(&emulator, "us-east-1", "logging-untouched").await;
    let read = emulator.get_table_bucket_logging("us-east-1", &held).await;
    assert_served(&read, "GetTableBucketLogging", "untouched");
    assert_eq!(
        reported_logging(&read.body, "untouched"),
        None,
        "case \"untouched\": a refused put must configure nothing, got {body}",
        body = read.body
    );
}
