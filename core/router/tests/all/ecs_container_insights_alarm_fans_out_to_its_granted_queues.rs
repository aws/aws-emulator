// SPDX-License-Identifier: CC0-1.0

//! The Container Insights alarm FAN-OUT: an alarm over a series an Amazon ECS cluster publishes
//! settles on the emulator's own clock, and the notification that transition fires reaches
//! exactly the subscribed queues whose own policy admits the delivery — driven end to end over
//! the production service set, the one `core/router/src/lib.rs` assembles for the binary.
//!
//! The RULE under test: **a metric alarm watching a Container Insights series of a cluster the
//! setting is ON for settles into `ALARM` on the evaluation the clock owes, with no call forcing
//! it, once the datapoints that series holds stand on the breaching side of the alarm's own
//! comparison; every transition INTO that state publishes ONE notification to each
//! `AlarmActions` topic, naming the alarm, the state it moved to and the metric that decided it,
//! and Amazon SNS enqueues that notification into each subscribed Amazon SQS queue whose own
//! `Policy` grants `sqs:SendMessage` to the SNS service principal and into NO other — the grant
//! being read when the delivery is made, so a queue granted after a silent transition receives
//! the next one.**
//!
//! Where each half comes from. `ClusterSetting$name` is `containerInsights` and
//! `Cluster$settings` "indicates whether CloudWatch Container Insights is on or off for a
//! cluster", so a cluster it is on for publishes that namespace's series for its own records;
//! `MetricAlarm$ComparisonOperator` is "the arithmetic operation to use when comparing the
//! specified statistic and threshold" and `PutMetricAlarmInput$AlarmActions` are "the actions to
//! execute when this alarm transitions to the ALARM state from any other state […] Valid Values:
//! Amazon SNS actions", so the state is the alarm's own comparison over the series and the topic
//! is what a transition hands it to; and Amazon SNS states the last hop for a subscribed queue
//! ("you must ensure that the Amazon SQS queue policy allows the Amazon SNS topic to send
//! messages to the queue"), which is why the queue with no such statement stays empty while its
//! sibling receives. The alarm reports what it decided over in "an explanation for the alarm
//! state, in JSON format" (`MetricAlarm$StateReasonData`), which is where each row reads back the
//! value the published series carried.
//!
//! # Why this suite lives here
//!
//! Four crates have to agree before any of it is observable: Amazon ECS owns the cluster, the
//! service and the collection that publishes the counts, CloudWatch owns the metric store, the
//! evaluation and the publish, Amazon SNS owns the fan-out and the policy decision on the
//! delivery, and Amazon SQS owns the queue the client finally looks at. The publish CloudWatch
//! makes and the delivery SNS makes are in-process calls addressed the way a client's own are, so
//! which mounted model claims them is only decidable with every service mounted. The sibling
//! router suites drive this chain from a client's forced transition
//! (`cloudwatch_alarm_action_reaches_its_subscribed_queue.rs`) and from a series nothing publishes
//! at all (`cloudwatch_breaching_alarm_fires_through_the_whole_router.rs`); what only this one
//! reaches is the chain driven by a series ANOTHER SERVICE publishes, with both halves of the
//! queue-policy decision on one topic at once.
//!
//! The rows differ along what the rule generalises over: which Container Insights metric drives
//! the alarm and at which level it is dimensioned (a service's own series, and a cluster-level
//! one), the statistic its windows are folded by, the direction and the threshold of the
//! comparison, the value the series carries (none, and one), and how many periods the look-back
//! covers. The second test is the half only two rounds can show: the queue granted after a silent
//! transition receives the next notification, and the granted sibling receives one per round.
//!
//! Every wait polls the CONDITION rather than a span, and the deadlines are generous on purpose:
//! they are here so a failure is a failure rather than a hang.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The whole-emulator router, the CloudWatch door and the queue reader this crate already keeps
// for the alarm fan-out, rather than a second copy of each.
use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{
    authorization, cw, drain, router,
};
// The ONE percent-encoder and element reader this test crate keeps for query-protocol suites
// (`all/query_wire.rs`).
use crate::query_wire::{element, form_encode};

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The `X-Amz-Target` prefix Amazon ECS declares, and the awsQuery API version Amazon SNS does.
const ECS_TARGET_PREFIX: &str = "AmazonEC2ContainerServiceV20141113";
const SNS_API_VERSION: &str = "2010-03-31";

