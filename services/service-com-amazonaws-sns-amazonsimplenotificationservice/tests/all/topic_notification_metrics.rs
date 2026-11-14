// SPDX-License-Identifier: CC0-1.0

//! The `AWS/SNS` counters a topic publishes about its own fan-out, read where a client reads
//! them: through CloudWatch's own `GetMetricStatistics` and `ListMetrics`, with SNS, SQS and
//! CloudWatch mounted on ONE shared engine exactly as `core/router` assembles them.
//!
//! RULE under test: a topic reports what its publish path did as four INDEPENDENT `AWS/SNS`
//! series dimensioned by `TopicName` — one count per publish the topic accepted
//! (`NumberOfMessagesPublished`), one per subscription the message was offered to whose
//! filter policy did not admit it (`NumberOfNotificationsFilteredOut`), one per notification
//! a subscribed endpoint took (`NumberOfNotificationsDelivered`) and one per notification it
//! did not (`NumberOfNotificationsFailed`). No series absorbs another: a message a filter
//! withheld is counted as filtered out and nowhere else, a notification an endpoint refused
//! is counted as failed and not as delivered, and a counter nothing contributed to carries
//! NO datapoint rather than a zero. Each count is of a real event, so a `Sum` over a window
//! is what that window carried — which is what lets a client tell "the filter withheld it"
//! from "the endpoint refused it" instead of seeing an idle topic.
//!
//! The table spans what the rule generalises over: how many publishes are made, how they are
//! made (one at a time or as one batch), how many subscriptions the topic has (none, one,
//! two), whether each subscription's filter policy admits the message, and WHY a notification
//! fails (a queue policy that denies the send, or an endpoint naming a queue that does not
//! exist). The case the issue recorded — one filtered subscription and one denying
//! subscription, one admitted publish and five withheld — is ONE row. Every row runs on its
//! own emulator and its own topic, so the totals each row asserts are its own.
//!
//! The counters are published as part of serving the publishing call, so nothing here waits
//! for a clock: the read that follows the publish sees what the publish counted.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
// The namespace, dimension, metric names and unit under test, read from the publication rule
// itself so the suite and the publisher cannot disagree about which series is meant.
use service_com_amazonaws_sns_amazonsimplenotificationservice::state::topic_metrics::{
    COUNT_UNIT, NAMESPACE, NUMBER_OF_MESSAGES_PUBLISHED, NUMBER_OF_NOTIFICATIONS_DELIVERED,
    NUMBER_OF_NOTIFICATIONS_FAILED, NUMBER_OF_NOTIFICATIONS_FILTERED_OUT, TOPIC_NAME_DIMENSION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::topics_subscriptions_publish::percent_encode;

/// The Region every call below is signed for.
const REGION: &str = "us-east-1";

/// The endpoint a CloudWatch client addresses.
const CLOUDWATCH_HOST: &str = "monitoring.us-east-1.amazonaws.com";

/// The `X-Amz-Target` prefix the CloudWatch model declares.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// How many seconds either side of the calls a metric read spans: a window wide enough that
/// every count of a test falls inside it whichever minute the clock is in when it runs.
const WINDOW_SECS: f64 = 300.0;

/// The period a read folds that window into: one minute, the resolution these counters are
/// published at.
const PERIOD_SECS: i64 = 60;

/// The message attribute every publish here carries, and the one a filter policy selects on.
const PROBE_ATTRIBUTE: &str = "probe";

/// The attribute value a filtered subscription's policy admits.
const ADMITTED: &str = "admit";

/// The attribute value no filter policy here admits.
const WITHHELD: &str = "withhold";

/// SNS, SQS and CloudWatch on ONE engine: a subscription's endpoint is a queue, and the
/// counters the fan-out contributes to are read from CloudWatch's own store.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    Emulator { router }
}

/// One awsQuery form call against SNS that must succeed, answering its body.
async fn sns(emulator: &Emulator, action: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version=2010-03-31");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        // The form encoding is the suite-shared one, not a second copy.
        body.push_str(&percent_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/sns/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "{action}: expected HTTP 200, got {status} / {text}"
    );
    text
}

