// SPDX-License-Identifier: CC0-1.0

//! A table's write traffic drives a CloudWatch alarm to ALARM and the notification all the way
//! to a subscribed queue — through the WHOLE mounted service set, the one
//! `core/router/src/lib.rs` assembles for the binary.
//!
//! The RULE under test: **the capacity units a request consumes on a table are counted in the
//! `AWS/DynamoDB` namespace against that table — dimensioned by its own `TableName` — as the
//! `ConsumedWriteCapacityUnits` series, carrying exactly the figure the responses reported as
//! their `ConsumedCapacity` (a transactional write therefore counting twice what the same
//! single-item write counts), so a metric alarm whose threshold that consumption crosses
//! transitions to ALARM, publishes its SNS alarm action and the queue subscribed to that topic
//! receives the notification; a table nothing wrote to produces no datapoint, so its own alarm
//! stays `OK` under `TreatMissingData: notBreaching`.**
//!
//! Four services have to agree for that to hold — DynamoDB for the writes and what they are
//! charged, CloudWatch for the series and the evaluation, SNS for the fan-out and SQS for what
//! arrived — and the publish CloudWatch makes is an in-process call addressed the way a
//! client's own `sns`-scoped call is, so only the production service set can show the whole
//! chain. The crate-local suites pin each link:
//! `services/service-com-amazonaws-dynamodb-dynamodb-20120810/tests/all/transactional_write_capacity_arm.rs`
//! and its neighbours pin what a write reports and publishes over a table of surfaces, and the
//! cloudwatch crate's own suites pin the evaluation and the delivery.
//!
//! The rows differ along what the rule generalises over: which surface the write traffic is
//! made with — the transactional one, whose cost is doubled, and the item API — beside the
//! untouched control table whose alarm has the same configuration and no traffic at all.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{
    authorization, cw, drain, given_topic_with_subscriber, router, Grant,
};

/// The Region the whole wiring lives in: a table name resolves inside one Region, so the
/// series, the alarm, the topic and the queue live there too.
const REGION: &str = "us-east-1";

/// DynamoDB's `X-Amz-Target` prefix.
const DYNAMODB_TARGET_PREFIX: &str = "DynamoDB_20120810";

/// The namespace, metric and dimension the alarm watches: a table's own consumption series.
const DYNAMODB_NAMESPACE: &str = "AWS/DynamoDB";
const CONSUMED_WRITE_CAPACITY_UNITS: &str = "ConsumedWriteCapacityUnits";
const TABLE_NAME_DIMENSION: &str = "TableName";

/// The alarm's period, in seconds, and the units its threshold sits at. The threshold is below
/// what ONE of the calls below consumes, so a period that holds any of the traffic breaches it
/// — the window the traffic lands in is the clock's to choose, not this suite's.
const ALARM_PERIOD: i64 = 60;
const ALARM_THRESHOLD: f64 = 2.0;

/// How many calls of write traffic each row makes.
const CALLS: usize = 5;

/// How large the item each call writes is, in bytes of payload: several write capacity units,
/// so the figure the series carries is not one unit per call.
const PAYLOAD_BYTES: usize = 3_000;

/// How long a row waits for the chain to complete. An evaluation grades the period the
/// datapoints fell in once that period has been open for the evaluation cadence, so the
/// transition is seconds away rather than instant; the deadline is generous only so a failure
/// is a failure rather than a hang.
const CHAIN_DEADLINE: Duration = Duration::from_secs(180);

/// How long a table's create is given to settle, polled rather than waited out.
const ACTIVE_DEADLINE: Duration = Duration::from_secs(60);

/// How often a wait looks again.
const LOOK_AGAIN: Duration = Duration::from_millis(200);

/// The largest difference two capacity figures may show and still be the same figure: both
/// sides are sums of whole units, so anything above rounding is a real divergence.
const TOLERANCE: f64 = 1e-9;

/// One Amazon DynamoDB call (`awsJson1_0`, `DynamoDB_20120810.<Operation>`), answering the
/// status and the parsed body — a row that drives traffic asserts the status itself.
async fn ddb(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("dynamodb.{REGION}.amazonaws.com"))
        .header("authorization", authorization("dynamodb", REGION))
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("{DYNAMODB_TARGET_PREFIX}.{operation}"),
        )
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// [`ddb`] for a call that must succeed.
async fn ddb_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = ddb(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DynamoDB {operation} {body}, got {status} with \
         {parsed}"
    );
    parsed
}