/// The service principal Amazon SNS performs a delivery to a subscribed queue as, and the action
/// that delivery performs on the queue — the two a queue policy has to name for a notification to
/// reach it.
const SNS_PRINCIPAL: &str = "sns.amazonaws.com";
const SEND_MESSAGE: &str = "sqs:SendMessage";

/// The namespace Amazon ECS publishes its Container Insights metrics in, the dimensions those
/// metrics carry and the four series this suite watches — spelled out here rather than imported
/// from the implementation, so a wrong constant there cannot make a test pass. Their source is the
/// CloudWatch User Guide's "Amazon ECS Container Insights metrics" table, whose service-level rows
/// are dimensioned by `ClusterName` and `ServiceName` and whose cluster-level rows by `ClusterName`
/// alone.
const NAMESPACE: &str = "ECS/ContainerInsights";
const CLUSTER_DIMENSION: &str = "ClusterName";
const SERVICE_DIMENSION: &str = "ServiceName";
const RUNNING_TASK_COUNT: &str = "RunningTaskCount";
const DESIRED_TASK_COUNT: &str = "DesiredTaskCount";
const TASK_COUNT: &str = "TaskCount";
const SERVICE_COUNT: &str = "ServiceCount";

/// The cluster setting that turns that namespace on, and the value this suite turns it on with.
const CONTAINER_INSIGHTS_SETTING: &str = "containerInsights";
const INSIGHTS_ENABLED: &str = "enabled";

/// The cluster, the service and the task definition family every row watches.
const CLUSTER: &str = "insightsfanout";
const SERVICE: &str = "idleservice";

/// The count the scheduler holds that service at: none, which is what makes a running-task count
/// of zero the steady reading of the series — the shape the alarm is built on.
const IDLE_COUNT: i64 = 0;

/// The image the task definition names and how much memory its container reserves. Nothing is
/// ever placed (the service asks for no task), so neither decides anything here.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";
const CONTAINER_MEMORY: i64 = 128;

/// The period every alarm of this suite is evaluated over, in seconds: the shortest cadence a
/// series stored at regular resolution has one value per ("Setting this to 60 specifies this
/// metric as regular resolution, which CloudWatch stores at 1-minute resolution",
/// `MetricDatum$StorageResolution`), which is the resolution a service publishes its own metrics
/// at.
const ALARM_PERIOD: i32 = 60;

/// The missing-data treatment every row carries: "breaching", the one a client watching for a
/// count that has fallen away configures, so a period the collection published nothing into is
/// counted on the breaching side rather than leaving the alarm undecided.
const TREAT_MISSING_BREACHING: &str = "breaching";

/// The state the rows wait for, and the one every notification they assert on carries.
const ALARM_STATE: &str = "ALARM";

/// The state a round forces the alarm through so the next force is a transition INTO `ALARM`.
/// It fires nothing of its own: no row's alarm carries an `OKActions` entry.
const OK_STATE: &str = "OK";

/// How wide a window the statistics read asks about, in seconds: far enough back to hold every
/// collection this suite's cluster has made, and far enough ahead to hold the period the clock is
/// in.
const READ_BACK_SECS: f64 = 900.0;
const READ_AHEAD_SECS: f64 = 300.0;

