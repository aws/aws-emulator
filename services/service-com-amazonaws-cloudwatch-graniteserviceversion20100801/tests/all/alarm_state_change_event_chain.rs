// SPDX-License-Identifier: CC0-1.0

//! The state-change events of ONE alarm form a CHAIN, end to end over the wire through the real
//! `MultiServiceRouter` with CloudWatch, EventBridge and Amazon SQS on one engine.
//!
//! The RULE asserted here: **every `CloudWatch Alarm State Change` event of one alarm reports as
//! `detail.previousState` exactly what the event before it for that alarm reported as
//! `detail.state` — the state the alarm moved out of is the state the preceding transition moved
//! it into, with the same value, the same explanation and the same instant — so a consumer
//! following one alarm's transitions sees each continue the one before, and the first event of an
//! alarm reports the state its creation left it in.** The chain holds whatever MOVED the alarm
//! and whatever happened to the alarm's own members in between: a write that re-states the state
//! the alarm is already in moves it nowhere and publishes nothing, while leaving the alarm
//! carrying its own explanation of that state — so a previous state read off the alarm as it
//! stands at the next transition is a state no event of that alarm ever announced.
//!
//! The rows differ along what the rule generalises over: WHAT drove each transition (a state a
//! caller forced, a datapoint the alarm's own evaluation graded, a window with no datapoint at
//! all, and those mixed), HOW MANY transitions the alarm went through, WHICH states it moved
//! through, WHETHER the state the alarm carries was RE-STATED under it between two transitions —
//! a write that states the state the alarm is already in moves it nowhere, so it publishes
//! nothing while leaving the alarm carrying another explanation of that state than the event
//! announced — and WHETHER the states being chained carry a DECISION RECORD at all: a state an
//! evaluation decided carries the record of THAT evaluation, while the state a creation left the
//! alarm in carries an explanation and no record, since nothing was evaluated for it.
//!
//! What that makes the chain assert, member for member, is that the record a state is announced
//! with is the record it is still reported with when the alarm moves out of it — the span of the
//! series and the instant of the evaluation that PUT the alarm in that state, and never the span
//! or the instant of the later evaluation that took it out.
//!
//! The events are ordered by the instant each reports rather than by the order they were read
//! off the queue, so what is asserted is the chain itself and not the delivery order of a
//! standard queue.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    document_instant_secs, given_queue, graded_window_stamp, received_messages, rpc, Emulator,
    REGION,
};

/// The `source` and `detail-type` every event below is matched on.
const SOURCE: &str = "aws.cloudwatch";
const DETAIL_TYPE: &str = "CloudWatch Alarm State Change";

/// The service principal EventBridge delivers to an SQS target as, which the queue's own policy
/// has to allow the send for.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// The namespace and metric the alarms below watch.
const NAMESPACE: &str = "Custom/StateChangeChain";
const METRIC: &str = "Backlog";

/// The period those alarms aggregate over: a high-resolution period, so a row that needs a
/// SECOND window of its own series waits seconds for it rather than minutes.
const PERIOD: i64 = 10;

/// The threshold they compare against, and the two values the rows publish: one over it and one
/// under it.
const THRESHOLD: f64 = 50.0;
const BREACHING: f64 = 100.0;
const NOT_BREACHING: f64 = 0.0;

/// The state every new alarm starts in, which is what the first event of an alarm reports as the
/// state it moved out of.
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// How far back in its own series the row whose last link is decided by a window with NO datapoint
/// publishes, in seconds: inside the evaluation range an alarm of [`PERIOD`] searches
/// (`crate::state::alarm_evaluation::MINIMUM_EVALUATION_RANGE_SPAN_SECS`, the span of clock that
/// range reaches back over) and close to its old end, so the clock carries the row's newest
/// datapoint out of the range within seconds of the publishing stopping — with room left for the
/// half-window the stamp this age is measured from already sits behind the clock
/// ([`crate::harness::graded_window_stamp`]).
const AGE_NEAR_THE_END_OF_THE_RANGE: f64 = 150.0;

/// How long this suite waits for work the clock owes before giving up on it. Generous on
/// purpose: nothing here is asserted about how fast the host runs, only about what eventually
/// arrives.
const WORK_WAIT: Duration = Duration::from_secs(120);

/// How often it looks while waiting.
const LOOK_INTERVAL: Duration = Duration::from_millis(100);