/// An on-demand table with a single-attribute key, polled until it reports `ACTIVE` — which is
/// when it serves ("You can use the DescribeTable action to check the table status").
async fn given_table(router: &MultiServiceRouter, case: &str, name: &str) {
    ddb_ok(
        router,
        case,
        "CreateTable",
        &json!({
            "TableName": name,
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
}

/// Poll `DescribeTable` until `name` reports `ACTIVE`.
async fn active(router: &MultiServiceRouter, case: &str, name: &str) {
    let started = Instant::now();
    let mut last = Value::Null;
    while started.elapsed() < ACTIVE_DEADLINE {
        last = ddb_ok(router, case, "DescribeTable", &json!({"TableName": name})).await;
        if last["Table"]["TableStatus"] == json!("ACTIVE") {
            return;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    panic!("case {case:?}: the table {name:?} never reached ACTIVE, last {last}");
}

/// Create the alarm `name` over `table`'s write consumption, with `topic` as its alarm action.
async fn given_alarm(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    table: &str,
    topic: &str,
) {
    cw(
        router,
        case,
        REGION,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Namespace": DYNAMODB_NAMESPACE,
            "MetricName": CONSUMED_WRITE_CAPACITY_UNITS,
            "Dimensions": [{"Name": TABLE_NAME_DIMENSION, "Value": table}],
            // The units a period's requests spent are their SUM, which is how a consumption
            // metric is read and how an alarm on a table's utilisation is evaluated.
            "Statistic": "Sum",
            "Period": ALARM_PERIOD,
            "EvaluationPeriods": 1,
            "Threshold": ALARM_THRESHOLD,
            "ComparisonOperator": "GreaterThanThreshold",
            "TreatMissingData": "notBreaching",
            "AlarmActions": [topic],
        }),
    )
    .await;
}

/// What `DescribeAlarms` reports as `name`'s state.
async fn alarm_state(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let described = cw(
        router,
        case,
        REGION,
        "DescribeAlarms",
        &json!({"AlarmNames": [name]}),
    )
    .await;
    described["MetricAlarms"][0]["StateValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Wall-clock now as epoch seconds, the clock the series' windows are measured against.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// The write capacity units the series carries for `table`, summed over the window the traffic
/// fell in.
async fn published_units(router: &MultiServiceRouter, case: &str, table: &str, at: f64) -> f64 {
    let answered = cw(
        router,
        case,
        REGION,
        "GetMetricStatistics",
        &json!({
            "Namespace": DYNAMODB_NAMESPACE,
            "MetricName": CONSUMED_WRITE_CAPACITY_UNITS,
            "Dimensions": [{"Name": TABLE_NAME_DIMENSION, "Value": table}],
            "StartTime": at - 2.0 * ALARM_PERIOD as f64,
            "EndTime": at + 2.0 * ALARM_PERIOD as f64,
            "Period": ALARM_PERIOD,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    answered["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|datapoint| datapoint["Sum"].as_f64())
        .sum()
}

/// Every `WriteCapacityUnits` figure a response carries, summed: with `ReturnConsumedCapacity`
/// of `TOTAL` each entry carries one figure for the table it is about, and these rows address
/// one table per request, so this is what the request reported spending in write capacity.
fn reported_units(answered: &Value) -> f64 {
    match answered {
        Value::Object(members) => members
            .iter()
            .map(|(name, value)| {
                if name == "WriteCapacityUnits" {
                    value.as_f64().unwrap_or_default()
                } else {
                    reported_units(value)
                }
            })
            .sum(),
        Value::Array(entries) => entries.iter().map(reported_units).sum(),
        _ => 0.0,
    }
}

/// One item under `key`, with `PAYLOAD_BYTES` of payload beside it so a write of it costs
/// several capacity units.
fn item(key: &str) -> Value {
    json!({"pk": {"S": key}, "payload": {"S": "x".repeat(PAYLOAD_BYTES)}})
}

/// One row: the surface the write traffic is made with, and the bodies of the calls it makes.
struct Row {
    case: &'static str,
    operation: &'static str,
    /// One body per call, given the table to write into.
    bodies: fn(&str) -> Vec<Value>,
}

/// The rows: the transactional surface, whose cost is doubled, and the item API.
const ROWS: &[Row] = &[
    Row {
        case: "a transactional put per call",
        operation: "TransactWriteItems",
        bodies: |table| {
            (0..CALLS)
                .map(|index| {
                    json!({
                        "TransactItems": [{
                            "Put": {"TableName": table, "Item": item(&format!("t{index}"))},
                        }],
                        "ReturnConsumedCapacity": "TOTAL",
                    })
                })
                .collect()
        },
    },
    Row {
        case: "a single-item put per call",
        operation: "PutItem",
        bodies: |table| {
            (0..CALLS)
                .map(|index| {
                    json!({
                        "TableName": table,
                        "Item": item(&format!("p{index}")),
                        "ReturnConsumedCapacity": "TOTAL",
                    })
                })
                .collect()
        },
    },
];

#[tokio::test]
async fn write_traffic_fires_the_capacity_alarm_to_its_queue_while_the_untouched_table_stays_ok() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let router = router();
        let written = format!("alarm-subject-{index}");
        let quiet = format!("alarm-control-{index}");
        // Both creates are made before either wait, so the two tables settle together.
        given_table(&router, case, &written).await;
        given_table(&router, case, &quiet).await;
        active(&router, case, &written).await;
        active(&router, case, &quiet).await;

        // Each alarm publishes to its OWN topic and queue, so the control's silence is read
        // where a client reads it rather than inferred from a shared queue.
        let (subject_topic, subject_queue) = given_topic_with_subscriber(
            &router,
            case,
            REGION,
            &format!("ddb-capacity-subject-{index}"),
            Grant::Unscoped,
        )
        .await;
        let (quiet_topic, quiet_queue) = given_topic_with_subscriber(
            &router,
            case,
            REGION,
            &format!("ddb-capacity-control-{index}"),
            Grant::ScopedToTopic,
        )
        .await;
        let subject_alarm = format!("ddb-write-capacity-subject-{index}");
        let quiet_alarm = format!("ddb-write-capacity-control-{index}");
        given_alarm(&router, case, &subject_alarm, &written, &subject_topic).await;
        given_alarm(&router, case, &quiet_alarm, &quiet, &quiet_topic).await;

        // The traffic, and what every response reported spending on it.
        let at = now_secs();
        let mut reported = 0.0_f64;
        for body in (row.bodies)(&written) {
            let answered = ddb_ok(&router, case, row.operation, &body).await;
            reported += reported_units(&answered);
        }
        assert!(
            reported > ALARM_THRESHOLD,
            "case {case:?}: the traffic must report more than the alarm's {ALARM_THRESHOLD} \
             unit threshold, got {reported}"
        );

        // What the series carries is what the responses reported: one cost, read in either
        // plane.
        let published = published_units(&router, case, &written, at).await;
        assert!(
            (published - reported).abs() < TOLERANCE,
            "case {case:?}: expected the {CONSUMED_WRITE_CAPACITY_UNITS} series of {written:?} \
             to carry the {reported} units the responses reported, got {published}"
        );
        // Nothing was written to the control table, so it has no series to breach on.
        let untouched = published_units(&router, case, &quiet, at).await;
        assert!(
            untouched.abs() < TOLERANCE,
            "case {case:?}: expected no consumption for the untouched {quiet:?}, got {untouched}"
        );

        // The chain: the evaluation the clock owes moves the alarm, its action publishes, and
        // the subscribed queue receives the notification — with no call forcing any of it. The
        // control alarm is looked at every time round, so its silence is checked for as long
        // as the subject's chain is in flight rather than after a fixed wait.
        let started = Instant::now();
        let mut delivered: Vec<Value> = Vec::new();
        while started.elapsed() < CHAIN_DEADLINE && delivered.is_empty() {
            delivered.extend(drain(&router, case, REGION, &subject_queue).await);
            let standing = alarm_state(&router, case, &quiet_alarm).await;
            assert_eq!(
                standing, "OK",
                "case {case:?}: the alarm of the table nothing wrote to must stay OK, got \
                 {standing:?}"
            );
        }
        assert!(
            !delivered.is_empty(),
            "case {case:?}: the alarm's action must reach the queue subscribed to its topic"
        );
        for notification in &delivered {
            assert_eq!(
                notification["AlarmName"].as_str(),
                Some(subject_alarm.as_str()),
                "case {case:?}: the notification must name the alarm, got {notification}"
            );
            assert_eq!(
                notification["NewStateValue"].as_str(),
                Some("ALARM"),
                "case {case:?}: the notification must carry the state moved to, got \
                 {notification}"
            );
        }
        let state = alarm_state(&router, case, &subject_alarm).await;
        assert_eq!(
            state, "ALARM",
            "case {case:?}: the alarm the traffic breached must report ALARM, got {state:?}"
        );

        // And nothing was delivered for the alarm that never fired.
        let withheld = drain(&router, case, REGION, &quiet_queue).await;
        assert!(
            withheld.is_empty(),
            "case {case:?}: nothing may be delivered for an alarm that never fired, got \
             {withheld:?}"
        );
    }
}