/// How often a poll looks again, and how long it keeps looking.
///
/// Generous: a Container Insights datapoint is stored at the one-minute resolution a service's own
/// metrics take, so the first whole period an alarm may grade closes at the next clock minute, and
/// this suite runs beside every other test of the workspace. Every poll stops the instant its own
/// condition holds.
const POLL: Duration = Duration::from_millis(100);
const DEADLINE: Duration = Duration::from_secs(240);

/// How long a row keeps looking for a delivery it expects.
///
/// Generous for the same reason, and bounded by the CONDITION: every look is a long poll of the
/// queue, and the wait ends the instant the notification is on it.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(120);

/// How many times a row looks at the queue no delivery was authorized into before calling it
/// silent. Each look is a long poll of the queue itself, so the window is Amazon SQS's own wait
/// rather than a sleep of this suite's.
const SILENT_LOOKS: usize = 3;

/// How close two counts have to read to be the same count: these are whole numbers of records,
/// carried as doubles.
const COUNT_TOLERANCE: f64 = 1e-9;

/// One row of the table: the Container Insights series it watches, how that series is folded and
/// compared, and the value the series carries for it.
#[derive(Debug, Clone, Copy)]
struct Row {
    /// What this row is about, named by every assertion it makes.
    case: &'static str,
    /// The alarm, and the names its topic and its two queues are built from.
    alarm: &'static str,
    /// The metric the alarm watches, and whether it is dimensioned by the cluster alone.
    metric: &'static str,
    cluster_level: bool,
    /// The statistic each period is folded by, the comparison the fold is graded with and the
    /// value it is compared against.
    statistic: &'static str,
    comparison: &'static str,
    threshold: f64,
    /// How many periods the look-back covers.
    evaluation_periods: i32,
    /// The value the series carries, which the row's comparison holds for.
    published: f64,
}

impl Row {
    /// The dimension list the row's series is identified by.
    fn dimensions(self) -> Value {
        if self.cluster_level {
            json!([{"Name": CLUSTER_DIMENSION, "Value": CLUSTER}])
        } else {
            json!([
                {"Name": CLUSTER_DIMENSION, "Value": CLUSTER},
                {"Name": SERVICE_DIMENSION, "Value": SERVICE},
            ])
        }
    }
}

/// The table: one row per shape the rule generalises over.
const ROWS: &[Row] = &[
    Row {
        // The recorded shape: a service's running task count, below the one task a client watches
        // for.
        case: "a service's running task count under one",
        alarm: "insights-running-tasks",
        metric: RUNNING_TASK_COUNT,
        cluster_level: false,
        statistic: "Maximum",
        comparison: "LessThanThreshold",
        threshold: 1.0,
        evaluation_periods: 1,
        published: 0.0,
    },
    Row {
        case: "a service's desired task count at none, over two periods",
        alarm: "insights-desired-tasks",
        metric: DESIRED_TASK_COUNT,
        cluster_level: false,
        statistic: "Minimum",
        comparison: "LessThanOrEqualToThreshold",
        threshold: 0.0,
        evaluation_periods: 2,
        published: 0.0,
    },
    Row {
        case: "the cluster's own task count under half a task",
        alarm: "insights-cluster-tasks",
        metric: TASK_COUNT,
        cluster_level: true,
        statistic: "Average",
        comparison: "LessThanThreshold",
        threshold: 0.5,
        evaluation_periods: 1,
        published: 0.0,
    },
    Row {
        // The other direction, over a series that carries something rather than nothing.
        case: "the cluster holding a service, over three periods",
        alarm: "insights-cluster-services",
        metric: SERVICE_COUNT,
        cluster_level: true,
        statistic: "Maximum",
        comparison: "GreaterThanOrEqualToThreshold",
        threshold: 1.0,
        evaluation_periods: 3,
        published: 1.0,
    },
];

/// What one row's fan-out was wired to: the topic the alarm publishes to, the queue whose policy
/// admits the delivery, and the queue whose policy says nothing at all.
struct Wiring {
    topic: String,
    granted: String,
    silent: String,
}

