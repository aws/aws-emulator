// SPDX-License-Identifier: CC0-1.0

//! An alarm-configuration-change event that an Amazon SQS target will not take is dead-lettered
//! under the class of WHY the delivery failed, with the answer THAT SERVICE gave beside it — end
//! to end over the real
//! `MultiServiceRouter` with CloudWatch, EventBridge, Amazon SQS and KMS on one engine, so the
//! event is produced by the write a client makes and the outcome is read where a client reads it:
//! off the message attributes of the dead letter on the target's `DeadLetterConfig` queue.
//!
//! The RULE asserted here: EventBridge assumes no role for a queue target — "for Lambda
//! functions, Amazon SNS topics and Amazon SQS queues, EventBridge relies on resource-based
//! policies" (`PutTargets`) — so the documents in force over the destination decide the send, and a
//! send they withheld is the PERMISSION class whatever code that destination's service words its
//! refusal under, with that whole answer beside it in `ERROR_MESSAGE`. Amazon SQS
//! models no authorization failure on `SendMessage` at all — its errors are `InvalidAddress`,
//! `InvalidMessageContents`, `InvalidSecurity`, the `Kms*` family, `QueueDoesNotExist`,
//! `RequestThrottled` and `UnsupportedOperation` (`smithy_query(query="operation",
//! operation="SendMessage",
//! model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`)
//! — because it does not disclose a queue it will not let the caller write to: every send the
//! queue's own document withheld, however it withheld it, is answered `QueueDoesNotExist`, HTTP
//! 400 under the `awsQueryError` code `AWS.SimpleQueueService.NonExistentQueue`
//! (`smithy_query(query="shape", shape="QueueDoesNotExist", model_path="...")`). The one refusal
//! of a send that service answers as an
//! authorization failure is the key an encrypted queue's messages are encrypted under, which it
//! relays under `KMS.AccessDeniedException` — a second document withholding the same send, and so
//! the same class.
//!
//! The producer is CloudWatch's own emission rather than a client's `PutEvents`: a write that
//! replaces an alarm's configuration publishes one `aws.cloudwatch` / `CloudWatch Alarm
//! Configuration Change` event onto the account's default bus, which is how alarm governance is
//! noticed — so this suite pins the classification on the path a service's native emission takes,
//! beside `service-com-amazonaws-eventbridge-awsevents`'s own suites, which pin it on the
//! published-event path.
//!
//! The rows differ along what the rule generalises over — the FORM in which the destination
//! withholds the send (no document at all, an explicit `Deny` scoped by `aws:SourceArn` to the
//! delivering rule, a grant to another service principal, a grant scoped to another rule, a grant
//! of another action) and WHICH service answers the refusal (the queue's own document, or the key
//! the queue is encrypted under) — and each row states both the class it expects and the code the
//! answer inside its own `ERROR_MESSAGE` must carry, so a row whose class and answer disagreed
//! would fail whichever of the two was wrong.
//!
//! Every row also pins the two facts around that one: the SAME event reaches the healthy sibling
//! rule's queue (so a refusal is per target, and the event was really produced), and the
//! destination's document or key repaired afterwards makes the NEXT configuration change arrive
//! with nothing else touched.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for and every resource lives in.
const REGION: &str = "us-west-2";

/// The envelope a change to an alarm's configuration is published under, spelled out here rather
/// than imported, so a wrong constant in the implementation cannot make this suite pass.
const SOURCE: &str = "aws.cloudwatch";
const DETAIL_TYPE: &str = "CloudWatch Alarm Configuration Change";

/// The `detail.operation` of the write that created the alarm and of the writes that replaced its
/// configuration afterwards.
const OPERATION_CREATE: &str = "create";
const OPERATION_UPDATE: &str = "update";

/// The service principal EventBridge hands a resource-policy-governed target over as.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// The action a delivery to a queue target performs on that queue, and one it does not.
const SEND_MESSAGE: &str = "sqs:SendMessage";
const OTHER_QUEUE_ACTION: &str = "sqs:ReceiveMessage";

/// The dead-letter message attributes EventBridge adds to an undeliverable event.
const RULE_ARN_ATTRIBUTE: &str = "RULE_ARN";
const TARGET_ARN_ATTRIBUTE: &str = "TARGET_ARN";
const ERROR_CODE_ATTRIBUTE: &str = "ERROR_CODE";
const ERROR_MESSAGE_ATTRIBUTE: &str = "ERROR_MESSAGE";