/// One `awsJson1_0` call against SQS that must succeed.
async fn sqs(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/sqs/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "{operation}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// One `awsJson1_0` call against CloudWatch that must succeed.
async fn cloudwatch(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", CLOUDWATCH_HOST)
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/monitoring/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "{operation}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// The text of the first `<tag>…</tag>` element of an awsQuery response.
fn element(body: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("no <{tag}> in {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("no </{tag}> in {body}"));
    body[start..start + end].to_string()
}

/// The field of `arn` at `at`, counted from `arn:`.
fn field(arn: &str, at: usize) -> String {
    arn.split(':')
        .nth(at)
        .unwrap_or_else(|| panic!("no field {at} in {arn}"))
        .to_string()
}

/// The queue policy AWS documents for an SQS queue subscribed to an SNS topic: the service
/// principal `sns.amazonaws.com`, allowed `sqs:SendMessage` on that queue.
fn grant_to_sns(queue_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowSNSDelivery",
            "Effect": "Allow",
            "Principal": {"Service": "sns.amazonaws.com"},
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
        }],
    })
    .to_string()
}

/// The same statement as a `Deny`: the policy of a queue that explicitly refuses SNS the
/// send, which is what makes its notifications FAIL.
fn deny_to_sns(queue_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "DenySNSDelivery",
            "Effect": "Deny",
            "Principal": {"Service": "sns.amazonaws.com"},
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
        }],
    })
    .to_string()
}

/// What a subscription's endpoint is, which is what decides whether its notifications are
/// taken or fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Endpoint {
    /// A queue whose policy grants SNS the send: it TAKES every notification.
    Granting,
    /// A queue whose policy denies SNS the send: it takes none.
    Denying,
    /// An endpoint ARN naming a queue that does not exist: nothing is there to take one.
    Absent,
}

/// One subscription of the row's topic: the endpoint it names, and the attribute value its
/// filter policy admits — `None` for a subscription that asks for no filtering and so
/// receives every message.
#[derive(Debug, Clone, Copy)]
struct Subscriber {
    endpoint: Endpoint,
    admits: Option<&'static str>,
}

/// How a row publishes: one message at a time, or several in one `PublishBatch`.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// One `Publish` carrying the probe attribute's value.
    One(&'static str),
    /// One `PublishBatch` whose entries carry those values.
    Batch(&'static [&'static str]),
}

/// One row: what the topic has subscribed to it, what is published to it, and the total each
/// of the four counters must then report — `None` where the counter must have NO datapoint.
struct Row {
    case: &'static str,
    topic: &'static str,
    subscribers: &'static [Subscriber],
    steps: &'static [Step],
    published: Option<f64>,
    delivered: Option<f64>,
    failed: Option<f64>,
    filtered_out: Option<f64>,
}

