// SPDX-License-Identifier: CC0-1.0

//! Two rules of ONE bus, matched by ONE event, sharing ONE dead-letter queue: what
//! the queue holds, what each parked copy says about WHOSE handover failed, and
//! which rule's `AWS/Events` counters moved for it.
//!
//! The RULE pinned here: every rule a published event matched is handed over to,
//! accounted for and parked on its OWN — so a handover the target's own
//! resource-based policy refuses ("for Lambda functions, Amazon SNS topics and
//! Amazon SQS queues, EventBridge relies on resource-based policies", `PutTargets`)
//! writes exactly ONE copy of the carried envelope to the queue that target's
//! `DeadLetterConfig` names ("the ARN of the SQS queue specified as the target for
//! the dead-letter queue", `DeadLetterConfig$Arn`), under the ARN of the rule whose
//! handover failed, the ARN of the target that refused it and the CONDITION of the
//! refusal — the class the CODE the answering service used belongs to, which is
//! `NO_PERMISSIONS` where that service answered an authorization code and
//! `NO_RESOURCE` where it answered its not-found one, as Amazon SQS does for a queue
//! it will not disclose to the sender — and counts one datapoint on THAT
//! rule's `InvocationsSentToDlq`; while a
//! sibling rule whose target accepted the same event counts
//! `SuccessfulInvocationAttempts` and carries no sent-to-DLQ datapoint at all —
//! even when both rules name the SAME dead-letter queue, because the queue an
//! event was parked on is not what attributes it.
//!
//! Sharing one queue between a failing and a healthy rule is the case where the two
//! halves of that rule are observable at once: the copy count tells a client that
//! the healthy rule parked nothing, and the three attribution attributes are the
//! only thing that says WHICH rule and WHICH target failed, and that the cause was
//! the missing grant rather than a throttle or a timeout. The parked copy is the
//! envelope that was carried, not a fresh one, so it agrees with the copy the
//! healthy twin received on every envelope key.
//!
//! The rows differ along what the rule generalises over: the KIND of the refused
//! target (a Lambda function and an Amazon SQS queue, two of the kinds a target's
//! own policy governs), the FORM in which that policy withholds the invocation (no
//! document at all, and a grant scoped by `aws:SourceArn` to another rule of the
//! bus), the BUS the two rules live on (a custom bus and the default one) and HOW
//! MANY matching events the round publishes, so a count is read as a count rather
//! than as a flag.
//!
//! `tests/all/dead_letter_accounting.rs` pins the same attribution where each rule
//! has its OWN dead-letter queue; what this suite adds is the shared one, the
//! envelope-key identity between the parked copy and the delivered one, and the
//! positive control beside the withheld counter.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    DEFAULT_EVENT_BUS_NAME, EVENT_BUS_NAME_DIMENSION, INVOCATIONS_SENT_TO_DLQ_METRIC,
    METRIC_NAMESPACE, NO_PERMISSIONS, NO_RESOURCE, RULE_NAME_DIMENSION, SERVICE_PRINCIPAL,
    SUCCESSFUL_INVOCATION_ATTEMPTS_METRIC,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::STATE_ACTIVE;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The `source` and `detail-type` every event this suite publishes carries, and
/// what both rules of a row match on.
const SOURCE: &str = "com.demo.dlq-attribution";
const DETAIL_TYPE: &str = "Dlq Attribution Probe";

/// The IAM action an invocation of a function performs, which is what a grant for
/// this service has to name.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The id of the one statement a row's grant is written under.
const STATEMENT_ID: &str = "events-invoke";

/// A deployment package as `Code.ZipFile` carries it. What it holds does not matter
/// here: only that Lambda accepts the function, and that the handover is decided
/// against the document the function carries.
const ZIP_FILE: &str = "UEsDBAoAAAAAAAAAAAA=";

/// The id of the one target each rule of a row carries.
const TARGET_ID: &str = "t";

/// The members of an event envelope the parked copy and the delivered copy must
/// agree on, because both are the same fanned-out entry. Source: the EventBridge
/// event-structure reference, and the `PutEvents` entry members an event is built
/// from.
const ENVELOPE_KEYS: [&str; 8] = [
    "id",
    "version",
    "time",
    "region",
    "account",
    "source",
    "detail-type",
    "detail",
];

