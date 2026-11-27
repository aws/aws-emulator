// SPDX-License-Identifier: CC0-1.0

//! What a BULK OPERATION reports about itself, member by member.
//!
//! RULE under test: the record a bulk put writes is reported through the members the model
//! declares for it (`smithy_query(query="closure", operation="StartBulkPutItems")`) — the
//! operation's own ARN and status, the `TableDetails` of the table it acted on (its name and
//! that table's own `TableId`), the instants it ran between, and the figures its task
//! settled: `ProcessedItemCount`, `FailedItemCount`, the input bytes it read
//! (`ProcessedInputBytes` and the source's `TotalInputBytes`) and the `BulkOperationUsage`
//! of what it wrote. The status it SETTLES into is the outcome of its task: a source every
//! record of which landed completes, a source some record of which could not be written
//! completes WITH ERRORS, and a source that could not be read at all fails and reports why.
//! A member the model does not declare is reported by nothing.
//!
//! The rows span what the rule generalises over: WHAT the source carried (records the table
//! holds, a record it cannot hold, no readable source at all), and so which terminal status
//! the task settles into and which figures it reports.
//!
//! Every figure is read back through `DescribeBulkOperation` — the reader a client watches an
//! operation with — rather than off the answer to the call that started it, and the table
//! identity it reports is read back through `DescribeTable`, so the two readers cannot
//! disagree about one table. The transition the clock owes is observed by POLLING for the
//! condition under a generous deadline, never by sleeping a fixed time.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{create_bucket, emulator_with_s3, ok, put_object, POLL_INTERVAL};
use crate::table_items::create_active_table;

/// The bucket every row stages its source in.
const BUCKET: &str = "bulk-operation-reported-members";

/// The table every row writes into: one string partition key.
const TABLE: &str = "BulkOperationReportedMembers";

/// How long a poll for the clock-owed terminal status may take before the test calls it a
/// defect: generously more than the span itself, so a loaded or slow host is not one.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// One source object's bytes in the DynamoDB JSON form a table export writes: one record per
/// line, each under the `Item` member.
fn dynamodb_json_lines(records: &[Value]) -> Vec<u8> {
    records
        .iter()
        .map(|record| format!("{}\n", json!({"Item": record})))
        .collect::<String>()
        .into_bytes()
}

/// One `StartBulkPutItems` body reading the source staged under `prefix` of `bucket`.
fn start_body(bucket: &str, prefix: &str) -> Value {
    json!({
        "TableName": TABLE,
        "InputS3Configuration": {
            "S3Bucket": bucket,
            "S3KeyPrefix": prefix,
            "InputFormat": "DYNAMODB_JSON",
        },
        "OutputS3Configuration": {
            "S3Bucket": BUCKET,
            "S3KeyPrefix": "outcomes/",
            "OutputType": ["ERROR"],
        },
    })
}

/// The description `DescribeBulkOperation` reports for `arn`.
async fn described(router: &MultiServiceRouter, arn: &str) -> Value {
    let answered = ok(
        router,
        "DescribeBulkOperation",
        json!({"BulkOperationArn": arn}),
    )
    .await;
    answered["BulkOperationDescription"].clone()
}