/// The classes EventBridge reports for a refused delivery — stated here in the test rather than
/// read from any implementation. Which one a delivery the target's service ANSWERED lands in is
/// the class of that answer's own error code: the permission one for an authorization code, the
/// resource one for a not-found code.
const NO_PERMISSIONS: &str = "NO_PERMISSIONS";
const NO_RESOURCE: &str = "NO_RESOURCE";

/// The name Amazon SQS's own error metadata carries, and the answer it gives a sender no document
/// in force over the queue permitted: its not-found class, under fixed text in its entirety —
/// nothing of the request, of the queue or of the account appears in it.
const SQS_SERVICE_NAME: &str = "AmazonSQS";
const QUEUE_NOT_DISCLOSED_CODE: &str = "AWS.SimpleQueueService.NonExistentQueue";
const QUEUE_NOT_DISCLOSED_STATUS: &str = "400";
const QUEUE_NOT_DISCLOSED_SENTENCE: &str =
    "The specified queue does not exist or you do not have access to it.";

/// The code that service relays when the key an ENCRYPTED queue's messages are encrypted under
/// withholds the data key — the one refusal of a send it really does answer as an authorization
/// failure (`smithy_query(query="shape", shape="KmsAccessDenied",
/// model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`),
/// and the action it is about.
const KEY_WITHHELD_CODE: &str = "KMS.AccessDeniedException";
const DATA_KEY_ACTION: &str = "kms:GenerateDataKey";

/// WHICH codes a queue target's service answers a withheld send under, stated here in the test:
/// the two answers above — the not-found code it gives a sender it has not authorized, because it
/// does not disclose such a queue, and the authorization code it relays for the key an encrypted
/// queue is encrypted under. The class beside the answer is the one THAT code belongs to, so the
/// first is the resource class and the second the permission one; a code outside this list is a
/// refusal no service of a queue delivery answers.
const WITHHELD_SEND_CODES: [&str; 3] = [
    QUEUE_NOT_DISCLOSED_CODE,
    "QueueDoesNotExist",
    KEY_WITHHELD_CODE,
];

/// What opens and closes the metadata block an AWS error carries.
const BLOCK_HEAD: &str = "(Service: ";
const BLOCK_TAIL: &str = "; Proxy: null)";

/// The AWS/Events counters a handover is counted on, the dimensions they are reported under and
/// the bus the rules live on.
const METRIC_NAMESPACE: &str = "AWS/Events";
const FAILED_INVOCATIONS_METRIC: &str = "FailedInvocations";
const INVOCATIONS_METRIC: &str = "Invocations";
const EVENT_BUS_NAME_DIMENSION: &str = "EventBusName";
const RULE_NAME_DIMENSION: &str = "RuleName";
const DEFAULT_EVENT_BUS: &str = "default";

/// The series every row's alarm watches: a namespace of the client's own, because `PutMetricData`
/// refuses an `AWS/` one.
const NAMESPACE: &str = "Custom/AlarmGovernance";
const METRIC: &str = "Backlog";

/// The period those alarms aggregate over, as the modelled seconds member carries it.
const PERIOD: i64 = 60;

/// The three configurations each row's alarm is written with, in order: the one it is created
/// with, the one the refused change replaces it with, and the one the change after the repair
/// replaces that with. Values of the row's own choosing — what matters is that each differs from
/// the one before, because a write that re-states a configuration changes none.
const CREATED_THRESHOLD: f64 = 1.0;
const REFUSED_THRESHOLD: f64 = 2.0;
const REPAIRED_THRESHOLD: f64 = 3.5;

/// How long a row waits for work the clock owes — the publication of an event, its delivery, the
/// counter the delivery is counted on — before giving up, and how often it looks meanwhile.
/// Generous: what is asserted is that it happens at all, never how fast the host runs.
const WORK_WAIT: Duration = Duration::from_secs(90);
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// How many looks a queue that must hold NOTHING is given: what such a queue holds is settled by
/// the time the sibling rule's delivery and the dead letter have both arrived, so these are
/// margin.
const QUIET_LOOKS: usize = 4;