/// How long a wait for a side effect looks before giving up on it, and how often it
/// looks. The budget is generous: it bounds a condition that should already hold,
/// never the speed of the host.
const WAIT: Duration = Duration::from_secs(60);
const LOOK_INTERVAL: Duration = Duration::from_millis(25);

/// How many further looks the absence hold takes after the copies that were owed
/// arrived: a message that should not be there is given room to show up.
const ABSENCE_LOOKS: u32 = 40;

/// The window a metric read spans, and the granularity it folds the series into:
/// one standard-resolution minute, the resolution these counters are published at.
const WINDOW_SECONDS: f64 = 300.0;
const PERIOD_SECONDS: i64 = 60;

/// EventBridge, Lambda, Amazon SQS and CloudWatch over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the function a rule targets, the queues it
/// delivers to and parks on, and the counters it publishes all live beside this
/// service.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
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

/// A SigV4 `Authorization` whose credential scope names `service`, which is what
/// routes the request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// The body of a response, parsed as JSON where there is one.
fn parsed(payload: &[u8]) -> Value {
    if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(payload).unwrap_or(Value::Null)
    }
}

/// One `X-Amz-Target` RPC call, as its status and parsed body.
async fn rpc(
    emulator: &Emulator,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, parsed(&response.into_body()))
}

/// An EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn events(emulator: &Emulator, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answered) = rpc(emulator, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with \
         {answered}"
    );
    answered
}

/// An Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200.
async fn sqs(emulator: &Emulator, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answered) = rpc(emulator, "sqs", "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {answered}"
    );
    answered
}

/// A CloudWatch call, which is where this service's own counters are read back from.
async fn cloudwatch(emulator: &Emulator, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answered) = rpc(
        emulator,
        "monitoring",
        "GraniteServiceVersion20100801",
        "1.0",
        operation,
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudWatch {operation}, got {status} with \
         {answered}"
    );
    answered
}

/// A Lambda `restJson1` call, as its status and parsed body.
async fn lambda(emulator: &Emulator, method: &str, uri: &str, body: Value) -> (u16, Value) {
    let payload = if body.is_null() {
        bytes::Bytes::new()
    } else {
        bytes::Bytes::from(body.to_string())
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(payload)
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, parsed(&response.into_body()))
}

/// The queue policy this service's `sqs:SendMessage` needs — on a target queue for
/// the delivery, and on a dead-letter queue for the parked copy, which is the same
/// send by the same principal.
fn granting_events(queue_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowEventBridge",
            "Effect": "Allow",
            "Principal": { "Service": SERVICE_PRINCIPAL },
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
        }],
    })
    .to_string()
}

