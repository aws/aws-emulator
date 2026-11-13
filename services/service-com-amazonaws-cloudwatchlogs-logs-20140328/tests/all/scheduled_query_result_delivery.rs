// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHERE a scheduled query's runs deliver their results — the
//! destinations `GetScheduledQueryHistory` reports the processing of.
//!
//! CloudWatch Logs and Amazon S3 are mounted on ONE shared engine — the same assembly
//! `core/router` performs — so an object a case reads back is one the object store itself
//! holds, written by the run over the in-process callback and not by anything here.
//!
//! The rule under test: a run delivers the rows it produced to each destination its query is
//! configured with, and the history reports a destination as `COMPLETE`, naming what the
//! delivery produced there, only once that delivery really happened — the lookup table
//! "automatically populate[d] or refresh[ed] with query results" holding those rows, and the
//! Amazon S3 object at the URI the destination names holding them too. A delivery the
//! destination will not take is reported as `FAILED`, carrying why, rather than as a delivery
//! that happened.
//!
//! The table spans what the rule generalises over: each destination type the model's
//! `DestinationConfiguration` carries (`s3Configuration`, `lookupTableConfiguration`), both at
//! once, and an Amazon S3 destination naming a bucket the object store does not hold.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::export_task_objects::Emulator;

/// The operations under test, and the reads a delivery is proved through.
const HISTORY: &str = "GetScheduledQueryHistory";
const CREATE: &str = "CreateScheduledQuery";
const GET_LOOKUP_TABLE: &str = "GetLookupTable";

/// The log group every case queries, and the stream its events are written to.
const GROUP: &str = "/aws/emulator/scheduled-query-delivery";
const STREAM: &str = "runs";

/// The IAM roles a scheduled query executes and delivers under.
const EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/scheduled-query-execution";
const DELIVERY_ROLE: &str = "arn:aws:iam::000000000000:role/scheduled-query-delivery";

/// The bucket the object store holds, which a destination of a case may name.
const BUCKET: &str = "scheduled-query-results";

/// A bucket the object store does NOT hold, which the failing row names.
const MISSING_BUCKET: &str = "scheduled-query-results-elsewhere";

/// The pipeline every case runs: the ERROR events of the window, as rows.
const QUERY_STRING: &str = "fields @timestamp, @message | filter @message like /ERROR/";

/// The messages seeded into the group — two of them ERROR, which is what the pipeline selects.
const MESSAGES: [&str; 3] = [
    "ERROR order 1 failed",
    "INFO order 2 accepted",
    "ERROR order 3 failed",
];

/// How many of [`MESSAGES`] the pipeline selects, which is also how many data rows a delivered
/// table holds.
const SELECTED_ROWS: i64 = 2;

/// How long a case is given to see the run the clock has made due, and how long it waits
/// between two looks. Generous on purpose: the condition — a record with its destinations
/// processed — is what is waited for, never a fixed span.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(180);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Milliseconds in one hour.
const HOUR: i64 = 60 * 60 * 1_000;

/// Wall-clock milliseconds.
fn now_ms() -> i64 {
    crate::harness::now_ms()
}

