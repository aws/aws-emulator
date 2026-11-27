// SPDX-License-Identifier: CC0-1.0

//! What EventBridge Scheduler publishes ABOUT the invocations it makes: end to end over
//! the wire through the real `MultiServiceRouter`, with Amazon SQS, IAM and CloudWatch
//! mounted beside EventBridge Scheduler on ONE engine — so each count is asserted where a
//! client reads it, through CloudWatch's own `GetMetricStatistics`.
//!
//! RULE: an occurrence is performed with no client action at all and leaves the schedule's
//! own configuration untouched however it goes, so what tells a fleet of schedules that is
//! firing from one that is failing is the series the service publishes about itself: in the
//! [`METRIC_NAMESPACE`] namespace, dimensioned on the SCHEDULE GROUP the schedule belongs
//! to, one observation per EVENT — an attempt actually made
//! ([`INVOCATION_ATTEMPT_COUNT`]), a target call that failed ([`TARGET_ERROR_COUNT`]), an
//! invocation no further attempt will be made of ([`INVOCATION_DROPPED_COUNT`]) and the
//! dead-letter write, counted as made ([`INVOCATIONS_SENT_TO_DEAD_LETTER_COUNT`]) or as
//! not made ([`INVOCATIONS_FAILED_TO_BE_SENT_TO_DEAD_LETTER_COUNT`]). A counter nothing of
//! its kind happened for holds nothing rather than an observation this service never made.
//!
//! The rows span the class rather than one recorded case: they differ along what becomes
//! of the occurrence — delivered, refused and reported, refused with the report withheld
//! too — each in its own schedule group, which is what shows the counts are keyed by that
//! dimension and not pooled across the account.
//!
//! Every wait here looks again until the counts it needs hold, on a generous deadline, and
//! a counter expected to stay quiet is an invariant: it is looked at repeatedly across its
//! own window rather than once at the end of a sleep. Nothing is asserted about elapsed
//! time.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_scheduler_awschronosservice::state::schedule_metrics::{
    INVOCATIONS_FAILED_TO_BE_SENT_TO_DEAD_LETTER_COUNT, INVOCATIONS_SENT_TO_DEAD_LETTER_COUNT,
    INVOCATION_ATTEMPT_COUNT, INVOCATION_DROPPED_COUNT, METRIC_NAMESPACE,
    SCHEDULE_GROUP_DIMENSION, TARGET_ERROR_COUNT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::schedule_invocation::{
    at_expression_body, given_execution_role_for, given_queue, peek, scheduler, seconds_from_now,
    POLL_INTERVAL, REGION,
};

/// Every counter this service publishes, so a row asserts the ones it expects to move AND
/// the ones it expects to stay quiet.
const COUNTERS: &[&str] = &[
    INVOCATION_ATTEMPT_COUNT,
    TARGET_ERROR_COUNT,
    INVOCATION_DROPPED_COUNT,
    INVOCATIONS_SENT_TO_DEAD_LETTER_COUNT,
    INVOCATIONS_FAILED_TO_BE_SENT_TO_DEAD_LETTER_COUNT,
];

/// How long a row waits for its counts to hold. Every wait looks again until they do, so
/// this bounds only how long a row the service never published for takes to fail: it is
/// generous rather than tight, because a host running every suite at once is slow at
/// everything.
const PATIENCE: Duration = Duration::from_secs(90);

/// How long a row keeps looking at the counters it expects to stay quiet, once the ones it
/// expects have moved.
const QUIET_WINDOW: Duration = Duration::from_secs(5);

/// How far ahead of the write each row states its one occurrence: far enough that every
/// row's queue, role, group and schedule is written while the occurrence is still ahead.
const OCCURRENCE_IN: u64 = 6;

/// How wide a window the statistics read asks about, in seconds: wide enough on both sides
/// of the run that nothing the suite published falls outside it.
const READ_WINDOW: f64 = 600.0;

/// The period the read lays over that window, in seconds: the one-minute period a
/// regular-resolution series is stored at (`MetricDatum$StorageResolution`).
const READ_PERIOD: i64 = 60;

/// What the row's execution role may write to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grants {
    /// The target queue, so the invocation is delivered.
    Target,
    /// The dead-letter queue only, so the invocation is refused and reported.
    DeadLetterOnly,
    /// Neither, so the invocation is refused and the report cannot be carried either.
    Neither,
}

/// One row: the schedule group it runs in, what its role may write to, and the count each
/// counter must hold afterwards.
struct Row {
    case: &'static str,
    group: &'static str,
    grants: Grants,
    /// The counters that must MOVE, each with the sum it must reach.
    moved: &'static [(&'static str, f64)],
}

const ROWS: &[Row] = &[
    Row {
        case: "an invocation the target accepted",
        group: "delivered",
        grants: Grants::Target,
        moved: &[(INVOCATION_ATTEMPT_COUNT, 1.0)],
    },
    Row {
        case: "an invocation the execution role withheld, reported to the dead-letter queue",
        group: "reported",
        grants: Grants::DeadLetterOnly,
        moved: &[
            (INVOCATION_ATTEMPT_COUNT, 1.0),
            (TARGET_ERROR_COUNT, 1.0),
            (INVOCATION_DROPPED_COUNT, 1.0),
            (INVOCATIONS_SENT_TO_DEAD_LETTER_COUNT, 1.0),
        ],
    },
    Row {
        case: "an invocation whose report the execution role may not carry either",
        group: "unreported",
        grants: Grants::Neither,
        moved: &[
            (INVOCATION_ATTEMPT_COUNT, 1.0),
            (TARGET_ERROR_COUNT, 1.0),
            (INVOCATION_DROPPED_COUNT, 1.0),
            (INVOCATIONS_FAILED_TO_BE_SENT_TO_DEAD_LETTER_COUNT, 1.0),
        ],
    },
];