const GRANTING: Subscriber = Subscriber {
    endpoint: Endpoint::Granting,
    admits: None,
};
const FILTERED: Subscriber = Subscriber {
    endpoint: Endpoint::Granting,
    admits: Some(ADMITTED),
};
const DENYING: Subscriber = Subscriber {
    endpoint: Endpoint::Denying,
    admits: None,
};
const ABSENT: Subscriber = Subscriber {
    endpoint: Endpoint::Absent,
    admits: None,
};

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a topic with no subscription counts the publish and no notification",
            topic: "counters-no-subscriber",
            subscribers: &[],
            steps: &[Step::One(ADMITTED)],
            published: Some(1.0),
            delivered: None,
            failed: None,
            filtered_out: None,
        },
        Row {
            case: "a subscriber that takes the notification is counted as delivered, once per \
                   publish",
            topic: "counters-granting",
            subscribers: &[GRANTING],
            steps: &[
                Step::One(ADMITTED),
                Step::One(WITHHELD),
                Step::One(ADMITTED),
            ],
            published: Some(3.0),
            delivered: Some(3.0),
            failed: None,
            filtered_out: None,
        },
        Row {
            case: "an endpoint whose policy denies the send fails every notification and \
                   delivers none",
            topic: "counters-denying",
            subscribers: &[DENYING],
            steps: &[Step::One(ADMITTED), Step::One(WITHHELD)],
            published: Some(2.0),
            delivered: None,
            failed: Some(2.0),
            filtered_out: None,
        },
        Row {
            case: "an endpoint that is not there fails its notification too",
            topic: "counters-absent-endpoint",
            subscribers: &[ABSENT],
            steps: &[Step::One(ADMITTED)],
            published: Some(1.0),
            delivered: None,
            failed: Some(1.0),
            filtered_out: None,
        },
        Row {
            case: "a filter policy that does not admit the message counts it as filtered out \
                   and neither delivered nor failed",
            topic: "counters-filtered",
            subscribers: &[FILTERED],
            steps: &[
                Step::One(ADMITTED),
                Step::One(WITHHELD),
                Step::One(WITHHELD),
            ],
            published: Some(3.0),
            delivered: Some(1.0),
            failed: None,
            filtered_out: Some(2.0),
        },
        Row {
            case: "a filtered subscription and a denying one separate the withheld messages \
                   from the refused notifications",
            topic: "counters-filtered-and-denying",
            subscribers: &[FILTERED, DENYING],
            steps: &[
                Step::One(ADMITTED),
                Step::One(WITHHELD),
                Step::One(WITHHELD),
                Step::One(WITHHELD),
                Step::One(WITHHELD),
                Step::One(WITHHELD),
            ],
            published: Some(6.0),
            delivered: Some(1.0),
            failed: Some(6.0),
            filtered_out: Some(5.0),
        },
        Row {
            case: "a notification is counted per subscription the message reaches, not per \
                   publish",
            topic: "counters-fan-out",
            subscribers: &[GRANTING, GRANTING],
            steps: &[Step::One(ADMITTED), Step::One(WITHHELD)],
            published: Some(2.0),
            delivered: Some(4.0),
            failed: None,
            filtered_out: None,
        },
        Row {
            case: "every entry of a batched publish is counted on its own",
            topic: "counters-batch",
            subscribers: &[FILTERED],
            steps: &[Step::Batch(&[ADMITTED, WITHHELD, ADMITTED])],
            published: Some(3.0),
            delivered: Some(2.0),
            failed: None,
            filtered_out: Some(1.0),
        },
        Row {
            case: "one message withheld by one subscription's filter and refused by another's \
                   endpoint is counted in both series",
            topic: "counters-filtered-and-absent",
            subscribers: &[FILTERED, ABSENT],
            steps: &[Step::One(WITHHELD)],
            published: Some(1.0),
            delivered: None,
            failed: Some(1.0),
            filtered_out: Some(1.0),
        },
    ]
}

/// Create the row's topic, its subscriptions and their queues, and answer the topic's ARN.
async fn given_topic(emulator: &Emulator, row: &Row) -> String {
    let topic = element(
        &sns(emulator, "CreateTopic", &[("Name", row.topic)]).await,
        "TopicArn",
    );
    for (at, subscriber) in row.subscribers.iter().enumerate() {
        let endpoint = given_endpoint(emulator, &topic, row.topic, at, subscriber.endpoint).await;
        let mut params: Vec<(String, String)> = vec![
            ("TopicArn".to_string(), topic.clone()),
            ("Protocol".to_string(), "sqs".to_string()),
            ("Endpoint".to_string(), endpoint),
            ("ReturnSubscriptionArn".to_string(), "true".to_string()),
        ];
        if let Some(admitted) = subscriber.admits {
            params.push((
                "Attributes.entry.1.key".to_string(),
                "FilterPolicy".to_string(),
            ));
            params.push((
                "Attributes.entry.1.value".to_string(),
                json!({PROBE_ATTRIBUTE: [admitted]}).to_string(),
            ));
        }
        let borrowed: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        sns(emulator, "Subscribe", &borrowed).await;
    }
    topic
}

/// The endpoint ARN one subscription of the row names: a queue carrying the policy its kind
/// calls for, or — for [`Endpoint::Absent`] — the ARN of a queue nothing created, in the
/// Region and account of the topic itself.
async fn given_endpoint(
    emulator: &Emulator,
    topic_arn: &str,
    topic: &str,
    at: usize,
    endpoint: Endpoint,
) -> String {
    let name = format!("{topic}-queue-{at}");
    if endpoint == Endpoint::Absent {
        return format!(
            "arn:aws:sqs:{}:{}:{name}-absent",
            field(topic_arn, 3),
            field(topic_arn, 4)
        );
    }
    let created = sqs(emulator, "CreateQueue", &json!({"QueueName": name})).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue {name}: no QueueUrl in {created}"))
        .to_string();
    let attributes = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("GetQueueAttributes {url}: no QueueArn in {attributes}"))
        .to_string();
    let policy = match endpoint {
        Endpoint::Granting => grant_to_sns(&arn),
        Endpoint::Denying => deny_to_sns(&arn),
        Endpoint::Absent => unreachable!("an absent endpoint has no queue to write a policy on"),
    };
    sqs(
        emulator,
        "SetQueueAttributes",
        &json!({"QueueUrl": url, "Attributes": {"Policy": policy}}),
    )
    .await;
    arn
}

