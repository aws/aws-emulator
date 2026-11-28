// SPDX-License-Identifier: CC0-1.0

//! A metric alarm's transition, and the SNS action that transition fires, are owed a SPAN after
//! the write that configured the alarm — not performed inside the client's own write sequence —
//! through the WHOLE mounted service set, the one `core/router/src/lib.rs` assembles for the
//! binary.
//!
//! The RULE under test: **the state a metric alarm's series implies is not due at the instant of
//! the write — "When this operation creates an alarm, the alarm state is immediately set to
//! INSUFFICIENT_DATA. The alarm is then evaluated and its state is set appropriately."
//! (`smithy_query(query="operation",operation="PutMetricAlarm")`), and what the alarm is then
//! evaluated on is its own cadence, "The length, in seconds, used each time the metric specified
//! in MetricName is evaluated […] the alarm still attempts to gather data at the period rate that
//! you specify" (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Period")`) — so a
//! client that writes an alarm over a series that is ALREADY breaching, finishes writing its
//! estate, clears the action's target of whatever was waiting there and only THEN begins watching
//! it still receives the notification that transition fires, because the transition had not
//! happened yet while it was writing.**
//!
//! Three services have to agree for that to hold — CloudWatch evaluates and publishes, SNS fans
//! out, SQS holds what arrived — and the publish CloudWatch makes is an in-process call addressed
//! the way a client's own `sns`-scoped call is, so WHICH mounted model claims it is only decidable
//! with every service mounted. The sibling suites drive transitions a client FORCES
//! (`cloudwatch_alarm_action_reaches_its_subscribed_queue.rs`) or a series that holds no datapoint
//! at all (`cloudwatch_breaching_alarm_fires_through_the_whole_router.rs`); what only this suite
//! pins is WHEN the transition of an already-breaching series becomes observable: an action
//! executed inside the write's own round trips reaches a target nobody is watching yet, and
//! nothing ever publishes it again.
//!
//! The rows differ along what the rule generalises over: the width and resolution of the periods
//! the series is graded over, the statistic those periods are folded by, how many of them must
//! breach, how the subscribed queue's policy scopes its grant to SNS, and whether a warm-up gated
//! arm shares the series and the topic with the control.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{
    cw, drain, given_topic_with_subscriber, router, Grant,
};

/// The Region the whole wiring of every row lives in.
const REGION: &str = "us-west-2";

/// The namespace every row publishes into, each row under a metric name of its own so no row's
/// series is another's.
const NAMESPACE: &str = "Custom/OwedSpan";

/// The value every row publishes and the threshold its alarms compare it against: one datapoint
/// of the series breaches.
const BREACHING_VALUE: f64 = 100.0;
const THRESHOLD: f64 = 10.0;

/// The `EvaluationState` an alarm inside its warm-up window reports.
const IN_WARM_UP: &str = "IN_WARM_UP";

/// How far back the series is already breaching when the alarm is written: one minute, the
/// granularity a regular-resolution datapoint is stored at ("Setting this to 60 specifies this
/// metric as regular resolution, which CloudWatch stores at 1-minute resolution",
/// `smithy_query(query="doc",shape="MetricDatum",member="StorageResolution")`), so the datapoint
/// falls in a period that has already closed for every row's grid.
const MINUTE_SECS: f64 = 60.0;

/// The longest warm-up the model admits, in minutes (`WarmUpConfiguration$WarmUpPeriodDurationInMinutes`
/// is `@range 1..2880`): a window that cannot pass while a row runs, so the gated arm's withholding
/// is the only thing its reading can be.
const LONGEST_WARM_UP_MINUTES: i64 = 2_880;

/// How long a row waits for the transition and the delivery it is owed, and how often it looks.
/// Generous on purpose: the wait is on the CONDITION, so a loaded host is given room rather than
/// failed.
const DELIVERY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

/// Wall-clock now, as the epoch seconds the evaluation windows are measured against.
fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the system clock is after the epoch")
        .as_secs_f64()
}

/// One row of the table: the alarm shape it writes, and the estate it writes it into.
struct Row {
    case: &'static str,
    /// The width of the periods the series is graded over, in seconds.
    period: i64,
    /// The resolution the datapoints are stored at (`MetricDatum$StorageResolution`).
    storage_resolution: i64,
    /// The statistic each period is folded by.
    statistic: &'static str,
    /// How many periods are evaluated, and how many of them must breach.
    evaluation_periods: i64,
    datapoints_to_alarm: i64,
    /// How the subscribed queue's policy grants SNS the send.
    grant: Grant,
    /// Whether a warm-up gated arm shares the series and the topic with the control.
    gated_arm: bool,
}