/// Seed [`GROUP`] with [`MESSAGES`] an hour back, so every run's lookback window holds them.
async fn seed_events(emulator: &Emulator, case: &str) {
    emulator
        .logs_ok(case, "CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    emulator
        .logs_ok(
            case,
            "CreateLogStream",
            &json!({"logGroupName": GROUP, "logStreamName": STREAM}),
        )
        .await;
    let first_ms = now_ms() - HOUR;
    let events: Vec<Value> = MESSAGES
        .iter()
        .enumerate()
        .map(|(index, message)| {
            json!({"timestamp": first_ms + index as i64 * 1_000, "message": message})
        })
        .collect();
    emulator
        .logs_ok(
            case,
            "PutLogEvents",
            &json!({
                "logGroupName": GROUP,
                "logStreamName": STREAM,
                "logEvents": events,
            }),
        )
        .await;
}

/// One row of the table: what the query delivers to, and what the history must then report.
struct Row {
    /// What the row is about, named in every assertion.
    case: &'static str,
    /// The name the scheduled query is created under.
    name: &'static str,
    /// The lookup table the run delivers to, when the row names one.
    table: Option<&'static str>,
    /// The bucket the run delivers to, when the row names one.
    bucket: Option<&'static str>,
    /// The status the Amazon S3 destination must reach, for a row that names one.
    s3_status: Option<&'static str>,
}

/// The `destinationConfiguration` this row's create names.
fn destination_configuration(row: &Row) -> Value {
    let mut configuration = json!({});
    if let Some(bucket) = row.bucket {
        configuration["s3Configuration"] = json!({
            "destinationIdentifier": format!("s3://{bucket}/history"),
            "roleArn": DELIVERY_ROLE,
        });
    }
    if let Some(table) = row.table {
        configuration["lookupTableConfiguration"] = json!({
            "tableName": table,
            "roleArn": DELIVERY_ROLE,
            "description": "the rows of the last run",
        });
    }
    configuration
}

/// The one destination of `destination_type` the newest record of `name`'s history reports,
/// once it has been processed — polled for, because the run that processes it is owed by the
/// clock.
async fn processed_destination(
    emulator: &Emulator,
    case: &str,
    name: &str,
    destination_type: &str,
) -> Value {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let page = emulator
            .logs_ok(
                case,
                HISTORY,
                &json!({"identifier": name, "startTime": 0, "endTime": now_ms() / 1_000 + 3_600}),
            )
            .await;
        let processed = page["triggerHistory"]
            .as_array()
            .and_then(|records| records.first())
            .and_then(|record| record["destinations"].as_array())
            .and_then(|destinations| {
                destinations
                    .iter()
                    .find(|destination| {
                        destination["destinationType"].as_str() == Some(destination_type)
                    })
                    .cloned()
            })
            .filter(|destination| destination["status"].as_str() != Some("IN_PROGRESS"));
        if let Some(processed) = processed {
            return processed;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the {destination_type} destination of {name} to be \
             processed, got {page}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A run delivers its rows to each destination its query names, and the history reports a
/// destination as processed only once the delivery happened.
#[tokio::test]
async fn a_runs_results_are_delivered_to_every_destination_the_query_names() {
    let emulator = Emulator::new();
    seed_events(&emulator, "seed").await;
    emulator.create_bucket("seed", BUCKET).await;

    let rows: [Row; 4] = [
        Row {
            case: "a lookup-table destination",
            name: "rows-to-a-lookup-table",
            table: Some("scheduled_query_rows"),
            bucket: None,
            s3_status: None,
        },
        Row {
            case: "an Amazon S3 destination",
            name: "rows-to-amazon-s3",
            table: None,
            bucket: Some(BUCKET),
            s3_status: Some("COMPLETE"),
        },
        Row {
            case: "both destinations at once",
            name: "rows-to-both",
            table: Some("scheduled_query_rows_both"),
            bucket: Some(BUCKET),
            s3_status: Some("COMPLETE"),
        },
        Row {
            case: "an Amazon S3 destination the object store does not hold",
            name: "rows-to-a-missing-bucket",
            table: None,
            bucket: Some(MISSING_BUCKET),
            s3_status: Some("FAILED"),
        },
    ];

    for row in &rows {
        let case = row.case;
        emulator
            .logs_ok(
                case,
                CREATE,
                &json!({
                    "name": row.name,
                    "queryLanguage": "CWLI",
                    "queryString": QUERY_STRING,
                    "scheduleExpression": "rate(1 day)",
                    "executionRoleArn": EXECUTION_ROLE,
                    "logGroupIdentifiers": [GROUP],
                    "startTimeOffset": 7_200,
                    "destinationConfiguration": destination_configuration(row),
                }),
            )
            .await;

        if let Some(table) = row.table {
            let destination = processed_destination(&emulator, case, row.name, "LOOKUP_TABLE").await;
            assert_eq!(
                destination["status"].as_str(),
                Some("COMPLETE"),
                "case {case:?}: the lookup table was refreshed, got {destination}"
            );
            assert_eq!(
                destination["destinationIdentifier"].as_str(),
                Some(table),
                "case {case:?}: the destination names the table it delivers to, got {destination}"
            );
            // "The identifier of the processed result at the destination": the table the rows
            // were written to, which really holds them.
            let arn = destination["processedIdentifier"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a processed delivery names what it produced, got {destination}")
                })
                .to_string();
            let held = emulator
                .logs_ok(case, GET_LOOKUP_TABLE, &json!({"lookupTableArn": arn}))
                .await;
            assert_eq!(
                held["lookupTableName"].as_str(),
                Some(table),
                "case {case:?}: the delivered table is the one the destination names, got {held}"
            );
            let body = held["tableBody"].as_str().unwrap_or_default().to_string();
            for message in MESSAGES.iter().filter(|message| message.contains("ERROR")) {
                assert!(
                    body.contains(message),
                    "case {case:?}: expected the delivered rows to hold {message:?}, got {body:?}"
                );
            }
            assert!(
                !body.contains("INFO order 2 accepted"),
                "case {case:?}: the delivered rows are the ones the query produced, got {body:?}"
            );
            // The delivered body is a header row naming the query's fields plus one row per
            // result, so what it holds is exactly the rows the run produced.
            let data_rows = body.lines().skip(1).filter(|line| !line.is_empty()).count();
            assert_eq!(
                i64::try_from(data_rows).unwrap_or_default(),
                SELECTED_ROWS,
                "case {case:?}: expected {SELECTED_ROWS} delivered rows, got {body:?}"
            );
        }

        let Some(expected_status) = row.s3_status else {
            continue;
        };
        let destination = processed_destination(&emulator, case, row.name, "S3").await;
        assert_eq!(
            destination["status"].as_str(),
            Some(expected_status),
            "case {case:?}: expected the Amazon S3 delivery to be {expected_status}, got \
             {destination}"
        );
        if expected_status == "FAILED" {
            assert!(
                destination["errorMessage"].as_str().is_some(),
                "case {case:?}: a delivery that did not happen says why, got {destination}"
            );
            assert!(
                destination.get("processedIdentifier").is_none(),
                "case {case:?}: a delivery that did not happen produced nothing, got \
                 {destination}"
            );
            continue;
        }
        // The object the delivery produced is one the OBJECT STORE holds, read back through
        // S3 itself at the key the destination says it wrote.
        let uri = destination["processedIdentifier"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: a processed delivery names what it produced, got {destination}")
            })
            .to_string();
        let key = uri
            .strip_prefix(&format!("s3://{BUCKET}/"))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the delivered object is in {BUCKET}, got {uri:?}")
            })
            .to_string();
        let keys = emulator.keys_under(case, BUCKET, "history").await;
        assert!(
            keys.contains(&key),
            "case {case:?}: expected the object store to hold {key:?}, got {keys:?}"
        );
    }
}