/// Perform one publishing step against `topic`.
async fn publish(emulator: &Emulator, topic: &str, step: Step) {
    match step {
        Step::One(probe) => {
            sns(
                emulator,
                "Publish",
                &[
                    ("TopicArn", topic),
                    ("Message", "a notification"),
                    ("MessageAttributes.entry.1.Name", PROBE_ATTRIBUTE),
                    ("MessageAttributes.entry.1.Value.DataType", "String"),
                    ("MessageAttributes.entry.1.Value.StringValue", probe),
                ],
            )
            .await;
        }
        Step::Batch(probes) => {
            let mut params: Vec<(String, String)> =
                vec![("TopicArn".to_string(), topic.to_string())];
            for (at, probe) in probes.iter().enumerate() {
                let prefix = format!("PublishBatchRequestEntries.member.{}", at + 1);
                params.push((format!("{prefix}.Id"), format!("entry-{at}")));
                params.push((format!("{prefix}.Message"), "a notification".to_string()));
                params.push((
                    format!("{prefix}.MessageAttributes.entry.1.Name"),
                    PROBE_ATTRIBUTE.to_string(),
                ));
                params.push((
                    format!("{prefix}.MessageAttributes.entry.1.Value.DataType"),
                    "String".to_string(),
                ));
                params.push((
                    format!("{prefix}.MessageAttributes.entry.1.Value.StringValue"),
                    (*probe).to_string(),
                ));
            }
            let borrowed: Vec<(&str, &str)> = params
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            sns(emulator, "PublishBatch", &borrowed).await;
        }
    }
}