/// CloudWatch, EventBridge, Amazon SQS and KMS over ONE engine, as `core/router/src/lib.rs` mounts
/// them: CloudWatch's publication reaches EventBridge's own handler, which matches it against this
/// account's rules and hands each matching rule's target over to the service that owns it.
fn build_router() -> MultiServiceRouter {
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
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked: this router runs with request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One `X-Amz-Target` RPC call, as its status and parsed body.
async fn rpc(
    router: &MultiServiceRouter,
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
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One CloudWatch call (`awsJson1_0`) that must answer 200.
async fn monitoring(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, parsed) = rpc(
        router,
        "monitoring",
        "GraniteServiceVersion20100801",
        "1.0",
        operation,
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudWatch {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One EventBridge call (`awsJson1_1`) that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Amazon SQS call (`awsJson1_0`) that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "sqs", "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Amazon SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One KMS call (`awsJson1_1`) that must answer 200.
async fn kms(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "kms", "TrentService", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from KMS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// A statement list wrapped as a policy document.
fn document(statements: Value) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// One statement about a service principal: `effect` of `action` on `resource`, scoped by
/// `condition` when the caller states one.
fn statement(
    effect: &str,
    principal: &str,
    action: &str,
    resource: &str,
    condition: Option<Value>,
) -> Value {
    let mut written = json!({
        "Effect": effect,
        "Principal": { "Service": principal },
        "Action": action,
        "Resource": resource,
    });
    if let Some(condition) = condition {
        written["Condition"] = condition;
    }
    written
}

/// The condition a least-privilege queue policy scopes a grant by: the deliveries of ONE rule.
fn from_rule(rule_arn: &str) -> Value {
    json!({ "ArnEquals": { "aws:SourceArn": rule_arn } })
}

/// The document that admits this service's send on `queue_arn` and nothing else — what the
/// sibling rule's queue carries from the start, and what each row's destination is repaired with.
fn admitting(queue_arn: &str) -> String {
    document(json!([statement(
        "Allow",
        EVENTS_PRINCIPAL,
        SEND_MESSAGE,
        queue_arn,
        None
    )]))
}

/// A queue called `name`: its url and the ARN a rule names it by, carrying `policy` when one is
/// stated.
async fn given_queue(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    policy: Option<String>,
) -> (String, String) {
    let created = sqs(router, case, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl in {created}"))
        .to_string();
    let read = sqs(
        router,
        case,
        "GetQueueAttributes",
        &json!({ "QueueUrl": &url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = read["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueArn in {read}"))
        .to_string();
    if let Some(policy) = policy {
        set_queue_attribute(router, case, &url, "Policy", &policy).await;
    }
    (url, arn)
}

/// Write one queue attribute, the way a client governs a queue.
async fn set_queue_attribute(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    name: &str,
    value: &str,
) {
    sqs(
        router,
        case,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { name: value } }),
    )
    .await;
}

/// A customer managed key carrying only the policy `CreateKey` writes, which admits no service
/// principal at all; answers its ARN.
async fn given_key(router: &MultiServiceRouter, case: &str) -> String {
    let created = kms(router, case, "CreateKey", &json!({})).await;
    created["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a KeyMetadata.Arn in {created}"))
        .to_string()
}

/// The key policy that admits this service the data key an encrypted queue's send needs, beside
/// the account root's own authority over the key — the statement `CreateKey` writes and the one a
/// replacement has to keep.
async fn admit_the_data_key(router: &MultiServiceRouter, case: &str, key_arn: &str) {
    let policy = json!({
        "Version": "2012-10-17",
        "Id": "key-default",
        "Statement": [
            {
                "Sid": "Enable IAM policies",
                "Effect": "Allow",
                "Principal": { "AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id()) },
                "Action": "kms:*",
                "Resource": "*",
            },
            statement("Allow", EVENTS_PRINCIPAL, "kms:GenerateDataKey*", "*", None),
        ],
    })
    .to_string();
    kms(
        router,
        case,
        "PutKeyPolicy",
        &json!({ "KeyId": key_arn, "PolicyName": "default", "Policy": policy }),
    )
    .await;
}

/// The pattern a rule carries to match the configuration changes of ONE alarm.
fn pattern(alarm: &str) -> String {
    json!({
        "source": [SOURCE],
        "detail-type": [DETAIL_TYPE],
        "detail": { "alarmName": [alarm] },
    })
    .to_string()
}

/// An ENABLED rule called `name` matching the configuration changes of `alarm`, carrying one queue
/// target — dead-lettering to `dead_letter_arn` with no retries when one is stated. Answers the
/// ARN EventBridge reports for the rule, read back from its own state.
async fn given_rule(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    alarm: &str,
    queue_arn: &str,
    dead_letter_arn: Option<&str>,
) -> String {
    events(
        router,
        case,
        "PutRule",
        &json!({ "Name": name, "EventPattern": pattern(alarm), "State": "ENABLED" }),
    )
    .await;
    let mut target = json!({ "Id": "q", "Arn": queue_arn });
    if let Some(dead_letter_arn) = dead_letter_arn {
        // "MaximumRetryAttempts: the maximum number of retry attempts to make before the request
        // fails" — none, so the refusal below is dead-lettered once.
        target["RetryPolicy"] = json!({ "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 });
        target["DeadLetterConfig"] = json!({ "Arn": dead_letter_arn });
    }
    let wired = events(
        router,
        case,
        "PutTargets",
        &json!({ "Rule": name, "Targets": [target] }),
    )
    .await;
    assert_eq!(
        wired["FailedEntryCount"],
        json!(0),
        "case {case:?}: expected the queue target of {name:?} to be accepted, got {wired}"
    );
    let described = events(router, case, "DescribeRule", &json!({ "Name": name })).await;
    described["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an Arn for the rule {name:?} in {described}"))
        .to_string()
}

/// Write the alarm `name` with `threshold`: the same configuration every time but for the
/// threshold, so each call after the first REPLACES a configuration with a different one.
async fn put_alarm(router: &MultiServiceRouter, case: &str, name: &str, threshold: f64) {
    monitoring(
        router,
        case,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Namespace": NAMESPACE,
            "MetricName": METRIC,
            "Dimensions": [{ "Name": "Row", "Value": name }],
            "Statistic": "Maximum",
            "Period": PERIOD,
            "EvaluationPeriods": 1,
            "Threshold": threshold,
            "ComparisonOperator": "GreaterThanThreshold",
            "TreatMissingData": "ignore",
        }),
    )
    .await;
}

/// The ARN CloudWatch reports for the alarm `name`.
async fn alarm_arn(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let described = monitoring(
        router,
        case,
        "DescribeAlarms",
        &json!({ "AlarmNames": [name] }),
    )
    .await;
    described["MetricAlarms"][0]["AlarmArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an AlarmArn in {described}"))
        .to_string()
}

/// Everything waiting on the queue at `url` right now, with its message attributes, taken OFF the
/// queue — so a later look reports what arrived after it rather than the same message again.
async fn take(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let answered = sqs(
        router,
        case,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "MessageAttributeNames": ["All"],
        }),
    )
    .await;
    let messages = answered["Messages"].as_array().cloned().unwrap_or_default();
    for message in &messages {
        if let Some(handle) = message["ReceiptHandle"].as_str() {
            sqs(
                router,
                case,
                "DeleteMessage",
                &json!({ "QueueUrl": url, "ReceiptHandle": handle }),
            )
            .await;
        }
    }
    messages
}

/// Look at the queue at `url` until it has handed over `wanted` messages or [`WORK_WAIT`] has
/// passed, answering everything it handed over meanwhile. The wait is for the CONDITION against a
/// generous deadline, so a slow host delays a row instead of breaking it.
async fn until_received(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    wanted: usize,
) -> Vec<Value> {
    let deadline = Instant::now() + WORK_WAIT;
    let mut held: Vec<Value> = Vec::new();
    loop {
        held.extend(take(router, case, url).await);
        if held.len() >= wanted || Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Everything the queue at `url` handed over across a run of looks, for a queue that must hand
/// over nothing.
async fn stayed_quiet(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let mut held: Vec<Value> = Vec::new();
    for _ in 0..QUIET_LOOKS {
        held.extend(take(router, case, url).await);
        if !held.is_empty() {
            return held;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    held
}

/// The string value of one message attribute.
fn attribute(message: &Value, name: &str) -> String {
    message["MessageAttributes"][name]["StringValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The envelope a message's body carries.
fn envelope(case: &str, message: &Value) -> Value {
    let body = message["Body"].as_str().unwrap_or_default();
    serde_json::from_str(body)
        .unwrap_or_else(|_| panic!("case {case:?}: expected an event envelope in {body:?}"))
}

/// One metadata block of a relayed AWS answer.
#[derive(Debug)]
struct AnswerBlock {
    service: String,
    status: String,
    code: String,
    request_id: String,
}

/// Every metadata block `message` carries, in the order the answer nests them — so the LAST is the
/// answer the delivery itself got from the target's own service.
fn answer_blocks(message: &str) -> Vec<AnswerBlock> {
    let mut blocks = Vec::new();
    let mut rest = message;
    while let Some(at) = rest.find(BLOCK_HEAD) {
        let after = &rest[at + BLOCK_HEAD.len()..];
        let Some(end) = after.find(BLOCK_TAIL) else {
            break;
        };
        let mut fields = after[..end].split("; ");
        let field = |fields: &mut std::str::Split<'_, &str>, prefix: &str| {
            fields
                .next()
                .and_then(|field| field.strip_prefix(prefix))
                .unwrap_or_default()
                .to_string()
        };
        blocks.push(AnswerBlock {
            service: fields.next().unwrap_or_default().to_string(),
            status: field(&mut fields, "Status Code: "),
            code: field(&mut fields, "Error Code: "),
            request_id: field(&mut fields, "Request ID: "),
        });
        rest = &after[end + BLOCK_TAIL.len()..];
    }
    blocks
}

/// Whether `request_id` is an AWS request id: a UUID in the canonical 8-4-4-4-12 form.
fn is_request_id(request_id: &str) -> bool {
    request_id.split('-').map(str::len).eq([8, 4, 4, 4, 12])
}

/// Whether `code` is one a queue target's service answers a send its documents withheld under,
/// by this suite's own list — false for a code no service of a queue delivery answers.
fn is_withheld_send_code(code: &str) -> bool {
    WITHHELD_SEND_CODES.contains(&code)
}

/// The `Sum` of the AWS/Events counter `metric` for the rule `rule` of the default bus, over a
/// window around now.
async fn counter(router: &MultiServiceRouter, case: &str, metric: &str, rule: &str) -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the host clock is after the epoch")
        .as_secs_f64();
    let read = monitoring(
        router,
        case,
        "GetMetricStatistics",
        &json!({
            "Namespace": METRIC_NAMESPACE,
            "MetricName": metric,
            "Dimensions": [
                { "Name": EVENT_BUS_NAME_DIMENSION, "Value": DEFAULT_EVENT_BUS },
                { "Name": RULE_NAME_DIMENSION, "Value": rule },
            ],
            "StartTime": now - 300.0,
            "EndTime": now + 300.0,
            "Period": 600,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    read["Datapoints"]
        .as_array()
        .map(|points| {
            points
                .iter()
                .filter_map(|point| point["Sum"].as_f64())
                .sum::<f64>()
        })
        .unwrap_or_default()
}

/// Look until the counter `metric` of `rule` has reached `wanted` or [`WORK_WAIT`] has passed,
/// answering what it read last.
async fn until_counted(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    rule: &str,
    wanted: f64,
) -> f64 {
    let deadline = Instant::now() + WORK_WAIT;
    loop {
        let read = counter(router, case, metric, rule).await;
        if read >= wanted || Instant::now() >= deadline {
            return read;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// The answer a row expects the delivery to have got from the destination's service.
#[derive(Clone, Copy, Debug)]
enum Answer {
    /// Amazon SQS's own not-found refusal, which it answers a sender its documents did not permit
    /// without disclosing the queue: fixed text in its entirety, in one metadata block.
    QueueNotDisclosed,
    /// The refusal the KEY an encrypted queue is encrypted under decided, relayed by that service
    /// under its own code: the one authorization failure a send really is answered with.
    KeyWithheld,
}

/// One row: the FORM in which the destination withholds the send, and the answer the delivery
/// therefore reads.
struct Row {
    case: &'static str,
    /// The document the target queue carries, over its own ARN, the delivering rule's ARN and the
    /// ARN of another rule of the same account — `None` for a queue carrying no document at all.
    policy: fn(&str, &str, &str) -> Option<String>,
    /// Whether the target queue is encrypted under a key whose policy admits nobody.
    encrypted: bool,
    /// The answer the refusal carries, and the class the row expects beside it.
    answer: Answer,
    class: &'static str,
}

/// RULE: a configuration change CloudWatch publishes that a queue target will not take is
/// dead-lettered under the class of the answer that service gave — the resource class for a send
/// it answers under its not-found code without disclosing the queue, and the permission class
/// where it relays the key's own authorization refusal —
/// while the same event still reaches a healthy
/// sibling rule's queue and the destination repaired afterwards takes the next change.
#[tokio::test]
async fn an_alarm_change_a_queue_target_withholds_is_classed_by_that_services_answer() {
    let rows = [
        Row {
            case: "a document denying the send, scoped by aws:SourceArn to the delivering rule",
            policy: |queue_arn, rule_arn, _| {
                Some(document(json!([statement(
                    "Deny",
                    EVENTS_PRINCIPAL,
                    SEND_MESSAGE,
                    queue_arn,
                    Some(from_rule(rule_arn)),
                )])))
            },
            encrypted: false,
            answer: Answer::QueueNotDisclosed,
            class: NO_RESOURCE,
        },
        Row {
            case: "a document denying the send outright beside a grant of it",
            policy: |queue_arn, _, _| {
                Some(document(json!([
                    statement("Allow", EVENTS_PRINCIPAL, SEND_MESSAGE, queue_arn, None),
                    statement("Deny", EVENTS_PRINCIPAL, SEND_MESSAGE, queue_arn, None),
                ])))
            },
            encrypted: false,
            answer: Answer::QueueNotDisclosed,
            class: NO_RESOURCE,
        },
        Row {
            case: "a queue carrying no document at all",
            policy: |_, _, _| None,
            encrypted: false,
            answer: Answer::QueueNotDisclosed,
            class: NO_RESOURCE,
        },
        Row {
            case: "a grant of the send to another service principal",
            policy: |queue_arn, _, _| {
                Some(document(json!([statement(
                    "Allow",
                    "s3.amazonaws.com",
                    SEND_MESSAGE,
                    queue_arn,
                    None,
                )])))
            },
            encrypted: false,
            answer: Answer::QueueNotDisclosed,
            class: NO_RESOURCE,
        },
        Row {
            case: "a grant of the send scoped by aws:SourceArn to another rule",
            policy: |queue_arn, _, other_rule_arn| {
                Some(document(json!([statement(
                    "Allow",
                    EVENTS_PRINCIPAL,
                    SEND_MESSAGE,
                    queue_arn,
                    Some(from_rule(other_rule_arn)),
                )])))
            },
            encrypted: false,
            answer: Answer::QueueNotDisclosed,
            class: NO_RESOURCE,
        },
        Row {
            case: "a grant of another action of the same service",
            policy: |queue_arn, _, _| {
                Some(document(json!([statement(
                    "Allow",
                    EVENTS_PRINCIPAL,
                    OTHER_QUEUE_ACTION,
                    queue_arn,
                    None,
                )])))
            },
            encrypted: false,
            answer: Answer::QueueNotDisclosed,
            class: NO_RESOURCE,
        },
        Row {
            case: "a queue granting the send, encrypted under a key admitting nobody",
            policy: |queue_arn, _, _| Some(admitting(queue_arn)),
            encrypted: true,
            answer: Answer::KeyWithheld,
            class: NO_PERMISSIONS,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let router = build_router();

        // The queues: the destination the row governs, the dead-letter queue its target names, and
        // the queue of a healthy sibling rule matched by the same event. Both of the latter admit
        // this service's send, so nothing about them can withhold what the row is about.
        let (dead_letter_url, dead_letter_arn) =
            given_queue(&router, case, &format!("cw-change-dlq-{index}"), None).await;
        set_queue_attribute(
            &router,
            case,
            &dead_letter_url,
            "Policy",
            &admitting(&dead_letter_arn),
        )
        .await;
        let (sibling_url, sibling_arn) =
            given_queue(&router, case, &format!("cw-change-sibling-{index}"), None).await;
        set_queue_attribute(&router, case, &sibling_url, "Policy", &admitting(&sibling_arn)).await;
        let (target_url, target_arn) =
            given_queue(&router, case, &format!("cw-change-target-{index}"), None).await;

        // The sibling rule goes on the bus BEFORE the alarm exists, so the event the alarm's
        // CREATION publishes has somewhere to go: taking it off that queue is what proves the
        // publication happened and leaves the queues empty for the change under test.
        let alarm = format!("alarm-governance-{index}");
        let sibling_rule = format!("cw-change-sibling-rule-{index}");
        let sibling_rule_arn =
            given_rule(&router, case, &sibling_rule, &alarm, &sibling_arn, None).await;
        put_alarm(&router, case, &alarm, CREATED_THRESHOLD).await;
        let alarm_arn = alarm_arn(&router, case, &alarm).await;
        let created = until_received(&router, case, &sibling_url, 1).await;
        assert_eq!(
            created.len(),
            1,
            "case {case:?}: expected the alarm's creation to publish one event to the sibling \
             rule's queue, got {created:?}"
        );
        assert_eq!(
            envelope(case, &created[0]).pointer("/detail/operation"),
            Some(&json!(OPERATION_CREATE)),
            "case {case:?}: expected the alarm's creation to be reported as a create, got {:?}",
            envelope(case, &created[0])
        );

        // The rule whose target the row's destination is, and the destination as the row leaves
        // it: the document it carries, and the key its messages are encrypted under.
        let rule = format!("cw-change-rule-{index}");
        let rule_arn = given_rule(
            &router,
            case,
            &rule,
            &alarm,
            &target_arn,
            Some(&dead_letter_arn),
        )
        .await;
        let key_arn = if row.encrypted {
            let key_arn = given_key(&router, case).await;
            set_queue_attribute(&router, case, &target_url, "KmsMasterKeyId", &key_arn).await;
            Some(key_arn)
        } else {
            None
        };
        if let Some(policy) = (row.policy)(&target_arn, &rule_arn, &sibling_rule_arn) {
            set_queue_attribute(&router, case, &target_url, "Policy", &policy).await;
        }

        // ONE write that replaces the alarm's configuration, which publishes ONE event matched by
        // both rules.
        put_alarm(&router, case, &alarm, REFUSED_THRESHOLD).await;

        // The sibling rule's delivery: the event was produced, matched and handed over, so what
        // the row is about is the destination and nothing else.
        let sibling_held = until_received(&router, case, &sibling_url, 1).await;
        assert_eq!(
            sibling_held.len(),
            1,
            "case {case:?}: expected the configuration change on the sibling rule's queue, got \
             {sibling_held:?}"
        );
        let sibling_event = envelope(case, &sibling_held[0]);
        assert_eq!(
            sibling_event.pointer("/detail/operation"),
            Some(&json!(OPERATION_UPDATE)),
            "case {case:?}: expected the sibling rule's copy to report an update, got \
             {sibling_event}"
        );

        // The dead letter, which is where the refusal is read.
        let letters = until_received(&router, case, &dead_letter_url, 1).await;
        assert_eq!(
            letters.len(),
            1,
            "case {case:?}: expected exactly one dead letter for a delivery that is not retried, \
             got {letters:?}"
        );
        let letter = &letters[0];

        // What it carries is the event the destination did not take, named by the rule that
        // routed it and the target it was routed to — both read from the services' own state.
        let dead_event = envelope(case, letter);
        for (pointer, expected) in [
            ("/source", json!(SOURCE)),
            ("/detail-type", json!(DETAIL_TYPE)),
            ("/region", json!(REGION)),
            ("/resources", json!([&alarm_arn])),
            ("/detail/alarmName", json!(&alarm)),
            ("/detail/operation", json!(OPERATION_UPDATE)),
            ("/detail/configuration/threshold", json!(REFUSED_THRESHOLD)),
        ] {
            let actual = dead_event.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the dead letter's {pointer} to be {expected}, got \
                 {actual} in {dead_event}"
            );
        }
        for (name, expected) in [
            (RULE_ARN_ATTRIBUTE, &rule_arn),
            (TARGET_ARN_ATTRIBUTE, &target_arn),
        ] {
            let actual = attribute(letter, name);
            assert_eq!(
                &actual, expected,
                "case {case:?}: expected the dead letter's {name} to be {expected:?}, got \
                 {actual:?} in {letter}"
            );
        }

        // The answer beside the class, and the class the code INSIDE that answer belongs to.
        let message = attribute(letter, ERROR_MESSAGE_ATTRIBUTE);
        let blocks = answer_blocks(&message);
        let answered = blocks.last().unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the {ERROR_MESSAGE_ATTRIBUTE} to carry the answer the \
                 destination's service gave, got {message:?}"
            )
        });
        for block in &blocks {
            assert!(
                is_request_id(&block.request_id),
                "case {case:?}: expected each refused call's request id in the 8-4-4-4-12 form, \
                 got {:?} in {message:?}",
                block.request_id
            );
        }
        match row.answer {
            Answer::QueueNotDisclosed => {
                // Fixed text in its entirety but for the id of the one refused call: nothing of
                // the request, of the queue or of the account appears in it.
                let expected = format!(
                    "{QUEUE_NOT_DISCLOSED_SENTENCE} {BLOCK_HEAD}{SQS_SERVICE_NAME}; Status Code: \
                     {QUEUE_NOT_DISCLOSED_STATUS}; Error Code: {QUEUE_NOT_DISCLOSED_CODE}; \
                     Request ID: {}{BLOCK_TAIL}",
                    answered.request_id
                );
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected the destination's own not-found refusal, got \
                     {message:?}"
                );
            }
            Answer::KeyWithheld => {
                assert_eq!(
                    (answered.service.as_str(), answered.status.as_str(), answered.code.as_str()),
                    (SQS_SERVICE_NAME, QUEUE_NOT_DISCLOSED_STATUS, KEY_WITHHELD_CODE),
                    "case {case:?}: expected the destination's service to relay the key's \
                     refusal, got {message:?}"
                );
                assert!(
                    message.contains(DATA_KEY_ACTION) && message.contains(EVENTS_PRINCIPAL),
                    "case {case:?}: expected the relayed refusal to name the data-key call it \
                     was made for, got {message:?}"
                );
            }
        }
        let class = attribute(letter, ERROR_CODE_ATTRIBUTE);
        assert!(
            is_withheld_send_code(&answered.code),
            "case {case:?}: no service of a queue delivery answers the code {:?}, read from \
             {message:?}",
            answered.code
        );
        assert_eq!(
            class.as_str(),
            row.class,
            "case {case:?}: expected the {ERROR_CODE_ATTRIBUTE} {:?} for the answer {message:?}",
            row.class
        );

        // Nothing was enqueued on the destination itself.
        let withheld = stayed_quiet(&router, case, &target_url).await;
        assert!(
            withheld.is_empty(),
            "case {case:?}: a withheld send enqueues nothing on the destination, got {withheld:?}"
        );

        // The refused handover is counted on the rule it was refused for, and on no other rule.
        let failed = until_counted(&router, case, FAILED_INVOCATIONS_METRIC, &rule, 1.0).await;
        assert!(
            failed >= 1.0,
            "case {case:?}: expected the refused handover to be counted on {rule:?}'s \
             {FAILED_INVOCATIONS_METRIC}, got {failed}"
        );
        let sibling_failed =
            counter(&router, case, FAILED_INVOCATIONS_METRIC, &sibling_rule).await;
        assert_eq!(
            sibling_failed, 0.0,
            "case {case:?}: the sibling rule's deliveries were made, so none of them may be \
             counted on its {FAILED_INVOCATIONS_METRIC}, got {sibling_failed}"
        );
        let sibling_invocations =
            until_counted(&router, case, INVOCATIONS_METRIC, &sibling_rule, 1.0).await;
        assert!(
            sibling_invocations >= 1.0,
            "case {case:?}: expected the sibling rule's deliveries on its \
             {INVOCATIONS_METRIC}, got {sibling_invocations}"
        );

        // The repair: the document or the key that withheld the send now admits it, and the NEXT
        // configuration change arrives on the destination with nothing else changed.
        match key_arn.as_deref() {
            Some(key_arn) => admit_the_data_key(&router, case, key_arn).await,
            None => {
                set_queue_attribute(&router, case, &target_url, "Policy", &admitting(&target_arn))
                    .await;
            }
        }
        put_alarm(&router, case, &alarm, REPAIRED_THRESHOLD).await;
        let repaired = until_received(&router, case, &target_url, 1).await;
        assert_eq!(
            repaired.len(),
            1,
            "case {case:?}: expected the change after the repair on the destination, got \
             {repaired:?}"
        );
        let repaired_event = envelope(case, &repaired[0]);
        assert_eq!(
            repaired_event.pointer("/detail/configuration/threshold"),
            Some(&json!(REPAIRED_THRESHOLD)),
            "case {case:?}: expected the configuration the repaired delivery carries to be the \
             one the last write stored, got {repaired_event}"
        );
        let dead_again = stayed_quiet(&router, case, &dead_letter_url).await;
        assert!(
            dead_again.is_empty(),
            "case {case:?}: a delivery that was made is dead-lettered nowhere, got {dead_again:?}"
        );
    }
}