/// Every row: the counts its occurrence came to are published under its own schedule
/// group, and every other counter of that group holds nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_occurrence_publishes_what_it_came_to() {
    let emulator = emulator_with_cloudwatch();
    let (dead_letter_url, dead_letter_arn) =
        given_queue(&emulator, "the suite", "metrics-dlq").await;
    let (_, target_arn) = given_queue(&emulator, "the suite", "metrics-target").await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let granted: Vec<&str> = match row.grants {
            Grants::Target => vec![target_arn.as_str()],
            Grants::DeadLetterOnly => vec![dead_letter_arn.as_str()],
            Grants::Neither => vec![],
        };
        let role =
            given_execution_role_for(&emulator, case, &format!("metrics-role-{index}"), &granted)
                .await;
        let (status, created) = scheduler(
            &emulator,
            "POST",
            &format!("/schedule-groups/{}", row.group),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateScheduleGroup answered {status} {created}"
        );
        let (status, created) = scheduler(
            &emulator,
            "POST",
            &format!("/schedules/metrics-{index}"),
            Some(json!({
                "GroupName": row.group,
                "ScheduleExpression": format!(
                    "at({})",
                    at_expression_body(seconds_from_now(OCCURRENCE_IN))
                ),
                "ScheduleExpressionTimezone": "UTC",
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "State": "ENABLED",
                "Target": {
                    "Arn": target_arn,
                    "RoleArn": role,
                    "Input": json!({ "row": index }).to_string(),
                    "RetryPolicy": { "MaximumRetryAttempts": 0 },
                    "DeadLetterConfig": { "Arn": dead_letter_arn },
                },
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateSchedule answered {status} {created}"
        );
    }

    // Every row's counters are waited for together, so one slow occurrence does not spend
    // another row's patience.
    let deadline = Instant::now() + PATIENCE;
    loop {
        let mut pending: Vec<String> = Vec::new();
        for row in ROWS {
            for (metric, expected) in row.moved {
                let held = sum_of(&emulator, row.group, metric).await;
                if held < *expected {
                    pending.push(format!("{}/{metric}={held}", row.group));
                }
            }
        }
        if pending.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "expected every counter of every row to be published; still short: {pending:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // What each row's occurrence came to is EXACTLY these counts: a counter it did not
    // move holds nothing at all, which is looked at repeatedly rather than once.
    let deadline = Instant::now() + QUIET_WINDOW;
    loop {
        for row in ROWS {
            let case = row.case;
            for metric in COUNTERS {
                let expected = row
                    .moved
                    .iter()
                    .find(|(moved, _)| moved == metric)
                    .map_or(0.0, |(_, count)| *count);
                let held = sum_of(&emulator, row.group, metric).await;
                assert!(
                    (held - expected).abs() < f64::EPSILON,
                    "case {case:?}: expected {metric} of group {:?} to hold {expected}, got \
                     {held}",
                    row.group
                );
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // The positive control for the two rows a report was expected of: the dead-letter
    // queue holds exactly the reports the counters claim were written.
    let reports = peek(&emulator, &dead_letter_url).await.len();
    assert_eq!(
        reports, 1,
        "expected exactly the one report the counters claim was written, got {reports}"
    );
}

/// The `Sum` of every datapoint CloudWatch holds for one group's `metric`, read exactly as
/// a client reads it. A series that does not exist holds nothing, which reads as 0.
pub(crate) async fn sum_of(router: &MultiServiceRouter, group: &str, metric: &str) -> f64 {
    sum_in(
        router,
        METRIC_NAMESPACE,
        metric,
        SCHEDULE_GROUP_DIMENSION,
        group,
    )
    .await
}

/// The same read, of any ONE series: the `Sum` of every datapoint CloudWatch holds for
/// `metric` of `namespace` on the single dimension `(dimension, value)`.
///
/// The one reader of a published series this test crate keeps: a suite also reads what
/// ANOTHER service published about the call an occurrence made to it — the function error
/// a Lambda target's own run came to — which differs from the read above only in the
/// series it names.
pub(crate) async fn sum_in(
    router: &MultiServiceRouter,
    namespace: &str,
    metric: &str,
    dimension: &str,
    value: &str,
) -> f64 {
    let now = epoch_seconds();
    let (status, answered) = cloudwatch(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": namespace,
            "MetricName": metric,
            "Dimensions": [{ "Name": dimension, "Value": value }],
            "StartTime": now - READ_WINDOW,
            "EndTime": now + READ_WINDOW,
            "Period": READ_PERIOD,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetMetricStatistics for {namespace}/{metric} of {dimension}={value:?} answered \
         {status} {answered}"
    );
    answered["Datapoints"]
        .as_array()
        .map(|points| {
            points
                .iter()
                .filter_map(|point| point["Sum"].as_f64())
                .sum::<f64>()
        })
        .unwrap_or_default()
}

/// One CloudWatch call, over the `awsJson` front door that service speaks.
async fn cloudwatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let signing_name = "monitoring";
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("GraniteServiceVersion20100801.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Epoch seconds, for the window a statistics read asks about.
fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// The emulator: EventBridge Scheduler, the service its targets live in, IAM, and the
/// CloudWatch whose store the counters land in — on ONE engine, the shape
/// `core/router/src/lib.rs` mounts them in.
pub(crate) fn emulator_with_cloudwatch() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_scheduler_awschronosservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}
