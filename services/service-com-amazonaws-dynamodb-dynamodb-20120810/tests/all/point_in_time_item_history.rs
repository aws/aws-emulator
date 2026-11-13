// SPDX-License-Identifier: CC0-1.0

//! What a read of a table AS OF a past instant answers: the table as it stood then.
//!
//! The rule pinned here: an operation that reads a table at a point in time inside its
//! restorable window — `RestoreTableToPointInTime`, and the snapshot
//! `ExportTableToPointInTime` renders — reports the items the table held AT that instant,
//! not the ones it holds now. So an item written before the point is there with the value
//! it carried then, an item written after it is absent, an update made after it is undone,
//! and a delete made after it is undone too. The accepted restore echoes the instant it
//! was asked for as `RestoreSummary.RestoreDateTime`.
//!
//! Point-in-time recovery is what makes such a point readable at all ("Point in time
//! recovery has not yet been enabled for this source table" —
//! `PointInTimeRecoveryUnavailableException`), so the window these suites restore inside
//! starts where `UpdateContinuousBackups` turned recovery on, which is the instant
//! `DescribeContinuousBackups` reports as `EarliestRestorableDateTime`.
//!
//! Every call is driven over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use. The rule is asserted over a TABLE
//! of item histories, whose rows differ in WHAT happened to the item after the restore
//! point (nothing, an update, a delete, a delete and a rewrite, a first write) and in
//! whether the item existed before it at all — and over both ways a request names the
//! point (`RestoreDateTime`, `UseLatestRestorableTime`).

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{
    create_bucket, describe_when_active, emulator, emulator_with_s3, ok, restorable_window, REGION,
};

/// How long a poll for a terminal export status may take before this suite calls it a
/// hang: generous slack over the emulator's transition span, because the point of the
/// bound is that the wait ENDS rather than how fast it does.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(10);

/// How long each poll waits before asking again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Wait until this process's clock has passed `seconds` — an instant as the restorable
/// window reports it — so a write made after this call is recorded strictly after that
/// instant, whatever resolution the host's clock has.
///
/// The wait is for that CONDITION and ends as soon as it holds (usually at once, because
/// the window's bound is the instant recovery was turned on); the deadline is only there so
/// a clock that never advances fails instead of hanging.
async fn past(seconds: f64) {
    let point = std::time::UNIX_EPOCH + Duration::from_secs_f64(seconds);
    let started = std::time::Instant::now();
    while std::time::SystemTime::now() <= point && started.elapsed() < TERMINAL_DEADLINE {
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One source table with one string partition key, waited `ACTIVE` so it serves writes:
/// "You can perform read and write operations only on an ACTIVE table".
async fn active_table(router: &MultiServiceRouter, case: &str, name: &str) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(router, case, name).await;
}

/// Turn point-in-time recovery on for `name`.
async fn enable_recovery(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "UpdateContinuousBackups",
        json!({
            "TableName": name,
            "PointInTimeRecoverySpecification": {"PointInTimeRecoveryEnabled": true},
        }),
    )
    .await;
}

/// Write one item carrying `value` under `pk`.
async fn put(router: &MultiServiceRouter, table: &str, pk: &str, value: &str) {
    ok(
        router,
        "PutItem",
        json!({"TableName": table, "Item": {"pk": {"S": pk}, "val": {"S": value}}}),
    )
    .await;
}

/// The item `pk` names in `table`, as a consistent read sees it — `Value::Null` for a key
/// that holds none.
async fn item(router: &MultiServiceRouter, table: &str, pk: &str) -> Value {
    let read = ok(
        router,
        "GetItem",
        json!({"TableName": table, "Key": {"pk": {"S": pk}}, "ConsistentRead": true}),
    )
    .await;
    read.get("Item").cloned().unwrap_or(Value::Null)
}

/// What happened to one item AFTER the restore point.
#[derive(Clone, Copy)]
enum After {
    /// Nothing at all: the item is untouched from before the point.
    Untouched,
    /// Overwritten with another value.
    Rewritten,
    /// Deleted.
    Deleted,
    /// Deleted and written again, with another value.
    DeletedAndRewritten,
    /// Written for the FIRST time: the key held nothing at the point.
    FirstWritten,
}

