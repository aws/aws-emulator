// SPDX-License-Identifier: CC0-1.0

//! EVERY ROUND of an alarm's fan-out, over the whole mounted service set: each transition into
//! `ALARM` hands ONE notification, carrying the reason that transition was given, to each
//! subscribed queue whose own policy admits the delivery — and to no other, for as many rounds
//! as a client drives.
//!
//! The RULE under test: **a metric alarm whose `AlarmActions` names an SNS topic publishes one
//! notification to that topic on every transition INTO `ALARM` ("the actions to execute when
//! this alarm transitions to the ALARM state from any other state",
//! `PutMetricAlarmInput$AlarmActions`), carrying the alarm's name, `NewStateValue` = `ALARM`,
//! the `NewStateReason` that transition was given (`SetAlarmStateInput$StateReason`, "the
//! reason that this alarm is set to this specific state") and the series the alarm watches as
//! its `Trigger`; Amazon SNS enqueues that notification into every subscribed Amazon SQS queue
//! whose own `Policy` grants the SNS service principal `sqs:SendMessage` and into NO other, the
//! grant being read when the delivery is MADE — so an ungranted subscriber reports nothing
//! waiting and nothing in flight after every round, and once granted receives the NEXT round's
//! notification and never the rounds withheld before it.**
//!
//! # Why this suite, beside the ones already here
//!
//! The sibling router suites pin the ends of this chain: a single forced transition reaching a
//! subscribed queue with the actions switched off and on again
//! (`cloudwatch_alarm_action_reaches_its_subscribed_queue.rs`), and an alarm over a series
//! Amazon ECS publishes settling into `ALARM` on the clock's own evaluation and fanning that
//! transition out to the queue its policy admits
//! (`ecs_container_insights_alarm_fans_out_to_its_granted_queues.rs`). What only this one
//! reaches is the chain driven REPEATEDLY: a client that drives round after round and tells the
//! rounds apart by the reason it gave each of them, which is what shows one transition owes one
//! delivery every time rather than once, that the reason travels with it rather than being
//! re-derived, and that the queue the policy left out is observably empty — read off the depths
//! Amazon SQS itself reports ("ApproximateNumberOfMessages", "ApproximateNumberOfMessagesNotVisible"
//! and "ApproximateNumberOfMessagesDelayed", `GetQueueAttributes$AttributeNames`) rather than
//! only off a receive — so a withheld notification cannot be mistaken for a slow one.
//!
//! Each row's alarm watches a series nothing publishes into and treats its missing data as
//! `missing` ("If `TreatMissingData` is omitted, the default behavior of `missing` is used"), so
//! the alarm's own evaluation never computes `ALARM` and every transition into it is the round's
//! own: what the assertions below count is therefore exactly the rounds the client drove.
//!
//! The rows differ along what the rule generalises over: the namespace and metric the alarm
//! watches and how its series is dimensioned (a cluster/service pair of the Container Insights
//! namespace, one dimension, none at all), the comparison the alarm is graded by, and how many
//! rounds the client drives.
//!
//! Every wait polls the CONDITION rather than a span, with a generous deadline: the deadlines are
//! here so a failure is a failure rather than a hang.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The whole-emulator router, the CloudWatch and SNS doors and the queue reader this crate
// already keeps for the alarm fan-out, rather than a second copy of each.
use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{cw, drain, router, sns};
// The Amazon SQS door, the queue setup and the two queue readers the sibling Container
// Insights fan-out suite already keeps, reached rather than copied: one implementation of
// "a queue with no policy", "grant Amazon SNS the send" and "look until it arrives".
use crate::ecs_container_insights_alarm_fans_out_to_its_granted_queues::{
    delivered_to, given_queue, grant_sns_delivery, sqs, what_arrived_over_several_looks,
};
// The ONE element reader this test crate keeps for the query-protocol wires.
use crate::query_wire::element;

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The service principal Amazon SNS performs a delivery to a subscribed queue as, and the action
/// that delivery performs on the queue — the two a queue policy has to name for a notification to
/// reach it, and what every assertion below about a withheld delivery names.
const SEND_MESSAGE: &str = "sqs:SendMessage";