/// CloudWatch, EventBridge and Amazon SQS over ONE shared engine, as `core/router/src/lib.rs`
/// mounts them.
fn emulator_with_a_bus() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    Emulator { router, engine }
}

/// One EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn eventbridge(emulator: &Emulator, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/events/\
                 aws4_request, SignedHeaders=host, Signature=0000"
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
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One CloudWatch call that must answer 200.
async fn monitoring(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    rpc(
        emulator,
        "monitoring",
        crate::harness::TARGET_PREFIX,
        operation,
        body,
    )
    .await
}

/// A queue a rule can target, with its own policy allowing EventBridge the send.
async fn given_target_queue(emulator: &Emulator, name: &str) -> String {
    let (url, arn) = given_queue(emulator, name).await;
    rpc(
        emulator,
        "sqs",
        "AmazonSQS",
        "SetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "Attributes": {"Policy": json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "AllowEventBridgeDelivery",
                    "Effect": "Allow",
                    "Principal": {"Service": EVENTS_PRINCIPAL},
                    "Action": "sqs:SendMessage",
                    "Resource": arn,
                }],
            })
            .to_string()},
        }),
    )
    .await;
    url
}

/// The ARN of the queue at `url`.
async fn queue_arn_of(emulator: &Emulator, url: &str) -> String {
    let attributes = rpc(
        emulator,
        "sqs",
        "AmazonSQS",
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("GetQueueAttributes {url}: no QueueArn in {attributes}"))
        .to_string()
}

/// Wire a rule matching the state changes of the alarm `alarm_name` to the queue at `url`.
async fn given_rule_for_alarm(
    emulator: &Emulator,
    case: &str,
    rule_name: &str,
    alarm_name: &str,
    queue_url: &str,
) {
    let pattern = json!({
        "source": [SOURCE],
        "detail-type": [DETAIL_TYPE],
        "detail": {"alarmName": [alarm_name]},
    })
    .to_string();
    eventbridge(
        emulator,
        case,
        "PutRule",
        &json!({"Name": rule_name, "EventPattern": pattern, "State": "ENABLED"}),
    )
    .await;
    let queue_arn = queue_arn_of(emulator, queue_url).await;
    let wired = eventbridge(
        emulator,
        case,
        "PutTargets",
        &json!({"Rule": rule_name, "Targets": [{"Id": "q", "Arn": queue_arn}]}),
    )
    .await;
    assert_eq!(
        wired["FailedEntryCount"],
        json!(0),
        "case {case:?}: expected the target to be accepted, got {wired}"
    );
}

/// The alarm of this suite: one metric, dimensioned by the alarm's own name, which keeps the
/// state it is in while its series holds no datapoint in range ("ignore — the current alarm
/// state is maintained", `PutMetricAlarmInput$TreatMissingData`).
async fn given_alarm(emulator: &Emulator, name: &str) {
    monitoring(
        emulator,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "AlarmDescription": "the alarm of this row",
            "Namespace": NAMESPACE,
            "MetricName": METRIC,
            "Dimensions": [{"Name": "QueueName", "Value": name}],
            "Statistic": "Average",
            "Period": PERIOD,
            "EvaluationPeriods": 1,
            "Threshold": THRESHOLD,
            "ComparisonOperator": "GreaterThanThreshold",
            "TreatMissingData": "ignore",
        }),
    )
    .await;
}

/// Publish `value` into the newest window of the alarm's series an evaluation grades.
async fn publish(emulator: &Emulator, alarm: &str, value: f64) {
    monitoring(
        emulator,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{
                "MetricName": METRIC,
                "Dimensions": [{"Name": "QueueName", "Value": alarm}],
                "Timestamp": graded_window_stamp(PERIOD as f64),
                "Value": value,
            }],
        }),
    )
    .await;
}

/// Publish `value` into the window `age_secs` BEFORE the newest window of the alarm's series an
/// evaluation grades, at HIGH resolution.
///
/// A datapoint is graded in the window its own time stamp falls in, so a row that needs its series
/// to run out inside its own wait places its datapoints near the old end of the evaluation range
/// ([`AGE_NEAR_THE_END_OF_THE_RANGE`]) instead of waiting out the whole span that range reaches
/// back over. High resolution for the same reason: "Setting this to 1 specifies this metric as a
/// high-resolution metric" (`MetricDatum$StorageResolution`), which is the granularity at which
/// each of this row's datapoints sits in a window of its own — a regular-resolution series holds
/// one datapoint per minute, so two values of such a row would be aggregated into one window and
/// the second could not move the alarm.
async fn publish_at_age(emulator: &Emulator, alarm: &str, value: f64, age_secs: f64) {
    monitoring(
        emulator,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{
                "MetricName": METRIC,
                "Dimensions": [{"Name": "QueueName", "Value": alarm}],
                "StorageResolution": 1,
                "Timestamp": graded_window_stamp(PERIOD as f64) - age_secs,
                "Value": value,
            }],
        }),
    )
    .await;
}