/// One row: one item's history across the restore point, and what the restored table must
/// hold for it.
struct Row {
    case: &'static str,
    pk: &'static str,
    /// The value the item carried BEFORE the point, where it existed then.
    before: Option<&'static str>,
    after: After,
}

/// The value every row writes after the point, so a restored table that copied the live
/// table is told apart from one materialised at the point.
const AFTER_VALUE: &str = "after-the-point";

/// The rows: one item per history the rule has to hold over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an item written before the point and never touched again",
            pk: "untouched",
            before: Some("before-the-point"),
            after: After::Untouched,
        },
        Row {
            case: "an item written before the point and overwritten after it",
            pk: "rewritten",
            before: Some("before-the-point"),
            after: After::Rewritten,
        },
        Row {
            case: "an item written before the point and deleted after it",
            pk: "deleted",
            before: Some("before-the-point"),
            after: After::Deleted,
        },
        Row {
            case: "an item written before the point, deleted and written again after it",
            pk: "deleted-and-rewritten",
            before: Some("before-the-point"),
            after: After::DeletedAndRewritten,
        },
        Row {
            case: "an item first written after the point",
            pk: "first-written",
            before: None,
            after: After::FirstWritten,
        },
    ]
}

/// The item each row must read back as from a table restored to the point: the value it
/// carried then, and nothing for a key that held nothing.
fn expected(row: &Row) -> Value {
    match row.before {
        Some(value) => json!({"pk": {"S": row.pk}, "val": {"S": value}}),
        None => Value::Null,
    }
}

/// Write every row's history: the values before the point, then — once `point_reached` has
/// been run — the changes after it.
async fn write_history<F, Fut>(router: &MultiServiceRouter, table: &str, point_reached: F)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    for row in rows() {
        if let Some(value) = row.before {
            put(router, table, row.pk, value).await;
        }
    }
    point_reached().await;
    for row in rows() {
        match row.after {
            After::Untouched => {}
            After::Rewritten | After::FirstWritten => {
                put(router, table, row.pk, AFTER_VALUE).await;
            }
            After::Deleted => {
                ok(
                    router,
                    "DeleteItem",
                    json!({"TableName": table, "Key": {"pk": {"S": row.pk}}}),
                )
                .await;
            }
            After::DeletedAndRewritten => {
                ok(
                    router,
                    "DeleteItem",
                    json!({"TableName": table, "Key": {"pk": {"S": row.pk}}}),
                )
                .await;
                put(router, table, row.pk, AFTER_VALUE).await;
            }
        }
    }
}