/// The states a round drives the alarm through: into `OK`, which fires nothing of its own (no
/// row's alarm carries an `OKActions` entry), and then into the state whose transition fires the
/// topic action.
const OK_STATE: &str = "OK";
const ALARM_STATE: &str = "ALARM";

/// The period every row's alarm is evaluated over, in seconds, and how many of those periods it
/// looks back over. "Valid values are 10, 20, 30, and any multiple of 60"
/// (`PutMetricAlarmInput$Period`).
const ALARM_PERIOD: i32 = 60;
const EVALUATION_PERIODS: i32 = 1;

/// The missing-data treatment every row carries: the documented default, spelled out — "If
/// `TreatMissingData` is omitted, the default behavior of `missing` is used"
/// (`PutMetricAlarmInput$TreatMissingData`), which leaves an alarm over a series nothing
/// publishes undecided rather than breaching, so no evaluation of its own can fire the action.
const TREAT_MISSING: &str = "missing";

/// The depth members a client reads a queue's own emptiness off
/// (`GetQueueAttributes$AttributeNames`): what is waiting, what has been handed to a receiver and
/// what is held back for its delay.
const DEPTH_ATTRIBUTES: [&str; 3] = [
    "ApproximateNumberOfMessages",
    "ApproximateNumberOfMessagesNotVisible",
    "ApproximateNumberOfMessagesDelayed",
];

/// One row of the table: the series the alarm watches, how it is graded, and how many rounds the
/// client drives it through.
#[derive(Debug, Clone, Copy)]
struct Row {
    /// What this row is about, named by every assertion it makes.
    case: &'static str,
    /// The alarm, and the names its topic and its two queues are built from.
    alarm: &'static str,
    /// The series the alarm watches: the namespace, the metric and the dimensions that key it.
    namespace: &'static str,
    metric: &'static str,
    dimensions: &'static [(&'static str, &'static str)],
    /// The comparison the alarm is graded by, and the value it is compared against.
    comparison: &'static str,
    threshold: f64,
    /// How many rounds the client drives before it repairs the silent queue's policy.
    rounds: usize,
}

impl Row {
    /// The dimension list the row's series is identified by, as `PutMetricAlarm` takes it.
    fn dimension_list(self) -> Value {
        Value::Array(
            self.dimensions
                .iter()
                .map(|(name, value)| json!({"Name": name, "Value": value}))
                .collect(),
        )
    }
}

/// The table: one row per shape the rule generalises over.
///
/// The first row is the recorded shape — a Container Insights running task count of one service
/// of one cluster, watched for a count that has fallen below the one task a client wants.
const ROWS: &[Row] = &[
    Row {
        case: "a Container Insights running task count keyed by a cluster and a service",
        alarm: "insights-rounds",
        namespace: "ECS/ContainerInsights",
        metric: "RunningTaskCount",
        dimensions: &[("ClusterName", "obs-rounds"), ("ServiceName", "idle")],
        comparison: "LessThanThreshold",
        threshold: 1.0,
        rounds: 3,
    },
    Row {
        case: "a customer metric keyed by one dimension, over two rounds",
        alarm: "backlog-rounds",
        namespace: "Custom/Orders",
        metric: "Backlog",
        dimensions: &[("QueueName", "orders")],
        comparison: "GreaterThanOrEqualToThreshold",
        threshold: 100.0,
        rounds: 2,
    },
    Row {
        case: "a series with no dimensions at all, over one round",
        alarm: "undimensioned-rounds",
        namespace: "Custom/Fleet",
        metric: "Healthy",
        dimensions: &[],
        comparison: "LessThanOrEqualToThreshold",
        threshold: 0.0,
        rounds: 1,
    },
];

/// What one row's fan-out was wired to: the topic the alarm publishes to, the queue whose policy
/// admits the delivery, and the queue whose policy says nothing at all.
struct Wiring {
    topic: String,
    granted: String,
    silent: String,
    silent_arn: String,
}