/// Take whatever has been delivered to the queue at `url` and add it to `held`, BY EVENT ID: a
/// standard queue delivers a message at least once, and a message a reader takes without deleting
/// it becomes visible again once its visibility timeout passes — so one event read more than once
/// is still one event, de-duplicated here by the `id` the bus gave it, the way a consumer
/// following one alarm does.
async fn collect(emulator: &Emulator, url: &str, held: &mut Vec<Value>) {
    for message in received_messages(emulator, url).await {
        let body = message["Body"].as_str().unwrap_or_default();
        if let Ok(parsed) = serde_json::from_str::<Value>(body) {
            let already_held = parsed
                .get("id")
                .is_some_and(|id| held.iter().any(|event| event.get("id") == Some(id)));
            if !already_held {
                held.push(parsed);
            }
        }
    }
}

/// Whether `held` already holds an event reporting the state `state`.
fn holds_state(held: &[Value], state: &str) -> bool {
    held.iter()
        .any(|event| event.pointer("/detail/state/value") == Some(&json!(state)))
}

/// How many looks fit in the wait this suite gives work the clock owes.
fn looks() -> u128 {
    WORK_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1)
}

/// Wait until `held` holds `wanted` events, adding everything delivered meanwhile to it. The
/// wait is for the CONDITION rather than for a span.
async fn until_count(emulator: &Emulator, url: &str, wanted: usize, held: &mut Vec<Value>) {
    for _ in 0..looks() {
        collect(emulator, url, held).await;
        if held.len() >= wanted {
            return;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    collect(emulator, url, held).await;
}

/// Wait until an event reporting the state `state` has reached the queue, adding everything
/// delivered meanwhile to `held`. The wait is for the CONDITION rather than for a span.
async fn until_state(emulator: &Emulator, url: &str, state: &str, held: &mut Vec<Value>) {
    for _ in 0..looks() {
        collect(emulator, url, held).await;
        if holds_state(held, state) {
            return;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    collect(emulator, url, held).await;
}

/// Drive the alarm into `state` with datapoints of its own series, publishing `value` on every
/// look until the event reporting that state arrives.
///
/// A datapoint is graded in the window its own time stamp falls in and an alarm grades the
/// windows that have CLOSED, so publishing again on every look keeps the drive independent of
/// how many period boundaries the clock crosses while it waits — and the repeated grading of a
/// state the alarm has already reached is what rewrites that state's explanation underneath it.
async fn until_evaluated(
    emulator: &Emulator,
    url: &str,
    alarm: &str,
    value: f64,
    state: &str,
    held: &mut Vec<Value>,
) {
    for _ in 0..looks() {
        publish(emulator, alarm, value).await;
        collect(emulator, url, held).await;
        if holds_state(held, state) {
            return;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    collect(emulator, url, held).await;
}

/// The same drive, publishing each datapoint `age_secs` before the newest window the alarm grades
/// ([`publish_at_age`]) — for the row whose last link is decided by its series running out.
async fn until_evaluated_at_age(
    emulator: &Emulator,
    url: &str,
    alarm: &str,
    value: f64,
    age_secs: f64,
    state: &str,
    held: &mut Vec<Value>,
) {
    for _ in 0..looks() {
        publish_at_age(emulator, alarm, value, age_secs).await;
        collect(emulator, url, held).await;
        if holds_state(held, state) {
            return;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    collect(emulator, url, held).await;
}

/// The alarm's own state as `DescribeAlarms` reports it: the value, the explanation and the
/// decision record.
async fn reported_state(emulator: &Emulator, alarm: &str) -> Value {
    let described = monitoring(
        emulator,
        "DescribeAlarms",
        &json!({"AlarmNames": [alarm], "AlarmTypes": ["MetricAlarm"]}),
    )
    .await;
    json!({
        "value": described["MetricAlarms"][0]["StateValue"].clone(),
        "reason": described["MetricAlarms"][0]["StateReason"].clone(),
        "reasonData": described["MetricAlarms"][0]["StateReasonData"].clone(),
    })
}

/// Force the alarm into `state`.
async fn force(emulator: &Emulator, alarm: &str, state: &str) {
    monitoring(
        emulator,
        "SetAlarmState",
        &json!({
            "AlarmName": alarm,
            "StateValue": state,
            "StateReason": format!("driven into {state} by the test"),
        }),
    )
    .await;
}

/// The events `held` holds, ordered by the instant each reports its transition at — which is
/// the order the transitions happened in, whatever order the queue handed them over in.
fn in_transition_order(held: &[Value]) -> Vec<Value> {
    let mut ordered: Vec<Value> = held.to_vec();
    ordered.sort_by(|left, right| {
        transition_instant(left)
            .partial_cmp(&transition_instant(right))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ordered
}

/// The instant one event reports its transition at, as seconds.
fn transition_instant(event: &Value) -> f64 {
    event
        .pointer("/detail/state/timestamp")
        .and_then(Value::as_str)
        .and_then(document_instant_secs)
        .unwrap_or_default()
}

/// Assert the chain over the events of one alarm: the first reports the state the alarm was
/// created in, and every later one reports as its previous state exactly what the event before
/// it reported as its state.
fn assert_the_chain_joins_up(case: &str, held: &[Value], drifted: Option<&Value>) {
    let ordered = in_transition_order(held);
    assert!(
        ordered.len() >= 2,
        "case {case:?}: expected at least two transitions to chain, got {ordered:?}"
    );
    let first = ordered[0]
        .pointer("/detail/previousState/value")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        first,
        json!(INSUFFICIENT_DATA),
        "case {case:?}: expected the alarm's first transition to move it out of the state its \
         creation left it in, got {first} in {:?}",
        ordered[0]
    );
    for pair in ordered.windows(2) {
        let announced = pair[0].pointer("/detail/state").cloned().unwrap_or(Value::Null);
        let reported = pair[1]
            .pointer("/detail/previousState")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            reported, announced,
            "case {case:?}: expected the transition {} to report the state the transition before \
             it announced ({announced}) as the state it moved out of, got {reported}{}",
            pair[1],
            drifted.map_or(String::new(), |drifted| format!(
                "; the alarm's own state meanwhile read {drifted}"
            ))
        );
    }
}

/// RULE: the transitions a CALLER forces chain — each event moves the alarm out of the state the
/// event before it moved it into.
#[tokio::test]
async fn forced_transitions_chain() {
    let case = "three states a caller forced in turn";
    let emulator = emulator_with_a_bus();
    let alarm = "chain-forced";
    let queue = given_target_queue(&emulator, "chain-forced-q").await;
    given_alarm(&emulator, alarm).await;
    given_rule_for_alarm(&emulator, case, "chain-forced-rule", alarm, &queue).await;

    let mut held: Vec<Value> = Vec::new();
    // Three transitions in turn: each force moves the alarm out of the state the force before it
    // left, and the row waits for each one's event so two are never read as one.
    for (index, state) in ["ALARM", "OK", "ALARM"].iter().enumerate() {
        force(&emulator, alarm, state).await;
        until_count(&emulator, &queue, index + 1, &mut held).await;
        assert_eq!(
            held.len(),
            index + 1,
            "case {case:?}: expected the force into {state:?} to publish exactly one event, got \
             {held:?}"
        );
    }
    assert_the_chain_joins_up(case, &held, None);
}

/// RULE, where it is hardest: the alarm's own EVALUATION moved it and the event announced the
/// state it reached; a later write then RE-STATED that same state, which moves the alarm nowhere
/// and publishes nothing while leaving the alarm carrying a different explanation of it. The
/// transition that moves the alarm OUT reports the state the event announced, not the one the
/// alarm happens to carry when it moves.
#[tokio::test]
async fn an_evaluated_transition_chains_to_the_state_it_announced() {
    let case = "a state re-stated under the alarm between two transitions";
    let emulator = emulator_with_a_bus();
    let alarm = "chain-evaluated";
    let queue = given_target_queue(&emulator, "chain-evaluated-q").await;
    given_alarm(&emulator, alarm).await;
    given_rule_for_alarm(&emulator, case, "chain-evaluated-rule", alarm, &queue).await;

    let mut held: Vec<Value> = Vec::new();
    until_evaluated(&emulator, &queue, alarm, BREACHING, "ALARM", &mut held).await;
    let announced = held
        .iter()
        .find(|event| event.pointer("/detail/state/value") == Some(&json!("ALARM")))
        .map(|event| event.pointer("/detail/state").cloned().unwrap_or(Value::Null))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an event reporting ALARM on the rule's queue: {held:?}")
        });
    let delivered = held.len();

    // A write that states the state the alarm is ALREADY in moves it nowhere, so it publishes
    // nothing — while the explanation the alarm carries for that state becomes the caller's.
    force(&emulator, alarm, "ALARM").await;
    let reported = reported_state(&emulator, alarm).await;
    assert_eq!(
        reported["value"],
        json!("ALARM"),
        "case {case:?}: the re-stated state must leave the alarm where it was, got {reported}"
    );
    assert_ne!(
        reported["reason"], announced["reason"],
        "case {case:?}: the re-stated state must leave the alarm carrying another explanation \
         than the event announced, got {reported}"
    );

    // Then the alarm moves out of it, and that event is the one the chain is read over.
    force(&emulator, alarm, "OK").await;
    until_count(&emulator, &queue, delivered + 1, &mut held).await;
    assert_eq!(
        held.len(),
        delivered + 1,
        "case {case:?}: expected the re-stated state to publish nothing and the move out of it \
         to publish one event, got {held:?}"
    );
    assert_the_chain_joins_up(case, &held, Some(&reported));
}

/// RULE, over the other DRIVER: two transitions the alarm's own evaluation made, out of
/// datapoints over and then under its threshold, chain to each other.
#[tokio::test]
async fn two_evaluated_transitions_chain() {
    let case = "two transitions the alarm's own evaluation made";
    let emulator = emulator_with_a_bus();
    let alarm = "chain-evaluated-pair";
    let queue = given_target_queue(&emulator, "chain-evaluated-pair-q").await;
    given_alarm(&emulator, alarm).await;
    given_rule_for_alarm(&emulator, case, "chain-evaluated-pair-rule", alarm, &queue).await;

    let mut held: Vec<Value> = Vec::new();
    until_evaluated(&emulator, &queue, alarm, BREACHING, "ALARM", &mut held).await;
    // Datapoints UNDER the threshold move it back: the windows the breaching ones were
    // published into age out of the evaluation's range as the clock advances, so a window
    // holding only these is what the next evaluation grades.
    until_evaluated(&emulator, &queue, alarm, NOT_BREACHING, "OK", &mut held).await;
    assert!(
        holds_state(&held, "OK"),
        "case {case:?}: expected the alarm to be moved back to OK by datapoints under its \
         threshold, got {held:?}"
    );
    assert_the_chain_joins_up(case, &held, None);
}

/// The alarm the three-link evaluated row drives: the same series, but treating a window with no
/// datapoint as MISSING ("missing — the alarm does not consider missing data points when
/// evaluating whether to change state", `PutMetricAlarmInput$TreatMissingData`), so the alarm's
/// own evaluation moves it back to the state its creation left it in once the series stops; and
/// folded by MAXIMUM over one window, so each state is decided by one window and its record names
/// that window.
async fn given_alarm_that_treats_missing_data_as_missing(emulator: &Emulator, name: &str) {
    monitoring(
        emulator,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "AlarmDescription": "the alarm of this row",
            "Namespace": NAMESPACE,
            "MetricName": METRIC,
            "Dimensions": [{"Name": "QueueName", "Value": name}],
            "Statistic": "Maximum",
            "Period": PERIOD,
            "EvaluationPeriods": 1,
            "DatapointsToAlarm": 1,
            "Threshold": THRESHOLD,
            "ComparisonOperator": "GreaterThanThreshold",
            "TreatMissingData": "missing",
            "ActionsEnabled": false,
        }),
    )
    .await;
}

/// Wait until an event reporting `state` has reached the queue WITHOUT publishing anything
/// meanwhile: the evaluation this waits for is the one a window holding no datapoint decides, so
/// the row looks at the alarm as well as at the queue and publishes nothing. The wait is for the
/// CONDITION rather than for a span.
async fn until_unpublished_state(
    emulator: &Emulator,
    url: &str,
    alarm: &str,
    state: &str,
    held: &mut Vec<Value>,
) {
    for _ in 0..looks() {
        reported_state(emulator, alarm).await;
        collect(emulator, url, held).await;
        if holds_state(held, state) {
            return;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    collect(emulator, url, held).await;
}

/// The DECISION RECORD one state document carries (`reasonData`, which is the alarm's
/// `StateReasonData`), or `None` for a state no evaluation decided.
fn decision_record(state: &Value) -> Option<&str> {
    state.pointer("/reasonData").and_then(Value::as_str)
}

/// The instant a decision record reports the evaluation it records was made at (`queryDate`), as
/// seconds.
fn record_query_instant(case: &str, record: &str) -> f64 {
    let parsed: Value = serde_json::from_str(record).unwrap_or_else(|error| {
        panic!("case {case:?}: expected the decision record {record} to be JSON, got {error}")
    });
    parsed
        .get("queryDate")
        .and_then(Value::as_str)
        .and_then(document_instant_secs)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a queryDate in the decision record {record}")
        })
}

/// The state value one event reports moving the alarm TO.
fn announced_value(event: &Value) -> String {
    event
        .pointer("/detail/state/value")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// RULE, over THREE links the alarm's own evaluations made and over the DECISION RECORD each
/// state was announced with: the alarm breaches, recovers, and is then moved back by a window
/// holding no datapoint at all. Every event reports as the state it moved out of the whole state
/// document the event before it announced — the record included — so the record a consumer reads
/// for the state that ended is the record of the evaluation that CREATED that state and not of
/// the one that ended it. The alarm's first transition moves it out of the state its creation
/// left it in, which carries an explanation and no record, because nothing was evaluated for it.
#[tokio::test]
async fn evaluated_transitions_chain_with_the_record_each_state_was_announced_with() {
    let case = "three evaluated transitions, the last one decided by a window with no datapoint";
    let emulator = emulator_with_a_bus();
    let alarm = "chain-missing-data";
    let queue = given_target_queue(&emulator, "chain-missing-data-q").await;
    given_alarm_that_treats_missing_data_as_missing(&emulator, alarm).await;
    given_rule_for_alarm(&emulator, case, "chain-missing-data-rule", alarm, &queue).await;

    // The state the CREATION left the alarm in: the state its first transition moves out of.
    let created = reported_state(&emulator, alarm).await;
    assert_eq!(
        created["value"],
        json!(INSUFFICIENT_DATA),
        "case {case:?}: expected the creation to leave the alarm in {INSUFFICIENT_DATA}, got \
         {created}"
    );
    assert_eq!(
        created["reasonData"],
        Value::Null,
        "case {case:?}: nothing is evaluated at creation, so the state a creation leaves carries \
         no decision record, got {created}"
    );

    let mut held: Vec<Value> = Vec::new();
    // Both published links are driven NEAR THE OLD END of the evaluation range this alarm
    // searches, so the window holding the row's newest datapoint leaves that range a few period
    // boundaries after the publishing stops rather than a whole range span later.
    until_evaluated_at_age(
        &emulator,
        &queue,
        alarm,
        BREACHING,
        AGE_NEAR_THE_END_OF_THE_RANGE,
        "ALARM",
        &mut held,
    )
    .await;
    until_evaluated_at_age(
        &emulator,
        &queue,
        alarm,
        NOT_BREACHING,
        AGE_NEAR_THE_END_OF_THE_RANGE - f64::from(i32::try_from(PERIOD).unwrap_or(i32::MAX)),
        "OK",
        &mut held,
    )
    .await;
    // Publishing STOPS here: what moves this alarm back is a window holding no datapoint at all.
    until_unpublished_state(&emulator, &queue, alarm, INSUFFICIENT_DATA, &mut held).await;

    let ordered = in_transition_order(&held);
    let states: Vec<String> = ordered.iter().map(announced_value).collect();
    assert_eq!(
        states.first().map(String::as_str),
        Some("ALARM"),
        "case {case:?}: expected the breaching datapoints to move the alarm into ALARM first, \
         got the states {states:?} from {held:?}"
    );
    assert_eq!(
        states.last().map(String::as_str),
        Some(INSUFFICIENT_DATA),
        "case {case:?}: expected the stopped series to move the alarm back to \
         {INSUFFICIENT_DATA} last, got the states {states:?} from {held:?}"
    );
    let recovered_at = states.iter().position(|state| state == "OK");
    assert!(
        recovered_at.is_some_and(|at| at > 0 && at + 1 < states.len()),
        "case {case:?}: expected the alarm to be moved out of ALARM into OK and only then back \
         to {INSUFFICIENT_DATA}, got the states {states:?} from {held:?}"
    );

    assert_the_chain_joins_up(case, &held, None);

    // And the tuple the alarm ITSELF serves is the one its last event announced: one stored
    // explanation per state, read the same by a consumer of the bus and by a caller of
    // `DescribeAlarms`.
    let reported = reported_state(&emulator, alarm).await;
    let last_announced = ordered
        .last()
        .and_then(|event| event.pointer("/detail/state").cloned())
        .unwrap_or(Value::Null);
    assert_eq!(
        reported,
        json!({
            "value": last_announced.pointer("/value").cloned().unwrap_or(Value::Null),
            "reason": last_announced.pointer("/reason").cloned().unwrap_or(Value::Null),
            "reasonData": last_announced
                .pointer("/reasonData")
                .cloned()
                .unwrap_or(Value::Null),
        }),
        "case {case:?}: expected the alarm to go on reporting the state its last event announced \
         ({last_announced}), got {reported}"
    );

    // The one state with no record of its own is the state the CREATION left: the first event
    // reports its explanation and no record, rather than a record of an evaluation that had not
    // happened while the alarm was in it.
    let left_at_creation = ordered[0]
        .pointer("/detail/previousState")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        left_at_creation.pointer("/reason").and_then(Value::as_str),
        created["reason"].as_str(),
        "case {case:?}: expected the first transition to move the alarm out of the state its \
         creation left it in ({created}), got {left_at_creation}"
    );
    assert_eq!(
        decision_record(&left_at_creation),
        None,
        "case {case:?}: expected the state a creation left to be reported with no decision \
         record, got {left_at_creation}"
    );

    // Every later link: the record of the state an event moved OUT OF is the record that state
    // was announced with, which is the record of an EARLIER evaluation than the one that moved
    // the alarm on — never the record the emitting evaluation computed for itself.
    for pair in ordered.windows(2) {
        let announced = pair[0]
            .pointer("/detail/state")
            .cloned()
            .unwrap_or(Value::Null);
        let reached = pair[1]
            .pointer("/detail/state")
            .cloned()
            .unwrap_or(Value::Null);
        let left = pair[1]
            .pointer("/detail/previousState")
            .cloned()
            .unwrap_or(Value::Null);
        let announced_record = decision_record(&announced).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected an evaluated state to be announced with a decision \
                 record, got {announced}"
            )
        });
        let reached_record = decision_record(&reached).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected an evaluated state to be announced with a decision \
                 record, got {reached}"
            )
        });
        let left_record = decision_record(&left).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the state an event moved out of to be reported with the \
                 record it was announced with ({announced_record}), got {left}"
            )
        });
        assert_eq!(
            left_record, announced_record,
            "case {case:?}: expected the transition {} to report the record the state it moved \
             out of was announced with, got {left_record}",
            pair[1]
        );
        assert_ne!(
            left_record, reached_record,
            "case {case:?}: expected the record of the state the transition {} left to be \
             another evaluation's than the record of the state it reached, got {left_record} for \
             both",
            pair[1]
        );
        let left_at = record_query_instant(case, left_record);
        let reached_at = record_query_instant(case, reached_record);
        assert!(
            left_at < reached_at,
            "case {case:?}: expected the record of the state the transition {} left to be the \
             one computed BEFORE the record of the state it reached, got {left_at} against \
             {reached_at}",
            pair[1]
        );
    }
}

/// RULE, over the two drivers MIXED: a state a caller forces out of a state the evaluation
/// reached chains to what that evaluation announced.
#[tokio::test]
async fn a_forced_transition_chains_to_an_evaluated_one() {
    let case = "a state a caller forced out of an evaluated one";
    let emulator = emulator_with_a_bus();
    let alarm = "chain-mixed";
    let queue = given_target_queue(&emulator, "chain-mixed-q").await;
    given_alarm(&emulator, alarm).await;
    given_rule_for_alarm(&emulator, case, "chain-mixed-rule", alarm, &queue).await;

    let mut held: Vec<Value> = Vec::new();
    until_evaluated(&emulator, &queue, alarm, BREACHING, "ALARM", &mut held).await;
    force(&emulator, alarm, "OK").await;
    until_state(&emulator, &queue, "OK", &mut held).await;
    assert_the_chain_joins_up(case, &held, None);
}