#[tokio::test]
async fn a_restore_materialises_the_table_as_it_stood_at_the_requested_instant() {
    // The two rows of this outer table are the two ways a request names its point: the
    // instant the window itself reports, and the newest restorable one. Both name the same
    // instant while the window's lag has not elapsed, so both must answer the same table.
    for (case, by_latest) in [("RestoreDateTime", false), ("UseLatestRestorableTime", true)] {
        let router = emulator();
        let source = format!("PitrHistory{}", usize::from(by_latest));
        let target = format!("PitrHistoryRestored{}", usize::from(by_latest));
        active_table(&router, case, &source).await;

        // Recovery is turned on BETWEEN the two halves of each item's history, so the
        // oldest restorable instant is the point every row is written around.
        let mut window = (0.0, 0.0);
        write_history(&router, &source, || async {
            enable_recovery(&router, &source).await;
            window = restorable_window(&router, &source).await;
            past(window.0).await;
        })
        .await;
        let (earliest, latest) = window;
        assert!(
            earliest > 0.0 && latest >= earliest,
            "case {case:?}: the table reports a restorable window, got ({earliest}, {latest})"
        );

        let mut request = json!({"SourceTableName": &source, "TargetTableName": &target});
        if by_latest {
            request["UseLatestRestorableTime"] = json!(true);
        } else {
            request["RestoreDateTime"] = json!(earliest);
        }
        let restored = ok(&router, "RestoreTableToPointInTime", request.clone()).await
            ["TableDescription"]
            .clone();
        let asked = if by_latest { latest } else { earliest };
        assert_eq!(
            restored["RestoreSummary"]["RestoreDateTime"].as_f64(),
            Some(asked),
            "case {case:?}: the restore echoes the instant it was asked for, got {restored}"
        );
        describe_when_active(&router, case, &target).await;

        for row in rows() {
            let read = item(&router, &target, row.pk).await;
            assert_eq!(
                read,
                expected(&row),
                "case {case:?} / {}: the restored table must hold the item as it stood at \
                 the restore point, got {read}",
                row.case
            );
            // The SOURCE table is untouched by the restore: it still holds what the
            // writes after the point left.
            let live = item(&router, &source, row.pk).await;
            let expected_live = match row.after {
                After::Untouched => expected(&row),
                After::Deleted => Value::Null,
                After::Rewritten | After::DeletedAndRewritten | After::FirstWritten => {
                    json!({"pk": {"S": row.pk}, "val": {"S": AFTER_VALUE}})
                }
            };
            assert_eq!(
                live, expected_live,
                "case {case:?} / {}: the source table keeps what was written after the \
                 point, got {live}",
                row.case
            );
        }

        // Nothing written after the point is in the restored table at all, whichever key
        // carried it.
        let scanned = ok(
            &router,
            "Scan",
            json!({"TableName": &target, "ConsistentRead": true}),
        )
        .await;
        let later: Vec<Value> = scanned["Items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| item["val"] == json!({"S": AFTER_VALUE}))
            .collect();
        assert!(
            later.is_empty(),
            "case {case:?}: the restored table holds no value written after the point, got \
             {scanned}"
        );
        assert_eq!(
            scanned["Count"].as_i64().unwrap_or_default(),
            i64::try_from(rows().iter().filter(|row| row.before.is_some()).count())
                .expect("the table is sized"),
            "case {case:?}: the restored table holds exactly the items of the restore \
             point, got {scanned}"
        );
    }
}

#[tokio::test]
async fn an_export_reports_the_items_of_the_point_in_time_it_is_taken_at() {
    let case = "an export at the oldest restorable point";
    let router = emulator_with_s3();
    create_bucket(&router, "pitr-export-snapshots").await;
    let source = "PitrExported";
    active_table(&router, case, source).await;

    let mut window = (0.0, 0.0);
    write_history(&router, source, || async {
        enable_recovery(&router, source).await;
        window = restorable_window(&router, source).await;
        past(window.0).await;
    })
    .await;
    let (earliest, _) = window;

    let started = ok(
        &router,
        "ExportTableToPointInTime",
        json!({
            "TableArn": service_com_amazonaws_dynamodb_dynamodb_20120810::state::table_arn(
                REGION, source,
            ),
            "S3Bucket": "pitr-export-snapshots",
            "ExportTime": earliest,
        }),
    )
    .await;
    let arn = started["ExportDescription"]["ExportArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let at_the_point = i64::try_from(rows().iter().filter(|row| row.before.is_some()).count())
        .expect("the table is sized");
    assert_eq!(
        started["ExportDescription"]["ExportTime"].as_f64(),
        Some(earliest),
        "case {case:?}: the export is taken at the instant it was asked for, got {started}"
    );

    // The figures are reported once the task has written its snapshot, and they are the
    // figures of the point in time rather than of the live table.
    let polling = std::time::Instant::now();
    let mut reported = Value::Null;
    while polling.elapsed() < TERMINAL_DEADLINE {
        let described = ok(&router, "DescribeExport", json!({"ExportArn": &arn})).await;
        reported = described["ExportDescription"].clone();
        if reported["ExportStatus"] != json!("IN_PROGRESS") {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    assert_eq!(
        (
            reported["ExportStatus"].clone(),
            reported["ItemCount"].as_i64(),
        ),
        (json!("COMPLETED"), Some(at_the_point)),
        "case {case:?}: the finished export reports the items it wrote, got {reported}"
    );
}