/// Wall-clock now as epoch seconds, the clock a metric window is measured against.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// `GetMetricStatistics` over one `AWS/SNS` series of `topic` across the window around the
/// calls: the datapoints it holds.
async fn datapoints(emulator: &Emulator, topic: &str, metric: &str) -> Vec<Value> {
    let now = now_secs();
    let answered = cloudwatch(
        emulator,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": [{"Name": TOPIC_NAME_DIMENSION, "Value": topic}],
            "StartTime": now - WINDOW_SECS,
            "EndTime": now + WINDOW_SECS,
            "Period": PERIOD_SECS,
            "Statistics": ["Sum", "SampleCount", "Minimum", "Maximum"],
        }),
    )
    .await;
    answered
        .get("Datapoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// What `metric` totals for `topic` across the window — the `Sum` of every datapoint it
/// holds — or `None` when the series holds none, which is what a counter nothing contributed
/// to reports.
async fn total(emulator: &Emulator, topic: &str, metric: &str) -> Option<f64> {
    let points = datapoints(emulator, topic, metric).await;
    if points.is_empty() {
        return None;
    }
    Some(
        points
            .iter()
            .filter_map(|point| point.get("Sum").and_then(Value::as_f64))
            .sum(),
    )
}

/// The `AWS/SNS` metric names `ListMetrics` indexes for `topic`, sorted.
async fn indexed_series(emulator: &Emulator, topic: &str) -> Vec<String> {
    let listed = cloudwatch(
        emulator,
        "ListMetrics",
        &json!({
            "Namespace": NAMESPACE,
            "Dimensions": [{"Name": TOPIC_NAME_DIMENSION, "Value": topic}],
        }),
    )
    .await;
    let mut names: Vec<String> = listed
        .get("Metrics")
        .and_then(Value::as_array)
        .map(|metrics| {
            metrics
                .iter()
                .filter_map(|metric| {
                    metric
                        .get("MetricName")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[tokio::test]
async fn the_topic_counters_count_the_publishes_the_withheld_and_the_refused() {
    for row in rows() {
        // One emulator per row, so each row's totals are its own.
        let emulator = emulator();
        let topic = given_topic(&emulator, &row).await;
        for step in row.steps {
            publish(&emulator, &topic, *step).await;
        }

        for (metric, expected) in [
            (NUMBER_OF_MESSAGES_PUBLISHED, row.published),
            (NUMBER_OF_NOTIFICATIONS_DELIVERED, row.delivered),
            (NUMBER_OF_NOTIFICATIONS_FAILED, row.failed),
            (NUMBER_OF_NOTIFICATIONS_FILTERED_OUT, row.filtered_out),
        ] {
            let actual = total(&emulator, row.topic, metric).await;
            match expected {
                Some(expected) => assert_eq!(
                    actual,
                    Some(expected),
                    "case {:?}: {metric} for TopicName={} must total {expected}, got {actual:?}",
                    row.case,
                    row.topic
                ),
                None => assert_eq!(
                    actual, None,
                    "case {:?}: {metric} for TopicName={} must have no datapoint at all, got \
                     {actual:?}",
                    row.case, row.topic
                ),
            }
        }
    }
}

#[tokio::test]
async fn each_counted_event_is_one_observation_in_the_count_unit() {
    let emulator = emulator();
    let row = Row {
        case: "two publishes to a filtered and a denying subscriber",
        topic: "counters-unit",
        subscribers: &[FILTERED, DENYING],
        steps: &[Step::One(ADMITTED), Step::One(WITHHELD)],
        published: Some(2.0),
        delivered: Some(1.0),
        failed: Some(2.0),
        filtered_out: Some(1.0),
    };
    let topic = given_topic(&emulator, &row).await;
    for step in row.steps {
        publish(&emulator, &topic, *step).await;
    }

    for metric in [
        NUMBER_OF_MESSAGES_PUBLISHED,
        NUMBER_OF_NOTIFICATIONS_DELIVERED,
        NUMBER_OF_NOTIFICATIONS_FAILED,
        NUMBER_OF_NOTIFICATIONS_FILTERED_OUT,
    ] {
        let points = datapoints(&emulator, row.topic, metric).await;
        assert!(
            !points.is_empty(),
            "case {metric}: the series must hold a datapoint"
        );
        for point in &points {
            assert_eq!(
                point.get("Unit").and_then(Value::as_str),
                Some(COUNT_UNIT),
                "case {metric}: a counted event is published in {COUNT_UNIT}, got {point}"
            );
            // Each counted event contributed exactly one, so the sum of a datapoint is the
            // number of observations it stands for and neither its smallest nor its largest
            // observation is anything but one.
            assert_eq!(
                point.get("Sum").and_then(Value::as_f64),
                point.get("SampleCount").and_then(Value::as_f64),
                "case {metric}: every observation counts one, got {point}"
            );
            assert_eq!(
                point.get("Maximum").and_then(Value::as_f64),
                Some(1.0),
                "case {metric}: one event is one observation of 1, got {point}"
            );
        }
    }

    // A client discovering what a topic reports finds exactly the series its traffic
    // contributed to.
    let mut expected = vec![
        NUMBER_OF_MESSAGES_PUBLISHED.to_string(),
        NUMBER_OF_NOTIFICATIONS_DELIVERED.to_string(),
        NUMBER_OF_NOTIFICATIONS_FAILED.to_string(),
        NUMBER_OF_NOTIFICATIONS_FILTERED_OUT.to_string(),
    ];
    expected.sort();
    let indexed = indexed_series(&emulator, row.topic).await;
    assert_eq!(
        indexed, expected,
        "case {:?}: ListMetrics must index the series the topic published, got {indexed:?}",
        row.case
    );
}

#[tokio::test]
async fn a_topic_nobody_published_to_reports_no_series_at_all() {
    let emulator = emulator();
    // The topic EXISTS and has a healthy subscriber: what it lacks is traffic.
    let row = Row {
        case: "a topic created and subscribed to, never published to",
        topic: "counters-idle",
        subscribers: &[GRANTING],
        steps: &[],
        published: None,
        delivered: None,
        failed: None,
        filtered_out: None,
    };
    given_topic(&emulator, &row).await;

    let indexed = indexed_series(&emulator, row.topic).await;
    assert!(
        indexed.is_empty(),
        "case {:?}: a topic nothing was published to carries no series, got {indexed:?}",
        row.case
    );
}