/// The description once the operation has reached a terminal status, or a failure naming the
/// last one it reported.
async fn settled(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let read = described(router, arn).await;
        let status = read["BulkOperationStatus"].as_str().unwrap_or_default();
        if status != "IN_PROGRESS" && status != "CANCELLING" {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the operation to settle, last {read}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One row: what the source carried, and what the task it ran settles into.
struct Row {
    case: &'static str,
    /// The bucket the call reads, which one row points at a bucket nothing holds.
    bucket: &'static str,
    /// The key prefix the source is staged under.
    prefix: &'static str,
    /// The records staged there, in the form the source carries them.
    staged: Vec<Value>,
    /// The `BulkOperationStatus` the operation settles into.
    settles_into: &'static str,
    /// The `ProcessedItemCount` and `FailedItemCount` it reports once settled.
    processed: i64,
    failed: i64,
}

#[tokio::test]
async fn a_bulk_operation_reports_the_members_the_model_declares_for_it() {
    let router = emulator_with_s3();
    create_bucket(&router, BUCKET).await;
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;

    // The `TableId` the table reports for itself, which the operation's `TableDetails` must
    // name beside the table's own name.
    let table = ok(&router, "DescribeTable", json!({"TableName": TABLE})).await;
    let table_id = table["Table"]["TableId"].clone();
    assert!(
        table_id.is_string(),
        "the table reports an id of its own, got {table}"
    );

    let rows = vec![
        Row {
            case: "a source every record of which the table holds",
            bucket: BUCKET,
            prefix: "landed/",
            staged: vec![
                json!({"pk": {"S": "landed-0"}, "note": {"S": "one"}}),
                json!({"pk": {"S": "landed-1"}, "note": {"S": "two"}}),
            ],
            settles_into: "COMPLETED",
            processed: 2,
            failed: 0,
        },
        Row {
            case: "a source carrying a record the table's key schema cannot key",
            bucket: BUCKET,
            prefix: "partly/",
            staged: vec![
                json!({"pk": {"S": "partly-0"}}),
                json!({"keyless": {"S": "no partition key at all"}}),
            ],
            settles_into: "COMPLETED_WITH_ERRORS",
            processed: 2,
            failed: 1,
        },
        Row {
            case: "a source bucket the object store holds none of",
            bucket: "bulk-operation-reported-members-absent",
            prefix: "absent/",
            staged: Vec::new(),
            settles_into: "FAILED",
            processed: 0,
            failed: 0,
        },
    ];

    for row in rows {
        let Row {
            case,
            bucket,
            prefix,
            staged,
            settles_into,
            processed,
            failed,
        } = row;
        if !staged.is_empty() {
            put_object(
                &router,
                BUCKET,
                &format!("{prefix}data.json"),
                dynamodb_json_lines(&staged),
            )
            .await;
        }

        let started = ok(&router, "StartBulkPutItems", start_body(bucket, prefix)).await
            ["BulkOperationDescription"]
            .clone();
        let arn = started["BulkOperationArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // While it RUNS: the operation names the table it acts on and reports no figure of a
        // task that has not finished — and no end for work that is not over.
        assert_eq!(
            started["TableDetails"],
            json!([{"TableName": TABLE, "TableId": table_id}]),
            "case {case:?}: expected the table's name and its own id, got {started}"
        );
        assert_eq!(
            (
                started["BulkOperationStatus"].clone(),
                started["ProcessedItemCount"].clone(),
                started["FailedItemCount"].clone(),
                started.get("EndTime").cloned(),
                started.get("TotalInputBytes").cloned(),
            ),
            (json!("IN_PROGRESS"), json!(0), json!(0), None, None),
            "case {case:?}: a running operation reports no figure and no end, got {started}"
        );
        assert!(
            started["StartTime"].is_number(),
            "case {case:?}: a running operation reports the instant it began, got {started}"
        );

        // Once SETTLED: the status its task reached, the counts it settled on, the source
        // bytes it read and the bytes it wrote.
        let read = settled(&router, case, &arn).await;
        assert_eq!(
            (
                read["BulkOperationStatus"].clone(),
                read["ProcessedItemCount"].clone(),
                read["FailedItemCount"].clone(),
            ),
            (json!(settles_into), json!(processed), json!(failed)),
            "case {case:?}: expected {settles_into} with {processed} processed and {failed} \
             failed, got {read}"
        );
        assert!(
            read["EndTime"].as_f64() >= read["StartTime"].as_f64(),
            "case {case:?}: a settled operation ends no earlier than it began, got {read}"
        );
        // The source's size is reported once it is known, and what was read is what there
        // was: this emulator applies the whole source under the call that started it.
        assert_eq!(
            read["ProcessedInputBytes"], read["TotalInputBytes"],
            "case {case:?}: the bytes read are the bytes the source held, got {read}"
        );
        let staged_bytes = i64::try_from(dynamodb_json_lines(&staged).len()).unwrap_or(i64::MAX);
        assert_eq!(
            read["ProcessedInputBytes"],
            json!(staged_bytes),
            "case {case:?}: expected the staged source's own size, got {read}"
        );
        // What it WROTE, metered per table it wrote into — one, since a bulk put addresses
        // one table — and nothing at all for a task that wrote nothing.
        let wrote = processed - failed > 0;
        let usage = read["BulkOperationUsage"][0].clone();
        assert_eq!(
            usage["TableName"],
            json!(TABLE),
            "case {case:?}: the usage names the table the writes landed in, got {read}"
        );
        assert_eq!(
            usage["BulkPutBytes"], usage["Table"]["BulkPutBytes"],
            "case {case:?}: one metered figure is reported under both names, got {read}"
        );
        assert_eq!(
            usage["BulkPutBytes"].as_i64().unwrap_or(-1) > 0,
            wrote,
            "case {case:?}: expected bytes to be reported exactly for a task that wrote, got \
             {read}"
        );
        // A FAILED task reports WHY, and no other status does: "DynamoDB returns this value
        // only when BulkOperationStatus is FAILED or CANCELLED"
        // (`BulkOperationDescription$FailureCode`).
        let failure = read.get("FailureCode").cloned();
        if settles_into == "FAILED" {
            assert_eq!(
                failure,
                Some(json!("S3NoSuchBucket")),
                "case {case:?}: expected the reason the source could not be read, got {read}"
            );
            assert!(
                read["FailureMessage"].is_string(),
                "case {case:?}: a failure states its reason in words too, got {read}"
            );
        } else {
            assert_eq!(
                failure, None,
                "case {case:?}: a task that did not fail reports no failure, got {read}"
            );
        }
        // The members the model does NOT declare are reported by nothing: the table an
        // operation acted on travels in `TableDetails` and the records it could not write in
        // `FailedItemCount`.
        for undeclared in ["TableArn", "ErrorCount"] {
            assert_eq!(
                read.get(undeclared),
                None,
                "case {case:?}: {undeclared} is no member of the description, got {read}"
            );
        }

        // And the listing reports the same operation with the members a summary declares,
        // and with none of the description's own.
        let page = ok(&router, "ListBulkOperations", json!({})).await;
        let summary = page["BulkOperationsSummaryList"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|summary| summary["BulkOperationArn"] == json!(arn))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            (
                summary["BulkOperationStatus"].clone(),
                summary["StartTime"].clone(),
                summary["EndTime"].clone(),
            ),
            (
                read["BulkOperationStatus"].clone(),
                read["StartTime"].clone(),
                read["EndTime"].clone(),
            ),
            "case {case:?}: the two readers report one operation, got {summary} and {read}"
        );
        for undeclared in ["TableArn", "TableDetails", "ProcessedItemCount"] {
            assert_eq!(
                summary.get(undeclared),
                None,
                "case {case:?}: {undeclared} is no member of a summary, got {summary}"
            );
        }
    }
}