/// A queue called `name`, carrying the grant this service's send needs when
/// `granting`; answers its URL and the ARN a target names it by.
async fn given_queue(
    emulator: &Emulator,
    case: &str,
    name: &str,
    granting: bool,
) -> (String, String) {
    let created = sqs(emulator, case, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
        .to_string();
    let attributes = sqs(
        emulator,
        case,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueArn, got {attributes}"))
        .to_string();
    if granting {
        sqs(
            emulator,
            case,
            "SetQueueAttributes",
            &json!({ "QueueUrl": url, "Attributes": { "Policy": granting_events(&arn) } }),
        )
        .await;
    }
    (url, arn)
}

/// The account every emulated resource of this suite belongs to, as the engine
/// holds it.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// Create the function a row's failing rule targets, and answer the ARN Lambda
/// assigned it.
async fn create_function(emulator: &Emulator, case: &str, name: &str) -> String {
    let (status, created) = lambda(
        emulator,
        "POST",
        "/2015-03-31/functions",
        json!({
            "FunctionName": name,
            "Role": format!("arn:aws:iam::{}:role/service-role/lambda-exec", account()),
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Code": { "ZipFile": ZIP_FILE },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from Lambda CreateFunction, got {status} ({created})"
    );
    created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a FunctionArn, got {created}"))
        .to_string()
}

/// Look until the created function reports `Active`, which is the state a handover
/// is decided by the POLICY in rather than by the lifecycle ("during this time, you
/// can't invoke or modify the function", `CreateFunction`).
async fn poll_until_active(emulator: &Emulator, case: &str, name: &str) {
    let looks = WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    let mut last = Value::Null;
    for _ in 0..=looks {
        let (status, described) = lambda(
            emulator,
            "GET",
            &format!("/2015-03-31/functions/{name}"),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from Lambda GetFunction, got {status} ({described})"
        );
        if described["Configuration"]["State"] == json!(STATE_ACTIVE) {
            return;
        }
        last = described;
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!("case {case:?}: the function never reported {STATE_ACTIVE}, last read {last}");
}

/// Grant this service the invocation on `function`, optionally scoped by
/// `aws:SourceArn` to one rule — which is the form a grant that admits another
/// rule's handover and not this one takes.
async fn add_permission(emulator: &Emulator, case: &str, function: &str, source_arn: Option<&str>) {
    let mut body = json!({
        "StatementId": STATEMENT_ID,
        "Action": INVOKE_ACTION,
        "Principal": SERVICE_PRINCIPAL,
    });
    if let Some(source_arn) = source_arn {
        body["SourceArn"] = json!(source_arn);
    }
    let (status, answered) = lambda(
        emulator,
        "POST",
        &format!("/2015-03-31/functions/{function}/policy"),
        body,
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from Lambda AddPermission, got {status} ({answered})"
    );
}

/// The bus a row's two rules live on, created when it is not the default one.
async fn given_bus(emulator: &Emulator, case: &str, bus: &str) {
    if bus == DEFAULT_EVENT_BUS_NAME {
        return;
    }
    events(emulator, case, "CreateEventBus", &json!({ "Name": bus })).await;
}

/// A rule called `name` on `bus` matching every event this suite publishes, with
/// `target` as its one target; answers the ARN EventBridge assigned the rule.
async fn given_rule(
    emulator: &Emulator,
    case: &str,
    bus: &str,
    name: &str,
    target: &Value,
) -> String {
    let pattern = json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string();
    let rule = events(
        emulator,
        case,
        "PutRule",
        &json!({
            "Name": name,
            "EventBusName": bus,
            "EventPattern": pattern,
            "State": "ENABLED",
        }),
    )
    .await;
    let arn = rule["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a RuleArn, got {rule}"))
        .to_string();
    let wired = events(
        emulator,
        case,
        "PutTargets",
        &json!({ "Rule": name, "EventBusName": bus, "Targets": [target] }),
    )
    .await;
    assert_eq!(
        wired["FailedEntryCount"],
        json!(0),
        "case {case:?}: a target is authorized at delivery time, so PutTargets accepts it: \
         {wired}"
    );
    arn
}

/// One target of a rule: the resource it names, no retry at all — "a permissions
/// failure is not retried" — and the dead-letter queue the two rules of a row
/// SHARE.
fn target(arn: &str, dead_letter_arn: &str) -> Value {
    json!({
        "Id": TARGET_ID,
        "Arn": arn,
        "RetryPolicy": { "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 },
        "DeadLetterConfig": { "Arn": dead_letter_arn },
    })
}

/// Publish one matching event onto `bus` carrying `nonce` in its detail, and answer
/// the `EventId` EventBridge assigned it.
async fn publish(emulator: &Emulator, case: &str, bus: &str, nonce: &str) -> String {
    let published = events(
        emulator,
        case,
        "PutEvents",
        &json!({
            "Entries": [{
                "EventBusName": bus,
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": json!({ "nonce": nonce }).to_string(),
            }],
        }),
    )
    .await;
    assert_eq!(
        published["FailedEntryCount"],
        json!(0),
        "case {case:?}: expected the entry to be accepted, got {published}"
    );
    published["Entries"][0]["EventId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an EventId, got {published}"))
        .to_string()
}

/// Every message waiting on the queue at `url`, taken OFF it with its attributes.
async fn drain(emulator: &Emulator, case: &str, url: &str) -> Vec<Value> {
    let answered = sqs(
        emulator,
        case,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "MessageAttributeNames": ["All"],
        }),
    )
    .await;
    let held = answered["Messages"].as_array().cloned().unwrap_or_default();
    for message in &held {
        let receipt = message["ReceiptHandle"].as_str().unwrap_or_default();
        sqs(
            emulator,
            case,
            "DeleteMessage",
            &json!({ "QueueUrl": url, "ReceiptHandle": receipt }),
        )
        .await;
    }
    held
}

/// Look until `wanted` messages have been taken off the queue at `url`, and answer
/// them.
async fn collected(
    emulator: &Emulator,
    case: &str,
    url: &str,
    wanted: usize,
    what: &str,
) -> Vec<Value> {
    let looks = WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    let mut taken: Vec<Value> = Vec::new();
    for _ in 0..=looks {
        taken.extend(drain(emulator, case, url).await);
        if taken.len() >= wanted {
            return taken;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!("case {case:?}: expected {wanted} {what} message(s), got {taken:?}");
}

/// Keep looking at the queue at `url` for a copy that must not be there, and fail on
/// the first one that arrives: the absence half of "exactly one copy".
async fn no_further_message(emulator: &Emulator, case: &str, url: &str, what: &str) {
    for _ in 0..ABSENCE_LOOKS {
        let extra = drain(emulator, case, url).await;
        assert!(
            extra.is_empty(),
            "case {case:?}: expected no further {what} message, got {extra:?}"
        );
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// The total depth of the queue at `url`: what is waiting, what is in flight and
/// what is delayed, as `GetQueueAttributes` reports them.
async fn queue_depth(emulator: &Emulator, case: &str, url: &str) -> i64 {
    let answered = sqs(
        emulator,
        case,
        "GetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "AttributeNames": [
                "ApproximateNumberOfMessages",
                "ApproximateNumberOfMessagesNotVisible",
                "ApproximateNumberOfMessagesDelayed",
            ],
        }),
    )
    .await;
    [
        "ApproximateNumberOfMessages",
        "ApproximateNumberOfMessagesNotVisible",
        "ApproximateNumberOfMessagesDelayed",
    ]
    .iter()
    .map(|name| {
        answered["Attributes"][*name]
            .as_str()
            .and_then(|count| count.parse::<i64>().ok())
            .unwrap_or_default()
    })
    .sum()
}

/// The instant this read is taken, as CloudWatch's own timestamps are.
fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// The `Sum` datapoints CloudWatch serves for `metric` of the rule `rule` on `bus`,
/// in EventBridge's own namespace, over a window around now.
async fn datapoints(
    emulator: &Emulator,
    case: &str,
    metric: &str,
    bus: &str,
    rule: &str,
) -> Vec<f64> {
    let now = epoch_seconds();
    let answered = cloudwatch(
        emulator,
        case,
        "GetMetricStatistics",
        &json!({
            "Namespace": METRIC_NAMESPACE,
            "MetricName": metric,
            "Dimensions": [
                { "Name": EVENT_BUS_NAME_DIMENSION, "Value": bus },
                { "Name": RULE_NAME_DIMENSION, "Value": rule },
            ],
            "StartTime": now - WINDOW_SECONDS,
            "EndTime": now + WINDOW_SECONDS,
            "Period": PERIOD_SECONDS,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    answered["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|point| point["Sum"].as_f64())
        .collect()
}

/// The envelope members of one message body, keyed by [`ENVELOPE_KEYS`].
fn envelope_keys(case: &str, message: &Value) -> Value {
    let body = message["Body"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a message Body, got {message}"));
    let parsed: Value = serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("case {case:?}: the body is not JSON ({error}): {body}"));
    let mut keyed = serde_json::Map::new();
    for key in ENVELOPE_KEYS {
        keyed.insert(key.to_string(), parsed[key].clone());
    }
    Value::Object(keyed)
}

/// The value of one message attribute, as a string.
fn attribute(message: &Value, name: &str) -> Option<String> {
    message["MessageAttributes"][name]["StringValue"]
        .as_str()
        .map(ToString::to_string)
}

/// WHAT the failing rule's target is, and HOW its own document withholds the
/// invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refused {
    /// A Lambda function carrying no resource policy at all, so it has granted this
    /// service nothing: the recorded break.
    FunctionWithNoPolicy,
    /// A Lambda function whose grant is scoped by `aws:SourceArn` to the SIBLING
    /// rule, so the document exists and admits another rule's handover, not this
    /// one's.
    FunctionGrantedToAnotherRule,
    /// An Amazon SQS queue carrying no policy, the other kind a target's own
    /// document governs.
    QueueWithNoPolicy,
}

/// One row: what refuses the failing rule's handover, which bus the two rules live
/// on, how many matching events the round publishes, and the CONDITION the parked
/// copy carries — the class the CODE the answering service used belongs to, which is
/// `NO_PERMISSIONS` where that service answered an authorization refusal and
/// `NO_RESOURCE` where it answered its own not-found code, as Amazon SQS does for a
/// queue it will not disclose to the sender.
struct Row {
    case: &'static str,
    refused: Refused,
    bus: &'static str,
    events: usize,
    condition: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a Lambda target with no invoke grant, a custom bus, one event",
            refused: Refused::FunctionWithNoPolicy,
            bus: "dlq-attribution-bus",
            events: 1,
            condition: NO_PERMISSIONS,
        },
        Row {
            case: "a Lambda target granted to the sibling rule only, a custom bus, two events",
            refused: Refused::FunctionGrantedToAnotherRule,
            bus: "orders.bus-2",
            events: 2,
            condition: NO_PERMISSIONS,
        },
        Row {
            case: "a queue target that has granted nothing, the default bus, three events",
            refused: Refused::QueueWithNoPolicy,
            bus: DEFAULT_EVENT_BUS_NAME,
            events: 3,
            condition: NO_RESOURCE,
        },
    ]
}

/// The two rules of every row.
const FAILING_RULE: &str = "failing";
const HEALTHY_RULE: &str = "healthy";

/// RULE: a handover the target's own resource-based policy refuses parks exactly one
/// copy of the carried envelope on the queue that target's `DeadLetterConfig` names,
/// attributed to the rule and target that failed and to the permission that was
/// missing, and counts one datapoint on that rule's `InvocationsSentToDlq` — while
/// the sibling rule the same event matched, whose target took it, counts
/// `SuccessfulInvocationAttempts` and has no datapoint on that metric at all, even
/// though both rules park on the SAME queue.
#[tokio::test]
async fn a_shared_dead_letter_queue_holds_only_the_failing_rules_copies() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        given_bus(&emulator, case, row.bus).await;

        // The one queue BOTH rules dead-letter to, and the queue the healthy rule's
        // target is: each granting this service the send it needs.
        let (shared_dlq_url, shared_dlq_arn) =
            given_queue(&emulator, case, &format!("shared-dlq-{index}"), true).await;
        let (healthy_url, healthy_arn) =
            given_queue(&emulator, case, &format!("healthy-target-{index}"), true).await;

        // The target whose invocation is refused, as this row withholds it.
        let function_name = format!("refused-fn-{index}");
        let failing_arn = match row.refused {
            Refused::FunctionWithNoPolicy | Refused::FunctionGrantedToAnotherRule => {
                let arn = create_function(&emulator, case, &function_name).await;
                poll_until_active(&emulator, case, &function_name).await;
                arn
            }
            Refused::QueueWithNoPolicy => {
                given_queue(&emulator, case, &format!("refused-target-{index}"), false)
                    .await
                    .1
            }
        };

        let healthy_rule_arn = given_rule(
            &emulator,
            case,
            row.bus,
            HEALTHY_RULE,
            &target(&healthy_arn, &shared_dlq_arn),
        )
        .await;
        let failing_rule_arn = given_rule(
            &emulator,
            case,
            row.bus,
            FAILING_RULE,
            &target(&failing_arn, &shared_dlq_arn),
        )
        .await;
        assert_ne!(
            failing_rule_arn, healthy_rule_arn,
            "case {case:?}: the two rules of a row are two rules"
        );
        if row.refused == Refused::FunctionGrantedToAnotherRule {
            // A document that admits the SIBLING rule's handover and no other, so
            // what refuses this one is the scope of the grant rather than its
            // absence.
            add_permission(&emulator, case, &function_name, Some(&healthy_rule_arn)).await;
        }

        let mut nonces = Vec::new();
        for round in 0..row.events {
            let nonce = format!("n-{index}-{round}");
            publish(&emulator, case, row.bus, &nonce).await;
            nonces.push(nonce);
        }

        // The healthy twin: one message per published event, each carrying its own
        // nonce and the service-generated envelope `id`.
        let delivered = collected(&emulator, case, &healthy_url, row.events, "delivered").await;
        assert_eq!(
            delivered.len(),
            row.events,
            "case {case:?}: expected {} delivered message(s), got {delivered:?}",
            row.events
        );
        for message in &delivered {
            let keys = envelope_keys(case, message);
            assert!(
                keys["id"].as_str().is_some_and(|id| !id.is_empty()),
                "case {case:?}: expected a service-generated envelope id, got {keys}"
            );
        }

        // The SHARED dead-letter queue: one copy per refused handover and not one
        // more — a second copy per event would mean the healthy rule dead-lettered
        // a delivery its target took.
        let parked = collected(&emulator, case, &shared_dlq_url, row.events, "parked").await;
        assert_eq!(
            parked.len(),
            row.events,
            "case {case:?}: expected exactly {} parked message(s) on the shared queue, got \
             {parked:?}",
            row.events
        );
        no_further_message(&emulator, case, &shared_dlq_url, "parked").await;
        let depth = queue_depth(&emulator, case, &shared_dlq_url).await;
        assert_eq!(
            depth, 0,
            "case {case:?}: expected the drained shared queue to report a total depth of 0, got \
             {depth}"
        );

        // The attribution every parked copy carries: whose handover failed, which
        // target refused it, and that the cause was the missing permission.
        for message in &parked {
            assert_eq!(
                attribute(message, "RULE_ARN").as_deref(),
                Some(failing_rule_arn.as_str()),
                "case {case:?}: expected the parked copy to name the failing rule \
                 {failing_rule_arn:?}, got {message}"
            );
            assert_eq!(
                attribute(message, "TARGET_ARN").as_deref(),
                Some(failing_arn.as_str()),
                "case {case:?}: expected the parked copy to name the refused target \
                 {failing_arn:?}, got {message}"
            );
            assert_eq!(
                attribute(message, "ERROR_CODE").as_deref(),
                Some(row.condition),
                "case {case:?}: expected the parked copy's ERROR_CODE {:?}, got {message}",
                row.condition
            );
        }

        // And the copies themselves: the parked one is the envelope that was
        // carried, so it agrees with the one the healthy twin received on every
        // envelope key, nonce included.
        for nonce in &nonces {
            let of_nonce = |message: &Value| {
                envelope_keys(case, message)["detail"]["nonce"] == json!(nonce.as_str())
            };
            let delivered_copy = delivered
                .iter()
                .find(|message| of_nonce(message))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no delivered copy of {nonce:?} in {delivered:?}")
                });
            let parked_copy = parked
                .iter()
                .find(|message| of_nonce(message))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no parked copy of {nonce:?} in {parked:?}")
                });
            assert_eq!(
                envelope_keys(case, parked_copy),
                envelope_keys(case, delivered_copy),
                "case {case:?}: expected the parked copy of {nonce:?} to carry the same envelope \
                 as the delivered one"
            );
        }

        // The accounting: the parked events are counted on the FAILING rule's
        // sent-to-DLQ series and the deliveries on the HEALTHY rule's attempt
        // series, and neither counter moves for the other rule — the positive
        // control beside each withheld one, so "no datapoint" is a withheld count
        // rather than an empty store.
        let want = f64::from(u32::try_from(row.events).unwrap_or(u32::MAX));
        let counted = [
            (INVOCATIONS_SENT_TO_DLQ_METRIC, FAILING_RULE, want),
            (INVOCATIONS_SENT_TO_DLQ_METRIC, HEALTHY_RULE, 0.0),
            (SUCCESSFUL_INVOCATION_ATTEMPTS_METRIC, HEALTHY_RULE, want),
            (SUCCESSFUL_INVOCATION_ATTEMPTS_METRIC, FAILING_RULE, 0.0),
        ];
        for (metric, rule, expected) in counted {
            let points = datapoints(&emulator, case, metric, row.bus, rule).await;
            let got: f64 = points.iter().sum();
            assert_eq!(
                got, expected,
                "case {case:?}: expected {metric} for rule {rule} on bus {} to sum to \
                 {expected}, got {points:?}",
                row.bus
            );
            if expected == 0.0 {
                // A counter a rule never moved holds no series at all, which is what
                // a client reads as "this rule dead-lettered nothing".
                assert!(
                    points.is_empty(),
                    "case {case:?}: expected no {metric} datapoint for rule {rule}, got \
                     {points:?}"
                );
            }
        }
    }
}