/// A topic named for `row`'s alarm with two queues of this emulator subscribed to it: one whose
/// policy grants Amazon SNS the send, and one carrying no policy at all.
async fn given_fan_out(router: &MultiServiceRouter, row: &Row) -> Wiring {
    let case = row.case;
    let topic = element(
        &sns(router, case, REGION, "CreateTopic", &[("Name", row.alarm)]).await,
        "TopicArn",
    );
    let (granted, granted_arn) = given_queue(router, case, &format!("{}-granted", row.alarm)).await;
    let (silent, silent_arn) = given_queue(router, case, &format!("{}-silent", row.alarm)).await;
    for endpoint in [&granted_arn, &silent_arn] {
        sns(
            router,
            case,
            REGION,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", endpoint),
            ],
        )
        .await;
    }
    grant_sns_delivery(router, case, &granted, &granted_arn, &topic).await;
    Wiring {
        topic,
        granted,
        silent,
        silent_arn,
    }
}

/// Configure `row`'s alarm over its own series, aimed at its own topic.
async fn given_alarm(router: &MultiServiceRouter, row: &Row, topic: &str) {
    cw(
        router,
        row.case,
        REGION,
        "PutMetricAlarm",
        &json!({
            "AlarmName": row.alarm,
            "Namespace": row.namespace,
            "MetricName": row.metric,
            "Dimensions": row.dimension_list(),
            "Statistic": "Maximum",
            "Period": ALARM_PERIOD,
            "EvaluationPeriods": EVALUATION_PERIODS,
            "Threshold": row.threshold,
            "ComparisonOperator": row.comparison,
            "TreatMissingData": TREAT_MISSING,
            "AlarmActions": [topic],
        }),
    )
    .await;
}

/// Move one alarm into `state`, giving the transition `reason` — which is what a client tells one
/// round's notification from another's by.
async fn force(
    router: &MultiServiceRouter,
    case: &str,
    alarm: &str,
    state: &str,
    reason: &str,
) {
    cw(
        router,
        case,
        REGION,
        "SetAlarmState",
        &json!({
            "AlarmName": alarm,
            "StateValue": state,
            "StateReason": reason,
        }),
    )
    .await;
}

/// The reason one round gives the transition it drives: a token no other round carries.
fn round_reason(row: &Row, round: usize) -> String {
    format!(
        "{} fell to zero running tasks on round {round}",
        row.metric
    )
}

