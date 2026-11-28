// SPDX-License-Identifier: CC0-1.0

//! A metric alarm over a CUSTOM-namespace series that starts breaching only after the alarm was
//! written reaches the queue subscribed to the topic its `AlarmActions` names — through the WHOLE
//! mounted service set, the one `core/router/src/lib.rs` assembles for the binary.
//!
//! The RULE under test: **an alarm's evaluation is owed a span after the write that configured it
//! AND after each evaluation it makes — the span delays the verdict, it never suppresses one — so
//! an alarm whose series holds nothing to grade rests where the write left it ("When this
//! operation creates an alarm, the alarm state is immediately set to INSUFFICIENT_DATA",
//! `smithy_query(query="operation",operation="PutMetricAlarm")`) and executes no action, and the
//! FIRST evaluation after a breaching datapoint of a closed period is published moves it to
//! `ALARM` and executes the SNS action that transition fires.**
//!
//! The companion suite
//! [`crate::cloudwatch_alarm_action_is_owed_a_span_after_the_write`] pins the other half of the
//! same cadence: a series that was already breaching when the alarm was written is not graded
//! inside the client's own write sequence. Here the series breaches LATER, so what is pinned is
//! that the alarm goes on being evaluated after it has settled, and that the transition the new
//! datapoint causes is delivered.
//!
//! The rows differ along what the rule generalises over: the statistic the periods are folded by
//! and the direction of the comparison the datapoint has to satisfy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{
    cw, drain, given_topic_with_subscriber, router, Grant,
};

/// The Region the whole wiring of every row lives in.
const REGION: &str = "us-west-2";

/// The namespace every row publishes into, each row under a metric name of its own.
const NAMESPACE: &str = "Custom/LaterBreach";

/// The width of the periods every row's alarm aggregates its series into, in seconds — the
/// shortest regular period (`PutMetricAlarmInput$Period`: "Valid values are 10, 20, 30, and any
/// multiple of 60").
const PERIOD_SECS: i64 = 60;

/// How long a row waits for the transition and the delivery it is owed, and how often it looks.
/// Generous on purpose: the wait is on the CONDITION it is waiting for.
const DELIVERY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

/// How many long-polling looks a row takes at the queue while the alarm has nothing to grade.
/// Each look is a poll of the queue rather than a sleep, and together they span several of the
/// spans an evaluation is owed.
const QUIET_LOOKS: usize = 5;

/// Wall-clock now, as the epoch seconds the evaluation windows are measured against.
fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the system clock is after the epoch")
        .as_secs_f64()
}

/// The middle of the newest period that has already CLOSED, which is the newest period an
/// evaluation grades.
fn newest_closed_instant() -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let period = PERIOD_SECS as f64;
    (now_secs() / period).floor() * period - period / 2.0
}

/// One row of the table: the alarm shape it writes and the datapoint that has to satisfy it.
struct Row {
    case: &'static str,
    statistic: &'static str,
    comparison_operator: &'static str,
    threshold: f64,
    /// The value the row publishes once the alarm is standing, which that comparison breaches.
    breaching_value: f64,
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

/// Watch the queue at `url` until a notification naming `alarm` has arrived — a wait for the
/// CONDITION, paced by the long poll each turn makes.
async fn until_notified(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    alarm: &str,
) -> Vec<Value> {
    let giving_up_at = std::time::Instant::now() + DELIVERY_DEADLINE;
    let mut arrived: Vec<Value> = Vec::new();
    loop {
        arrived.extend(drain(router, case, REGION, url).await);
        let named = arrived
            .iter()
            .any(|notification| notification["AlarmName"].as_str() == Some(alarm));
        if named || std::time::Instant::now() >= giving_up_at {
            return arrived;
        }
    }
}

/// RULE: the span an evaluation is owed delays the verdict and never suppresses it — an alarm with
/// nothing to grade executes no action, and the first evaluation after a breaching datapoint of a
/// closed period arrives executes the one its transition fires.
#[tokio::test]
async fn a_series_that_breaches_after_the_alarm_was_written_notifies_the_subscribed_queue() {
    let rows: &[Row] = &[
        Row {
            case: "a maximum over the threshold",
            statistic: "Maximum",
            comparison_operator: "GreaterThanThreshold",
            threshold: 10.0,
            breaching_value: 100.0,
        },
        Row {
            case: "an average under the threshold",
            statistic: "Average",
            comparison_operator: "LessThanThreshold",
            threshold: 10.0,
            breaching_value: 1.0,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let router = router();
        let name = format!("later-breach-{index}");
        let metric = format!("Signal{index}");
        let (topic, url) =
            given_topic_with_subscriber(&router, case, REGION, &name, Grant::Unscoped).await;

        cw(
            &router,
            case,
            REGION,
            "PutMetricAlarm",
            &json!({
                "AlarmName": name,
                "Namespace": NAMESPACE,
                "MetricName": metric,
                "Statistic": row.statistic,
                "Period": PERIOD_SECS,
                "EvaluationPeriods": 1,
                "DatapointsToAlarm": 1,
                "Threshold": row.threshold,
                "ComparisonOperator": row.comparison_operator,
                "TreatMissingData": "missing",
                "ActionsEnabled": true,
                "AlarmActions": [topic.clone()],
            }),
        )
        .await;

        // Nothing is published into the series, so every evaluation the span owes finds nothing
        // to grade: the alarm rests where the write left it and executes no action, however many
        // of those spans pass.
        for look in 0..QUIET_LOOKS {
            let held = drain(&router, case, REGION, &url).await;
            assert!(
                held.is_empty(),
                "case {case:?}: look {look}: an alarm with nothing to grade must execute no \
                 action, got {held:?}"
            );
        }
        let resting = described(&router, case, &name).await;
        assert_eq!(
            resting["StateValue"].as_str(),
            Some("INSUFFICIENT_DATA"),
            "case {case:?}: an alarm whose series holds nothing to grade rests where the write \
             left it, got {resting}"
        );

        // Now the series breaches, in the newest period an evaluation grades. The next
        // evaluation the span owes moves the alarm and executes its action.
        cw(
            &router,
            case,
            REGION,
            "PutMetricData",
            &json!({
                "Namespace": NAMESPACE,
                "MetricData": [{
                    "MetricName": metric,
                    "Unit": "Count",
                    "Value": row.breaching_value,
                    "Timestamp": newest_closed_instant(),
                }],
            }),
        )
        .await;

        let arrived = until_notified(&router, case, &url, &name).await;
        let notification = arrived
            .iter()
            .find(|notification| notification["AlarmName"].as_str() == Some(name.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the transition the published datapoint causes must reach the \
                     subscribed queue, got {arrived:?}"
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
        let moved = described(&router, case, &name).await;
        assert_eq!(
            moved["StateValue"].as_str(),
            Some("ALARM"),
            "case {case:?}: the alarm must stand in the state its notification announced, got \
             {moved}"
        );
    }
}