/// RULE: the grant a client adds repairs the very handover its absence refused — the
/// next matching event reaches the target, nothing further is parked, and the
/// already-parked copy is not replayed.
#[tokio::test]
async fn the_added_grant_repairs_the_handover_and_parks_nothing_further() {
    let case = "the invoke grant is added after a refused handover";
    let emulator = emulator();
    let bus = "dlq-repair-bus";
    given_bus(&emulator, case, bus).await;

    let (shared_dlq_url, shared_dlq_arn) = given_queue(&emulator, case, "repair-dlq", true).await;
    let (healthy_url, healthy_arn) = given_queue(&emulator, case, "repair-target", true).await;
    let function_name = "repair-fn".to_string();
    let function_arn = create_function(&emulator, case, &function_name).await;
    poll_until_active(&emulator, case, &function_name).await;

    given_rule(
        &emulator,
        case,
        bus,
        HEALTHY_RULE,
        &target(&healthy_arn, &shared_dlq_arn),
    )
    .await;
    let failing_rule_arn = given_rule(
        &emulator,
        case,
        bus,
        FAILING_RULE,
        &target(&function_arn, &shared_dlq_arn),
    )
    .await;

    // Before the grant: the handover is refused and its event is parked once.
    let refused_nonce = "before-the-grant";
    publish(&emulator, case, bus, refused_nonce).await;
    let parked = collected(&emulator, case, &shared_dlq_url, 1, "parked").await;
    assert_eq!(
        parked.len(),
        1,
        "case {case:?}: expected one parked copy before the grant, got {parked:?}"
    );
    assert_eq!(
        attribute(&parked[0], "RULE_ARN").as_deref(),
        Some(failing_rule_arn.as_str()),
        "case {case:?}: expected the parked copy to name the failing rule, got {:?}",
        parked[0]
    );
    let delivered = collected(&emulator, case, &healthy_url, 1, "delivered").await;
    assert_eq!(
        delivered.len(),
        1,
        "case {case:?}: expected one delivered copy before the grant, got {delivered:?}"
    );

    // After it: the identical publication is accepted by the function, so nothing
    // further reaches the shared queue — and the copy parked before the grant is not
    // replayed, because the grant governs the events that FOLLOW it.
    add_permission(&emulator, case, &function_name, Some(&failing_rule_arn)).await;
    let repaired_nonce = "after-the-grant";
    publish(&emulator, case, bus, repaired_nonce).await;
    let delivered_after = collected(&emulator, case, &healthy_url, 1, "delivered").await;
    let nonces: Vec<Value> = delivered_after
        .iter()
        .map(|message| envelope_keys(case, message)["detail"]["nonce"].clone())
        .collect();
    assert_eq!(
        nonces,
        vec![json!(repaired_nonce)],
        "case {case:?}: expected the healthy twin to receive the newly published nonce alone, \
         got {nonces:?}"
    );
    no_further_message(&emulator, case, &shared_dlq_url, "parked").await;

    // And the counters: one parked event for the failing rule over the whole run —
    // the refused handover and no other — beside the two attempts its sibling's
    // target took.
    let points = datapoints(
        &emulator,
        case,
        INVOCATIONS_SENT_TO_DLQ_METRIC,
        bus,
        FAILING_RULE,
    )
    .await;
    let parked_total: f64 = points.iter().sum();
    assert_eq!(
        parked_total, 1.0,
        "case {case:?}: expected the failing rule to have parked exactly one event over the run, \
         got {points:?}"
    );
    let attempts = datapoints(
        &emulator,
        case,
        SUCCESSFUL_INVOCATION_ATTEMPTS_METRIC,
        bus,
        HEALTHY_RULE,
    )
    .await;
    let attempts_total: f64 = attempts.iter().sum();
    assert_eq!(
        attempts_total, 2.0,
        "case {case:?}: expected the healthy rule to have taken both events, got {attempts:?}"
    );
    let repaired = datapoints(
        &emulator,
        case,
        SUCCESSFUL_INVOCATION_ATTEMPTS_METRIC,
        bus,
        FAILING_RULE,
    )
    .await;
    let repaired_total: f64 = repaired.iter().sum();
    assert_eq!(
        repaired_total, 1.0,
        "case {case:?}: expected the granted handover to be counted as the one attempt the \
         function took, got {repaired:?}"
    );
}