/// The `StateValue` `DescribeAlarms` reports for `alarm`.
async fn reported_state(router: &MultiServiceRouter, case: &str, alarm: &str) -> String {
    let described = cw(
        router,
        case,
        REGION,
        "DescribeAlarms",
        &json!({"AlarmNames": [alarm]}),
    )
    .await;
    described["MetricAlarms"][0]["StateValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The depths Amazon SQS reports for the queue at `url`, as `(member, count)` pairs — what a
/// client watching its own queue reads.
///
/// A member the queue does not report at all is a FAILURE here rather than a zero, so no
/// assertion below can pass on a count nothing answered.
async fn depths(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<(&'static str, u64)> {
    let attributes = sqs(
        router,
        case,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": DEPTH_ATTRIBUTES}),
    )
    .await;
    DEPTH_ATTRIBUTES
        .iter()
        .map(|member| {
            let reported = attributes["Attributes"][*member].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: the queue reports no {member}: {attributes}")
            });
            let count = reported.parse::<u64>().unwrap_or_else(|error| {
                panic!("case {case:?}: {member} is not a number ({error}): {attributes}")
            });
            (*member, count)
        })
        .collect()
}

/// Assert that `delivered` is the ONE notification a round owes: it names the alarm, the state the
/// transition moved to, the reason that transition was given and the series that is watched.
fn assert_round_notification(case: &str, row: &Row, reason: &str, delivered: &[Value]) {
    assert_eq!(
        delivered.len(),
        1,
        "case {case:?}: the round whose reason is {reason:?} owes exactly one notification, got \
         {delivered:?}"
    );
    let notification = &delivered[0];
    assert_eq!(
        notification["AlarmName"].as_str(),
        Some(row.alarm),
        "case {case:?}: the notification must name the alarm, got {notification}"
    );
    assert_eq!(
        notification["NewStateValue"].as_str(),
        Some(ALARM_STATE),
        "case {case:?}: the notification must carry the state moved to, got {notification}"
    );
    assert_eq!(
        notification["NewStateReason"].as_str(),
        Some(reason),
        "case {case:?}: the notification must carry the reason the transition was given, got \
         {notification}"
    );
    assert_eq!(
        notification["Trigger"]["MetricName"].as_str(),
        Some(row.metric),
        "case {case:?}: the notification must name the metric watched, got {notification}"
    );
    assert_eq!(
        notification["Trigger"]["Namespace"].as_str(),
        Some(row.namespace),
        "case {case:?}: the notification must name the namespace watched, got {notification}"
    );
}

/// Assert the queue at `url` reports nothing waiting, nothing in flight and nothing delayed.
async fn assert_empty_queue(router: &MultiServiceRouter, case: &str, url: &str, when: &str) {
    let reported = depths(router, case, url).await;
    for (member, count) in &reported {
        assert_eq!(
            *count, 0,
            "case {case:?}: {when}, the queue granting Amazon SNS no {SEND_MESSAGE} must report \
             0 for {member}, got {count} in {reported:?}"
        );
    }
}

/// RULE: every round's transition into `ALARM` hands exactly one notification, carrying that
/// round's own reason, to the subscribed queue whose policy admits the delivery, and the
/// subscriber whose policy admits nothing stays observably empty round after round.
#[tokio::test]
async fn every_alarm_round_reaches_the_granted_queue_and_leaves_the_ungranted_one_empty() {
    for row in ROWS {
        let case = row.case;
        let router = router();
        let wiring = given_fan_out(&router, row).await;
        given_alarm(&router, row, &wiring.topic).await;

        for round in 0..row.rounds {
            // Into OK first, so the move into ALARM is a transition whatever the alarm's own
            // evaluation left behind. It fires nothing itself: no row carries an `OKActions`.
            force(
                &router,
                case,
                row.alarm,
                OK_STATE,
                &format!("resetting before round {round}"),
            )
            .await;
            let reason = round_reason(row, round);
            force(&router, case, row.alarm, ALARM_STATE, &reason).await;
            let state = reported_state(&router, case, row.alarm).await;
            assert_eq!(
                state, ALARM_STATE,
                "case {case:?}: round {round} must leave the alarm in {ALARM_STATE}, got {state:?}"
            );

            let delivered = delivered_to(&router, case, &wiring.granted).await;
            assert_round_notification(case, row, &reason, &delivered);
            let again = drain(&router, case, REGION, &wiring.granted).await;
            assert!(
                again.is_empty(),
                "case {case:?}: round {round} owes one notification and no second, got {again:?}"
            );

            assert_empty_queue(
                &router,
                case,
                &wiring.silent,
                &format!("after round {round}"),
            )
            .await;
        }

        // The sibling subscribed to the very same topic, whose policy says nothing about the
        // delivery, received nothing over any of those rounds.
        let withheld = what_arrived_over_several_looks(&router, case, &wiring.silent).await;
        assert!(
            withheld.is_empty(),
            "case {case:?}: a subscribed queue granting Amazon SNS no {SEND_MESSAGE} must receive \
             nothing over {} round(s), got {withheld:?}",
            row.rounds
        );

        // The grant is added, and one more round is driven: the queue that was silent holds THAT
        // round's notification — not the rounds withheld before it — and so does the sibling that
        // held every earlier one.
        grant_sns_delivery(
            &router,
            case,
            &wiring.silent,
            &wiring.silent_arn,
            &wiring.topic,
        )
        .await;
        let repair_round = row.rounds;
        force(
            &router,
            case,
            row.alarm,
            OK_STATE,
            &format!("resetting before round {repair_round}"),
        )
        .await;
        let reason = round_reason(row, repair_round);
        force(&router, case, row.alarm, ALARM_STATE, &reason).await;

        let repaired = delivered_to(&router, case, &wiring.silent).await;
        assert_round_notification(case, row, &reason, &repaired);
        let delivered = delivered_to(&router, case, &wiring.granted).await;
        assert_round_notification(case, row, &reason, &delivered);
    }
}