/// One call that must answer 200, with its parsed body.
async fn answered(
    router: &MultiServiceRouter,
    case: &str,
    what: &str,
    request: http::Request<Bytes>,
) -> Value {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {what}, got {status} with {parsed}"
    );
    parsed
}

/// One Amazon ECS call (`awsJson1_1`).
async fn ecs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ecs.{REGION}.amazonaws.com"))
        .header("authorization", authorization("ecs", REGION))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{ECS_TARGET_PREFIX}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    answered(router, case, &format!("Amazon ECS {operation}"), request).await
}

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`).
///
/// `pub(crate)` with the queue helpers below, so the sibling suite that drives this same
/// granted/withheld fan-out over SEVERAL rounds
/// (`alarm_transition_rounds_reach_only_the_granted_queues.rs`) reaches one copy of each
/// rather than keeping a second.
pub(crate) async fn sqs(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("authorization", authorization("sqs", REGION))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    answered(router, case, &format!("Amazon SQS {operation}"), request).await
}

/// One Amazon SNS call over the awsQuery form wire that service serves.
async fn sns(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version={SNS_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("authorization", authorization("sns", REGION))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Amazon SNS {action}, got {status} with {text}"
    );
    text
}

/// A cluster with Container Insights turned on, holding one service the scheduler keeps at no
/// tasks — the records every row's series is collected off.
async fn given_insights_cluster(router: &MultiServiceRouter, case: &str) {
    ecs(
        router,
        case,
        "CreateCluster",
        &json!({
            "clusterName": CLUSTER,
            "settings": [{"name": CONTAINER_INSIGHTS_SETTING, "value": INSIGHTS_ENABLED}],
        }),
    )
    .await;
    ecs(
        router,
        case,
        "RegisterTaskDefinition",
        &json!({
            "family": SERVICE,
            "containerDefinitions": [{
                "name": "app",
                "image": IMAGE,
                "memory": CONTAINER_MEMORY,
                "essential": true,
            }],
        }),
    )
    .await;
    ecs(
        router,
        case,
        "CreateService",
        &json!({
            "cluster": CLUSTER,
            "serviceName": SERVICE,
            "taskDefinition": SERVICE,
            "desiredCount": IDLE_COUNT,
            "launchType": "EC2",
        }),
    )
    .await;
}

/// Epoch seconds, for the window a statistics read asks about.
fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Every datapoint CloudWatch holds for one row's series, folded by that row's own statistic.
async fn datapoints(router: &MultiServiceRouter, row: &Row) -> Vec<Value> {
    let now = epoch_seconds();
    let answer = cw(
        router,
        row.case,
        REGION,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": row.metric,
            "Dimensions": row.dimensions(),
            "StartTime": now - READ_BACK_SECS,
            "EndTime": now + READ_AHEAD_SECS,
            "Period": ALARM_PERIOD,
            "Statistics": [row.statistic],
        }),
    )
    .await;
    answer["Datapoints"].as_array().cloned().unwrap_or_default()
}

/// Whether two counts are the same count.
fn same_count(left: f64, right: f64) -> bool {
    (left - right).abs() < COUNT_TOLERANCE
}

/// Look again until Amazon ECS has published the value this row grades into CloudWatch, so the
/// alarm below is configured over a series that already carries data: what then moves it is its
/// own evaluation of that data.
async fn wait_for_published(router: &MultiServiceRouter, row: &Row) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let points = datapoints(router, row).await;
        if points.iter().any(|point| {
            point[row.statistic]
                .as_f64()
                .is_some_and(|value| same_count(value, row.published))
        }) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: the Container Insights series {} of {} must report {} as its {}, got \
             {points:?}",
            row.case,
            row.metric,
            row.dimensions(),
            row.published,
            row.statistic
        );
        tokio::time::sleep(POLL).await;
    }
}

/// A queue called `name`, answered as its `(url, arn)`. It carries no policy at all.
pub(crate) async fn given_queue(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> (String, String) {
    let created = sqs(router, case, "CreateQueue", &json!({"QueueName": name})).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {created}"))
        .to_string();
    let attributes = sqs(
        router,
        case,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueArn in {attributes}"))
        .to_string();
    (url, arn)
}

/// Grant Amazon SNS the send on the queue at `url`, scoped by `aws:SourceArn` to the publishing
/// topic — the statement AWS documents for a queue subscribed to a topic.
pub(crate) async fn grant_sns_delivery(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    queue_arn: &str,
    topic: &str,
) {
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowSNSDelivery",
            "Effect": "Allow",
            "Principal": {"Service": SNS_PRINCIPAL},
            "Action": SEND_MESSAGE,
            "Resource": queue_arn,
            "Condition": {"ArnEquals": {"aws:SourceArn": topic}},
        }],
    });
    sqs(
        router,
        case,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "Attributes": {"Policy": document.to_string()},
        }),
    )
    .await;
}

/// A topic named for `alarm`, with two queues of this emulator subscribed to it: one whose policy
/// grants Amazon SNS the send, and one carrying no policy at all.
async fn given_fan_out(router: &MultiServiceRouter, case: &str, alarm: &str) -> Wiring {
    let topic = element(
        &sns(router, case, "CreateTopic", &[("Name", alarm)]).await,
        "TopicArn",
    );
    let (granted, granted_arn) = given_queue(router, case, &format!("{alarm}-granted")).await;
    let (silent, silent_arn) = given_queue(router, case, &format!("{alarm}-silent")).await;
    for endpoint in [&granted_arn, &silent_arn] {
        sns(
            router,
            case,
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
    }
}

/// The alarm one row configures over its own series, aimed at its own topic.
fn alarm_of(row: &Row, topic: &str) -> Value {
    json!({
        "AlarmName": row.alarm,
        "Namespace": NAMESPACE,
        "MetricName": row.metric,
        "Dimensions": row.dimensions(),
        "Statistic": row.statistic,
        "Period": ALARM_PERIOD,
        "EvaluationPeriods": row.evaluation_periods,
        // One period on the breaching side decides the alarm, whatever the periods beside it hold.
        "DatapointsToAlarm": 1,
        "Threshold": row.threshold,
        "ComparisonOperator": row.comparison,
        "TreatMissingData": TREAT_MISSING_BREACHING,
        "AlarmActions": [topic],
    })
}

/// Look again until `alarm` reports `ALARM`, and answer the alarm as `DescribeAlarms` last
/// reported it. NOTHING forces the state: the reads are reads, and what moves the alarm is the
/// evaluation the clock owes it.
async fn settled_in_alarm(router: &MultiServiceRouter, case: &str, alarm: &str) -> Value {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let described = cw(
            router,
            case,
            REGION,
            "DescribeAlarms",
            &json!({"AlarmNames": [alarm]}),
        )
        .await;
        let reported = described["MetricAlarms"][0].clone();
        if reported["StateValue"].as_str() == Some(ALARM_STATE) {
            return reported;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the alarm {alarm} must reach {ALARM_STATE} on its own evaluation, got \
             {described}"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// The values the evaluation that decided the reported state really MEASURED, out of "an
/// explanation for the alarm state, in JSON format" (`MetricAlarm$StateReasonData`).
fn measured_values(case: &str, reported: &Value) -> Vec<f64> {
    let text = reported["StateReasonData"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a StateReasonData, got {reported}"));
    let record: Value = serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("case {case:?}: StateReasonData is not JSON: {error}"));
    record["recentDatapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_f64)
        .collect()
}

/// Look at the queue at `url` several times over and answer what arrived — nothing, for a queue no
/// delivery was authorized into.
pub(crate) async fn what_arrived_over_several_looks(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
) -> Vec<Value> {
    let mut arrived = Vec::new();
    for _ in 0..SILENT_LOOKS {
        arrived.extend(drain(router, case, REGION, url).await);
    }
    arrived
}

/// Look again until the queue at `url` holds a notification, and answer everything that was on it
/// when it did — so a second delivery of one transition is counted rather than hidden behind the
/// first.
///
/// The wait is the condition's: a delivery is asynchronous ("delivery is asynchronous in AWS and is
/// not part of any operation's answer"), so the look is repeated until the message is there or the
/// deadline passes, and nothing is asserted about how long it took.
pub(crate) async fn delivered_to(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
) -> Vec<Value> {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let arrived = drain(router, case, REGION, url).await;
        if !arrived.is_empty() {
            return arrived;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the notification must be delivered to the queue that grants Amazon SNS \
             {SEND_MESSAGE}, got nothing on {url}"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// Assert that `delivered` is the ONE notification a transition into `ALARM` of `row`'s alarm
/// owes: it names the alarm, the state it moved to and the series that decided it.
fn assert_one_alarm_notification(case: &str, row: &Row, delivered: &[Value]) {
    assert_eq!(
        delivered.len(),
        1,
        "case {case:?}: one transition into {ALARM_STATE} owes exactly one notification, got \
         {delivered:?}"
    );
    assert_alarm_notifications(case, row, delivered);
}

/// Assert that every entry of `delivered` is a notification of a transition into `ALARM` of `row`'s
/// alarm, and that there is at least one.
///
/// What a round whose transition this suite FORCES asserts: the alarm's series is breaching
/// throughout, so the evaluation the clock owes may take the alarm back into `ALARM` itself before
/// the force does — one round can therefore owe more than one transition, and what the rule fixes
/// about each of them is what every notification below is checked for.
fn assert_alarm_notifications(case: &str, row: &Row, delivered: &[Value]) {
    assert!(
        !delivered.is_empty(),
        "case {case:?}: a transition into {ALARM_STATE} owes a notification, got none"
    );
    for notification in delivered {
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
            notification["Trigger"]["MetricName"].as_str(),
            Some(row.metric),
            "case {case:?}: the notification must name the metric that decided it, got \
             {notification}"
        );
        assert_eq!(
            notification["Trigger"]["Namespace"].as_str(),
            Some(NAMESPACE),
            "case {case:?}: the notification must name the namespace it watched, got {notification}"
        );
    }
}

/// RULE: every row's alarm settles in `ALARM` on its own evaluation of the series Amazon ECS
/// published, and the notification that transition fires reaches the queue whose policy admits
/// Amazon SNS and no other.
#[tokio::test]
async fn an_ecs_container_insights_alarm_reaches_only_the_queue_its_policy_admits() {
    let router = router();
    let setup = "the Container Insights cluster";
    given_insights_cluster(&router, setup).await;

    // Every row watches a series of that one cluster, so the whole table runs on ONE emulator and
    // the evaluation the clock owes is waited for once rather than per row.
    let mut wired: Vec<Wiring> = Vec::with_capacity(ROWS.len());
    for row in ROWS {
        wait_for_published(&router, row).await;
        let wiring = given_fan_out(&router, row.case, row.alarm).await;
        cw(
            &router,
            row.case,
            REGION,
            "PutMetricAlarm",
            &alarm_of(row, &wiring.topic),
        )
        .await;
        wired.push(wiring);
    }

    for (row, wiring) in ROWS.iter().zip(&wired) {
        let case = row.case;
        let reported = settled_in_alarm(&router, case, row.alarm).await;

        // The verdict rests on the series Amazon ECS published, not on an empty window: the
        // explanation the transition recorded names the value that series carried.
        let measured = measured_values(case, &reported);
        assert!(
            measured.iter().any(|value| same_count(*value, row.published)),
            "case {case:?}: the state must be decided over the published {} of {}, got {measured:?} \
             in {reported}",
            row.published,
            row.metric
        );

        // The granted queue holds the notification, once: a second look right after it finds
        // nothing, so the one transition owed exactly the one delivery.
        let delivered = delivered_to(&router, case, &wiring.granted).await;
        assert_one_alarm_notification(case, row, &delivered);
        let again = drain(&router, case, REGION, &wiring.granted).await;
        assert!(
            again.is_empty(),
            "case {case:?}: one transition into {ALARM_STATE} owes one notification and no second, \
             got {again:?}"
        );

        // And the sibling subscribed to the very same topic, whose policy says nothing about the
        // delivery, holds nothing at all.
        let withheld = what_arrived_over_several_looks(&router, case, &wiring.silent).await;
        assert!(
            withheld.is_empty(),
            "case {case:?}: a subscribed queue granting Amazon SNS no {SEND_MESSAGE} must receive \
             nothing, got {withheld:?}"
        );
    }
}

/// The row the second test drives: the recorded shape, a service's running task count under the
/// one task a client watches for.
fn repaired_row() -> &'static Row {
    &ROWS[0]
}

/// Force one alarm into `state`, which is what makes the next transition this test's own rather
/// than the clock's.
async fn force(router: &MultiServiceRouter, case: &str, alarm: &str, state: &str) {
    cw(
        router,
        case,
        REGION,
        "SetAlarmState",
        &json!({
            "AlarmName": alarm,
            "StateValue": state,
            "StateReason": "driven by the test",
        }),
    )
    .await;
}

/// RULE, the other half: the queue-policy decision is taken when the delivery is MADE, so a queue
/// granted after a silent transition receives the next notification — while the granted sibling
/// receives one per transition, every round.
#[tokio::test]
async fn a_queue_granted_after_a_silent_round_receives_the_next_notification() {
    let router = router();
    let case = "the repaired queue";
    let row = repaired_row();
    given_insights_cluster(&router, case).await;
    wait_for_published(&router, row).await;

    let topic = element(
        &sns(&router, case, "CreateTopic", &[("Name", "insights-repair")]).await,
        "TopicArn",
    );
    let (granted, granted_arn) = given_queue(&router, case, "insights-repair-granted").await;
    let (repaired, repaired_arn) = given_queue(&router, case, "insights-repair-silent").await;
    for endpoint in [&granted_arn, &repaired_arn] {
        sns(
            &router,
            case,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", endpoint),
            ],
        )
        .await;
    }
    grant_sns_delivery(&router, case, &granted, &granted_arn, &topic).await;
    cw(
        &router,
        case,
        REGION,
        "PutMetricAlarm",
        &alarm_of(row, &topic),
    )
    .await;

    // Round one, the transition the clock owes: the granted queue receives it and the queue with
    // no policy does not.
    settled_in_alarm(&router, case, row.alarm).await;
    let delivered = delivered_to(&router, case, &granted).await;
    assert_one_alarm_notification(case, row, &delivered);
    let withheld = what_arrived_over_several_looks(&router, case, &repaired).await;
    assert!(
        withheld.is_empty(),
        "case {case:?}: the queue granting Amazon SNS no {SEND_MESSAGE} must receive nothing, got \
         {withheld:?}"
    );

    // The grant is added, and the next transition is made: the queue that was silent now holds the
    // notification, and so does the sibling that held the first one — one per transition, each
    // round.
    grant_sns_delivery(&router, case, &repaired, &repaired_arn, &topic).await;
    force(&router, case, row.alarm, OK_STATE).await;
    force(&router, case, row.alarm, ALARM_STATE).await;

    let delivered = delivered_to(&router, case, &granted).await;
    assert_alarm_notifications(case, row, &delivered);
    let arrived = delivered_to(&router, case, &repaired).await;
    assert_alarm_notifications(case, row, &arrived);
}