/// Publish one breaching datapoint of `metric`, stamped `at`.
async fn publish(router: &MultiServiceRouter, row: &Row, metric: &str, at: f64) {
    cw(
        router,
        row.case,
        REGION,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{
                "MetricName": metric,
                "Unit": "Count",
                "Value": BREACHING_VALUE,
                "Timestamp": at,
                "StorageResolution": row.storage_resolution,
            }],
        }),
    )
    .await;
}

/// Write one alarm of `row`'s shape over `metric`, acting on `topic`, carrying a warm-up window
/// when the arm is the gated one.
async fn put_alarm(
    router: &MultiServiceRouter,
    row: &Row,
    name: &str,
    metric: &str,
    topic: &str,
    warming_up: bool,
) {
    let mut request = json!({
        "AlarmName": name,
        "Namespace": NAMESPACE,
        "MetricName": metric,
        "Statistic": row.statistic,
        "Period": row.period,
        "EvaluationPeriods": row.evaluation_periods,
        "DatapointsToAlarm": row.datapoints_to_alarm,
        "Threshold": THRESHOLD,
        "ComparisonOperator": "GreaterThanThreshold",
        "ActionsEnabled": true,
        "AlarmActions": [topic],
    });
    if warming_up {
        request["WarmUpConfiguration"] = json!({
            "WarmUpPeriodDurationInMinutes": LONGEST_WARM_UP_MINUTES,
            "OnlyStartEvaluatingAfterWarmUpPeriodEnds": true,
        });
    }
    cw(router, row.case, REGION, "PutMetricAlarm", &request).await;
}

/// The alarm `name` as `DescribeAlarms` reports it.
async fn described(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let answered = cw(
        router,
        case,
        REGION,
        "DescribeAlarms",
        &json!({"AlarmNames": [name]}),
    )
    .await;
    answered["MetricAlarms"][0].clone()
}

/// The `Action` items the alarm `name`'s history holds, newest first.
async fn action_history(router: &MultiServiceRouter, case: &str, name: &str) -> Vec<Value> {
    let answered = cw(
        router,
        case,
        REGION,
        "DescribeAlarmHistory",
        &json!({
            "AlarmName": name,
            "HistoryItemType": "Action",
            "ScanBy": "TimestampDescending",
        }),
    )
    .await;
    answered["AlarmHistoryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Watch the queue at `url` until a notification naming `alarm` has arrived, re-publishing the
/// breaching datapoint each turn the way a client watching its alarm does — a wait for the
/// CONDITION, paced by the long poll each turn makes.
async fn until_notified(
    router: &MultiServiceRouter,
    row: &Row,
    metric: &str,
    url: &str,
    alarm: &str,
) -> Vec<Value> {
    let giving_up_at = std::time::Instant::now() + DELIVERY_DEADLINE;
    let mut arrived: Vec<Value> = Vec::new();
    loop {
        publish(router, row, metric, now_secs()).await;
        arrived.extend(drain(router, row.case, REGION, url).await);
        let named = arrived
            .iter()
            .any(|notification| notification["AlarmName"].as_str() == Some(alarm));
        if named || std::time::Instant::now() >= giving_up_at {
            return arrived;
        }
    }
}

/// RULE: the transition of an alarm written over an already breaching series, and the SNS action
/// it fires, are owed a span after that write — so a client that clears the target and only then
/// starts watching it still receives the notification.
#[tokio::test]
async fn an_already_breaching_alarms_action_is_owed_a_span_after_the_write() {
    let rows: &[Row] = &[
        Row {
            case: "a minute-wide maximum, one period of one, an unscoped grant",
            period: 60,
            storage_resolution: 60,
            statistic: "Maximum",
            evaluation_periods: 1,
            datapoints_to_alarm: 1,
            grant: Grant::Unscoped,
            gated_arm: false,
        },
        Row {
            case: "a minute-wide average, one of three periods breaching, the grant scoped to \
                   the publishing topic, a warm-up gated arm beside the control",
            period: 60,
            storage_resolution: 60,
            statistic: "Average",
            evaluation_periods: 3,
            datapoints_to_alarm: 1,
            grant: Grant::ScopedToTopic,
            gated_arm: true,
        },
        Row {
            case: "a high-resolution ten-second sum, one period of one, an unscoped grant",
            period: 10,
            storage_resolution: 1,
            statistic: "Sum",
            evaluation_periods: 1,
            datapoints_to_alarm: 1,
            grant: Grant::Unscoped,
            gated_arm: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let router = router();
        let name = format!("owed-span-{index}");
        let metric = format!("Breach{index}");
        let (topic, url) =
            given_topic_with_subscriber(&router, case, REGION, &name, row.grant).await;

        // The series as it stands BEFORE any alarm watches it: one datapoint in the period
        // before the one the clock is in, which is the newest period an evaluation grades, and
        // one in the period still being published into.
        let published_at = now_secs();
        publish(&router, row, &metric, published_at - MINUTE_SECS).await;
        publish(&router, row, &metric, published_at).await;

        let control = format!("{name}-control");
        let gated = format!("{name}-gated");
        put_alarm(&router, row, &control, &metric, &topic, false).await;
        if row.gated_arm {
            put_alarm(&router, row, &gated, &metric, &topic, true).await;
        }

        // The rest of the client's write sequence: it reads each alarm back. The control still
        // reports the state the write left it in, because the evaluation of its series is owed a
        // span and this is still the write's own sequence of calls.
        let written = described(&router, case, &control).await;
        assert_eq!(
            written["StateValue"].as_str(),
            Some("INSUFFICIENT_DATA"),
            "case {case:?}: the write leaves the alarm in insufficient data and the evaluated \
             state is owed a span later, got {written}"
        );
        assert!(
            written.get("EvaluationState").is_none_or(Value::is_null),
            "case {case:?}: an alarm carrying no warm-up reports no evaluation state, got \
             {written}"
        );
        if row.gated_arm {
            let withheld = described(&router, case, &gated).await;
            assert_eq!(
                withheld["EvaluationState"].as_str(),
                Some(IN_WARM_UP),
                "case {case:?}: the arm inside its warm-up window reports that it is, got \
                 {withheld}"
            );
        }

        // And the client CLEARS the action's target of whatever was waiting there before it
        // starts watching it, which is what a consumer of a queue does. Nothing of this
        // transition may be in there yet.
        let cleared = drain(&router, case, REGION, &url).await;
        assert!(
            cleared.is_empty(),
            "case {case:?}: no notification may have been delivered while the client was still \
             writing its estate, got {cleared:?}"
        );

        // Only NOW does the client begin watching, and the transition it is owed arrives.
        let arrived = until_notified(&router, row, &metric, &url, &control).await;
        let notification = arrived
            .iter()
            .find(|notification| notification["AlarmName"].as_str() == Some(control.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the control's notification must reach the subscribed queue, \
                     got {arrived:?}"
                )
            });
        assert_eq!(
            notification["NewStateValue"].as_str(),
            Some("ALARM"),
            "case {case:?}: the notification must carry the state moved to, got {notification}"
        );
        assert_eq!(
            notification["OldStateValue"].as_str(),
            Some("INSUFFICIENT_DATA"),
            "case {case:?}: the notification must carry the state moved out of, got \
             {notification}"
        );

        // The alarm now stands in the state its notification announced, and its own account of
        // the action it executed names the topic the publish went to.
        let moved = described(&router, case, &control).await;
        assert_eq!(
            moved["StateValue"].as_str(),
            Some("ALARM"),
            "case {case:?}: the breaching series must move the control alarm, got {moved}"
        );
        let items = action_history(&router, case, &control).await;
        let executed = items.first().unwrap_or_else(|| {
            panic!("case {case:?}: the delivered action must be recorded, got {items:?}")
        });
        let data: Value = serde_json::from_str(executed["HistoryData"].as_str().unwrap_or_default())
            .unwrap_or_else(|error| {
                panic!("case {case:?}: the action record is not a document ({error}): {executed}")
            });
        assert_eq!(
            data["actionState"].as_str(),
            Some("Succeeded"),
            "case {case:?}: the recorded action must report the publish it made, got {executed}"
        );
        assert!(
            executed["HistorySummary"]
                .as_str()
                .is_some_and(|summary| summary.contains(&topic)),
            "case {case:?}: the recorded action must name the topic it published to ({topic}), \
             got {executed}"
        );

        // The gated arm watches the SAME series and carries the SAME action, and its warm-up
        // withholds both: read only now, after the control's own delivery has arrived, so an
        // absence here is an absence rather than a delivery still in flight.
        if row.gated_arm {
            let withheld = described(&router, case, &gated).await;
            assert_eq!(
                withheld["StateValue"].as_str(),
                Some("INSUFFICIENT_DATA"),
                "case {case:?}: an alarm inside its warm-up stays in insufficient data, got \
                 {withheld}"
            );
            assert!(
                !arrived
                    .iter()
                    .any(|notification| notification["AlarmName"].as_str()
                        == Some(gated.as_str())),
                "case {case:?}: nothing may be delivered for the arm that is warming up, got \
                 {arrived:?}"
            );
        }
    }
}
