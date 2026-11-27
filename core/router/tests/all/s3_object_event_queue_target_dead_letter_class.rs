// SPDX-License-Identifier: CC0-1.0

//! WHICH class EventBridge dead-letters an object event under when the Amazon SQS queue a
//! rule targets does not let this service send it a message — the commonest
//! S3 -> EventBridge -> SQS misconfiguration.
//!
//! RULE: the dead letter's `ERROR_CODE` is the class of the error CODE the target's own
//! service answered the refused call under, and the sibling `ERROR_MESSAGE` carries that
//! answer in full. For a queue target whose documents withheld the `sqs:SendMessage` that
//! answer is Amazon SQS's own not-found refusal: that service models no authorization
//! failure on `SendMessage` at all (its errors are `InvalidAddress`,
//! `InvalidMessageContents`, `InvalidSecurity`, the `KMS.*` family, `QueueDoesNotExist`,
//! `RequestThrottled` and `UnsupportedOperation`, `smithy_query(query="operation",
//! operation="SendMessage",
//! model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`),
//! and it does not disclose a queue it will not let the caller write to, so it answers
//! `QueueDoesNotExist` — HTTP 400 under the `awsQueryError` code
//! `AWS.SimpleQueueService.NonExistentQueue` (`smithy_query(query="shape",
//! shape="QueueDoesNotExist", model_path="…")`) — so the class read off it is the RESOURCE
//! one, never the permission one.
//!
//! That class says nothing about whether the queue is THERE: the second test here writes a
//! grant onto the same queue url and watches the next object event arrive on it, with the
//! bucket, the rule and the target otherwise untouched.
//!
//! The rows differ along what the rule generalises over: the object change that publishes
//! the event (a write, a delete), the FORM the withholding takes (no policy at all, a grant
//! whose `aws:SourceArn` condition names a DIFFERENT rule, an explicit `Deny` beside a
//! grant), what the target's `RetryPolicy` allows (none at all, no retries, several) and
//! whether a PERMITTED sibling target of the same rule took the same event. A control row
//! whose queue grants the send receives the event and parks no dead letter, so no row passes
//! because nothing was delivered anywhere.
//!
//! Only a cross-service suite can state this: the object event is Amazon S3's, published
//! because the bucket carries an `EventBridgeConfiguration`; the pattern match, the send and
//! the dead-letter write are EventBridge's; and both queues and the document that withholds
//! the send are Amazon SQS's — all on the ONE engine `core/router/src/lib.rs` mounts.
//!
//! [`super::events_queue_target_refusal_class_follows_the_answer_not_the_origin`] pins the
//! same rule for an event a client published and for one CloudTrail fanned out, and
//! [`super::s3_eventbridge_pipeline_repair_delivers_the_next_event`] pins what a repair to
//! the S3 -> EventBridge -> Lambda chain does to the next object event. Neither states what
//! a bucket's OWN event is dead-lettered as when the queue it is routed to withholds the
//! send, which is what this suite is about.

use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one each bucket is created in.
const REGION: &str = "us-west-2";

/// The service principal EventBridge hands a resource-policy-governed target over as, and
/// the action a delivery to a queue target performs on that queue.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";
const SEND_MESSAGE: &str = "sqs:SendMessage";

/// The `source` every event an EventBridge-enabled bucket publishes carries, and the two
/// `detail-type`s the object changes below are published under.
const S3_SOURCE: &str = "aws.s3";
const OBJECT_CREATED: &str = "Object Created";
const OBJECT_DELETED: &str = "Object Deleted";

/// The S3 document namespace a bucket configuration is written in.
const S3_XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The bus a bucket's own events are published onto, which is the only one it publishes
/// onto.
const DEFAULT_BUS: &str = "default";

/// The dead-letter message attributes EventBridge adds to the dead-letter message.
const RULE_ARN_ATTRIBUTE: &str = "RULE_ARN";
const TARGET_ARN_ATTRIBUTE: &str = "TARGET_ARN";
const ERROR_CODE_ATTRIBUTE: &str = "ERROR_CODE";
const ERROR_MESSAGE_ATTRIBUTE: &str = "ERROR_MESSAGE";

/// EventBridge's class for a delivery answered under a target service's not-found code,
/// which is what Amazon SQS answers a send it will not disclose the queue for.
const NO_RESOURCE: &str = "NO_RESOURCE";

/// The fixed sentence Amazon SQS answers a send the documents in force over the queue do not
/// permit — RECORDED, and written out here rather than read from any implementation, so a
/// wrong answer cannot make this suite pass. It states both the absence and the want of
/// access without saying which holds, and carries nothing of the request.
const QUEUE_NOT_DISCLOSED_SENTENCE: &str =
    "The specified queue does not exist or you do not have access to it.";

/// One answering service's refusal, as the dead letter's `ERROR_MESSAGE` quotes it: the
/// error code that answer names and the status beside it.
struct AnswerClass {
    code: &'static str,
    status: u16,
}

/// The two answers a refused delivery to a queue target can read.
///
/// `AWS.SimpleQueueService.NonExistentQueue` is `QueueDoesNotExist`'s `awsQueryError` code
/// and `AccessDenied` the code an authorization refusal is answered under; a dead letter has
/// to quote exactly one of them, which is what makes an answer naming two a failure of this
/// suite rather than a detail it ignores.
const ANSWER_CLASSES: [AnswerClass; 2] = [
    AnswerClass {
        code: "AWS.SimpleQueueService.NonExistentQueue",
        status: 400,
    },
    AnswerClass {
        code: "AccessDenied",
        status: 403,
    },
];

/// How long the arrival of what a row expects is waited for, and how long between two looks.
/// Generous: each wait is on the CONDITION (the message is there), and the deadline is only
/// the point at which waiting longer cannot help.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(60);
const LOOK_INTERVAL: Duration = Duration::from_millis(25);

/// How many looks a queue that must stay EMPTY is given before it is believed.
const QUIET_LOOKS: usize = 4;

/// The object change whose event a row's rule matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    /// A `PutObject`, published as an "Object Created" event.
    Written,
    /// A `DeleteObject` of a key written first, published as an "Object Deleted" event.
    Deleted,
}

impl Change {
    /// The `detail-type` the change is published under.
    fn detail_type(self) -> &'static str {
        match self {
            Self::Written => OBJECT_CREATED,
            Self::Deleted => OBJECT_DELETED,
        }
    }
}

/// How the row's target queue withholds the send — or grants it, for the control row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Document {
    /// No resource policy at all, which has granted this service nothing.
    None,
    /// A grant for this service whose `aws:SourceArn` condition names a DIFFERENT rule, so
    /// it authorizes that rule's deliveries and not this one's.
    ScopedToAnotherRule,
    /// A grant with an explicit `Deny` of the same send beside it.
    DenyBesideGrant,
    /// A grant of the send to this service.
    Grants,
}

/// One row: the object change its event comes from, what its target queue's document does
/// with the send, what its `RetryPolicy` allows, and whether a PERMITTED sibling target of
/// the same rule takes the same event.
struct Row {
    case: &'static str,
    change: Change,
    document: Document,
    /// `MaximumRetryAttempts`, or `None` for a target carrying no `RetryPolicy` at all.
    retry_attempts: Option<i32>,
    sibling: bool,
}

/// The rows, in the order they run. The first is the recorded case this suite was written
/// for; the rest vary one axis of it at a time.
const ROWS: [Row; 5] = [
    Row {
        case: "an object write, a target queue with no policy at all, no retries, beside a \
               permitted sibling",
        change: Change::Written,
        document: Document::None,
        retry_attempts: Some(0),
        sibling: true,
    },
    Row {
        case: "an object write, a grant whose aws:SourceArn names another rule, retries \
               allowed, no sibling",
        change: Change::Written,
        document: Document::ScopedToAnotherRule,
        retry_attempts: Some(3),
        sibling: false,
    },
    Row {
        case: "an object delete, a policy denying the send beside granting it, no retry \
               policy at all, beside a permitted sibling",
        change: Change::Deleted,
        document: Document::DenyBesideGrant,
        retry_attempts: None,
        sibling: true,
    },
    Row {
        case: "an object delete, a target queue with no policy at all, one retry",
        change: Change::Deleted,
        document: Document::None,
        retry_attempts: Some(1),
        sibling: false,
    },
    Row {
        case: "an object write whose target queue grants the send",
        change: Change::Written,
        document: Document::Grants,
        retry_attempts: Some(0),
        sibling: false,
    },
];

/// The whole emulator over one engine, exactly as the binary assembles it.
fn router() -> MultiServiceRouter {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(engine);
    // Signature verification is the front door's concern, not this suite's; the credential
    // scope is still carried, because it is what names the Region.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature itself is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
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
    let request = ::http::Request::builder()
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
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// An EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with {parsed}"
    );
    parsed
}

/// An Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "sqs", "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Amazon SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Amazon S3 REST call, path-style as the emulator serves it, as its status and body.
async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: &str,
) -> (u16, String) {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = ::http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", &host)
        .header("authorization", authorization("s3"))
        .header("content-type", "application/xml")
        .body(Bytes::from(body.to_owned()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).into_owned(),
    )
}

/// A bucket of the Region whose `NotificationConfiguration` "enables delivery of events to
/// Amazon EventBridge" (`NotificationConfiguration$EventBridgeConfiguration`) — the one
/// condition a bucket's own events being published hangs off.
async fn given_event_bridge_bucket(router: &MultiServiceRouter, case: &str, bucket: &str) {
    let configuration = format!(
        "<CreateBucketConfiguration xmlns=\"{S3_XMLNS}\">\
         <LocationConstraint>{REGION}</LocationConstraint>\
         </CreateBucketConfiguration>"
    );
    let (status, body) = s3(router, "PUT", &format!("/{bucket}"), &configuration).await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: CreateBucket({bucket:?}) must succeed, got {status} with {body}"
    );
    let notification = format!(
        "<NotificationConfiguration xmlns=\"{S3_XMLNS}\">\
         <EventBridgeConfiguration></EventBridgeConfiguration>\
         </NotificationConfiguration>"
    );
    let (status, body) = s3(
        router,
        "PUT",
        &format!("/{bucket}?notification"),
        &notification,
    )
    .await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: enabling EventBridge notifications on {bucket:?} must succeed, got \
         {status} with {body}"
    );
}

/// Make the row's object change happen on `bucket`, whose event the rule then matches: a
/// write, or a write followed by the delete whose own event is the one that matches.
async fn make_the_change(
    router: &MultiServiceRouter,
    case: &str,
    change: Change,
    bucket: &str,
    key: &str,
) {
    let (status, body) = s3(
        router,
        "PUT",
        &format!("/{bucket}/{key}"),
        "the object whose change is published",
    )
    .await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: PutObject({key:?}) must succeed, got {status} with {body}"
    );
    if change == Change::Deleted {
        let (status, body) = s3(router, "DELETE", &format!("/{bucket}/{key}"), "").await;
        assert!(
            (200..300).contains(&status),
            "case {case:?}: DeleteObject({key:?}) must succeed, got {status} with {body}"
        );
    }
}

/// A statement list wrapped as a policy document.
fn document(statements: Value) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// A grant of the send to this service on `queue_arn`.
fn granting(queue_arn: &str) -> String {
    document(json!([{
        "Effect": "Allow",
        "Principal": { "Service": EVENTS_PRINCIPAL },
        "Action": SEND_MESSAGE,
        "Resource": queue_arn,
    }]))
}

/// A grant of the send to this service on `queue_arn` for the deliveries of the rule
/// `rule_arn` and no other.
fn granting_for_rule(queue_arn: &str, rule_arn: &str) -> String {
    document(json!([{
        "Effect": "Allow",
        "Principal": { "Service": EVENTS_PRINCIPAL },
        "Action": SEND_MESSAGE,
        "Resource": queue_arn,
        "Condition": { "ArnEquals": { "aws:SourceArn": rule_arn } },
    }]))
}

/// A grant of the send with an explicit `Deny` of the same send beside it.
fn denying_beside_granting(queue_arn: &str) -> String {
    document(json!([
        {
            "Effect": "Allow",
            "Principal": { "Service": EVENTS_PRINCIPAL },
            "Action": SEND_MESSAGE,
            "Resource": queue_arn,
        },
        {
            "Effect": "Deny",
            "Principal": { "Service": EVENTS_PRINCIPAL },
            "Action": SEND_MESSAGE,
            "Resource": queue_arn,
        },
    ]))
}

/// A queue called `name`, carrying no document of its own yet, as its url and the ARN a
/// target names it by — read back from the queue rather than built here, which is also what
/// a document naming the queue is then written from ([`write_policy`]).
async fn given_queue(router: &MultiServiceRouter, case: &str, name: &str) -> (String, String) {
    let created = sqs(router, case, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl for {name:?}, got {created}"))
        .to_string();
    let attributes = sqs(
        router,
        case,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueArn, got {attributes}"))
        .to_string();
    (url, arn)
}

/// A queue called `name` whose own document GRANTS this service the send, as its url and
/// ARN: the dead-letter queue every row writes to, and the permitted sibling target.
async fn given_granting_queue(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> (String, String) {
    let (url, arn) = given_queue(router, case, name).await;
    write_policy(router, case, &url, &granting(&arn)).await;
    (url, arn)
}

/// Write `policy` onto the queue at `url`, the way a client grants a queue target.
async fn write_policy(router: &MultiServiceRouter, case: &str, url: &str, policy: &str) {
    sqs(
        router,
        case,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
    )
    .await;
}

/// Everything the queue at `url` holds right now, with its message attributes.
async fn look(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
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
    answered["Messages"].as_array().cloned().unwrap_or_default()
}

/// Look at `url` until it has held at least `count` messages, or [`DELIVERY_DEADLINE`]
/// passes — the CONDITION, never a span of time.
async fn held(router: &MultiServiceRouter, case: &str, url: &str, count: usize) -> Vec<Value> {
    let mut taken: Vec<Value> = Vec::new();
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        taken.extend(look(router, case, url).await);
        if taken.len() >= count {
            return taken;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {count} message(s) on {url}, got {taken:?}"
        );
        ::tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Everything `url` holds over [`QUIET_LOOKS`] looks — for a queue that must stay empty.
async fn quiet(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let mut taken: Vec<Value> = Vec::new();
    for _ in 0..QUIET_LOOKS {
        taken.extend(look(router, case, url).await);
        ::tokio::time::sleep(LOOK_INTERVAL).await;
    }
    taken
}

/// The string value of a message attribute, or `""` where the message carries none.
fn attribute(message: &Value, name: &str) -> String {
    message["MessageAttributes"][name]["StringValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Assert the message carries the bucket's own object event: the `source` and `detail-type`
/// the change is published under, and the bucket and key it is about.
fn assert_is_the_object_event(
    case: &str,
    message: &Value,
    change: Change,
    bucket: &str,
    key: &str,
) {
    let body = message["Body"].as_str().unwrap_or_default();
    let carried: Value = serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("case {case:?}: a delivered body is JSON ({error}): {body}"));
    assert_eq!(
        (carried["source"].clone(), carried["detail-type"].clone()),
        (json!(S3_SOURCE), json!(change.detail_type())),
        "case {case:?}: expected the bucket's own {:?} envelope, got {carried}",
        change.detail_type()
    );
    assert_eq!(
        (
            carried.pointer("/detail/bucket/name").cloned(),
            carried.pointer("/detail/object/key").cloned()
        ),
        (Some(json!(bucket)), Some(json!(key))),
        "case {case:?}: expected the event of {key:?} in {bucket:?}, got {carried}"
    );
}

/// Assert the dead letter's two attributes name ONE failure: the answer Amazon SQS gives a
/// send the documents in force over the queue do not permit, and the class that answer's own
/// code belongs to.
///
/// The pair is read the way a consumer reads it: the error code quoted in `ERROR_MESSAGE` has
/// to be one answer of [`ANSWER_CLASSES`], with the status that answer carries, and
/// `ERROR_CODE` has to be the class of that code. So a letter whose halves belong to
/// different failures fails here however plausible each half is on its own.
fn assert_withheld_class(case: &str, message: &Value) {
    let reported = attribute(message, ERROR_MESSAGE_ATTRIBUTE);
    let named: Vec<&AnswerClass> = ANSWER_CLASSES
        .iter()
        .filter(|answer| reported.contains(&format!("Error Code: {};", answer.code)))
        .collect();
    assert_eq!(
        named.len(),
        1,
        "case {case:?}: expected the answer to name exactly one refusal class, got {reported:?}"
    );
    let answer = named[0];
    assert!(
        reported.contains(&format!("Status Code: {};", answer.status)),
        "case {case:?}: expected the status {} beside the code {:?}, got {reported:?}",
        answer.status,
        answer.code
    );
    let class = attribute(message, ERROR_CODE_ATTRIBUTE);
    // A send the queue's own documents withheld reads that service's own fixed sentence
    // under its not-found code, so the class beside it is the resource one.
    assert_eq!(
        class, NO_RESOURCE,
        "case {case:?}: expected the class of the answer that service gave a send the queue's \
         documents withheld, got {class:?} with {reported:?}"
    );
    assert!(
        reported.contains(QUEUE_NOT_DISCLOSED_SENTENCE),
        "case {case:?}: expected the sentence {QUEUE_NOT_DISCLOSED_SENTENCE:?}, got {reported:?}"
    );
}

/// The `EventPattern` a row's rule matches its own bucket's change with, scoped to the
/// bucket so no other row's bucket can reach it.
fn pattern(change: Change, bucket: &str) -> Value {
    json!({
        "source": [S3_SOURCE],
        "detail-type": [change.detail_type()],
        "detail": { "bucket": { "name": [bucket] } },
    })
}

/// Install a rule on the default bus with the targets described, and answer the rule's own
/// ARN — the one the dead letter has to name.
async fn given_rule(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    change: Change,
    bucket: &str,
    targets: Value,
) -> String {
    let rule = events(
        router,
        case,
        "PutRule",
        &json!({
            "Name": name,
            "EventBusName": DEFAULT_BUS,
            "State": "ENABLED",
            "EventPattern": pattern(change, bucket).to_string(),
        }),
    )
    .await;
    let rule_arn = rule["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a RuleArn, got {rule}"))
        .to_string();
    let put = events(
        router,
        case,
        "PutTargets",
        &json!({ "Rule": name, "EventBusName": DEFAULT_BUS, "Targets": targets }),
    )
    .await;
    assert_eq!(
        put["FailedEntryCount"],
        json!(0),
        "case {case:?}: every target must be installed, got {put}"
    );
    rule_arn
}

/// A rule carrying no target, installed only so a grant can be scoped to an ARN that is not
/// the row's own rule.
async fn given_other_rule(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let installed = events(
        router,
        case,
        "PutRule",
        &json!({
            "Name": name,
            "State": "ENABLED",
            "EventPattern": json!({ "source": ["sim2real.s3.other"] }).to_string(),
        }),
    )
    .await;
    installed["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a RuleArn, got {installed}"))
        .to_string()
}

/// A bucket's own object event, routed to a queue target the documents in force over that
/// queue withhold the send from, is dead-lettered under the class of the answer Amazon SQS
/// gave — the resource one — with that answer's own sentence beside it.
#[tokio::test]
async fn a_withheld_object_event_delivery_carries_the_answering_services_class() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let router = router();
        let bucket = format!("object-event-class-{index}");
        given_event_bridge_bucket(&router, case, &bucket).await;

        let other_rule = if row.document == Document::ScopedToAnotherRule {
            given_other_rule(&router, case, &format!("object-event-other-{index}")).await
        } else {
            String::new()
        };

        let (dead_letter_url, dead_letter_arn) =
            given_granting_queue(&router, case, &format!("object-event-dlq-{index}")).await;
        let (target_url, target_arn) =
            given_queue(&router, case, &format!("object-event-target-{index}")).await;
        // The document the row's target queue carries, written from that queue's own ARN.
        let withholding = match row.document {
            Document::None => None,
            Document::ScopedToAnotherRule => Some(granting_for_rule(&target_arn, &other_rule)),
            Document::DenyBesideGrant => Some(denying_beside_granting(&target_arn)),
            Document::Grants => Some(granting(&target_arn)),
        };
        if let Some(withholding) = &withholding {
            write_policy(&router, case, &target_url, withholding).await;
        }
        let sibling = if row.sibling {
            Some(given_granting_queue(&router, case, &format!("object-event-sibling-{index}")).await)
        } else {
            None
        };

        let mut target = json!({
            "Id": "withheld",
            "Arn": target_arn,
            "DeadLetterConfig": { "Arn": dead_letter_arn },
        });
        if let Some(attempts) = row.retry_attempts {
            target["RetryPolicy"] =
                json!({ "MaximumRetryAttempts": attempts, "MaximumEventAgeInSeconds": 60 });
        }
        let mut targets = vec![target];
        if let Some((_, sibling_arn)) = &sibling {
            targets.push(json!({ "Id": "permitted", "Arn": sibling_arn }));
        }
        let rule_arn = given_rule(
            &router,
            case,
            &format!("object-event-rule-{index}"),
            row.change,
            &bucket,
            json!(targets),
        )
        .await;

        let key = format!("changed/object-{index}.bin");
        make_the_change(&router, case, row.change, &bucket, &key).await;

        if let Some((sibling_url, _)) = &sibling {
            let delivered = held(&router, case, sibling_url, 1).await;
            assert_eq!(
                delivered.len(),
                1,
                "case {case:?}: expected the permitted sibling to receive the event, got \
                 {delivered:?}"
            );
            assert_is_the_object_event(case, &delivered[0], row.change, &bucket, &key);
        }

        if row.document == Document::Grants {
            // The control row: a granted queue takes the event, and nothing is dead-lettered.
            let delivered = held(&router, case, &target_url, 1).await;
            assert_is_the_object_event(case, &delivered[0], row.change, &bucket, &key);
            let dead_lettered = quiet(&router, case, &dead_letter_url).await;
            assert!(
                dead_lettered.is_empty(),
                "case {case:?}: a delivery that went through is not dead-lettered, got \
                 {dead_lettered:?}"
            );
            continue;
        }

        let dead_lettered = held(&router, case, &dead_letter_url, 1).await;
        assert_eq!(
            dead_lettered.len(),
            1,
            "case {case:?}: expected exactly one dead letter, got {dead_lettered:?}"
        );
        let letter = &dead_lettered[0];
        // The body is the event itself, so a consumer re-drives what was not delivered.
        assert_is_the_object_event(case, letter, row.change, &bucket, &key);
        assert_eq!(
            (
                attribute(letter, RULE_ARN_ATTRIBUTE),
                attribute(letter, TARGET_ARN_ATTRIBUTE)
            ),
            (rule_arn.clone(), target_arn.clone()),
            "case {case:?}: expected the dead letter to name the rule and the target of the \
             failed delivery, got {letter:?}"
        );
        assert_withheld_class(case, letter);
        // Nothing was enqueued on the target the send was withheld from.
        let leaked = quiet(&router, case, &target_url).await;
        assert!(
            leaked.is_empty(),
            "case {case:?}: a withheld delivery leaves the target empty, got {leaked:?}"
        );
    }
}

/// The class says nothing about whether the queue is THERE: the same queue url takes the next
/// object event as soon as its policy grants the send, with the bucket, the rule and the
/// target otherwise untouched.
#[tokio::test]
async fn a_granted_policy_delivers_the_next_object_event_to_the_same_queue() {
    let case = "a dead-lettered object event delivered once the queue's policy grants the send";
    let router = router();
    let bucket = "object-event-repair";
    given_event_bridge_bucket(&router, case, bucket).await;

    let (dead_letter_url, dead_letter_arn) =
        given_granting_queue(&router, case, "object-event-repair-dlq").await;
    let (target_url, target_arn) = given_queue(&router, case, "object-event-repair-target").await;
    let rule_arn = given_rule(
        &router,
        case,
        "object-event-repair-rule",
        Change::Written,
        bucket,
        json!([{
            "Id": "withheld",
            "Arn": target_arn,
            "DeadLetterConfig": { "Arn": dead_letter_arn },
            "RetryPolicy": { "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 },
        }]),
    )
    .await;

    let withheld_key = "repair/before-the-grant.bin";
    make_the_change(&router, case, Change::Written, bucket, withheld_key).await;
    let dead_lettered = held(&router, case, &dead_letter_url, 1).await;
    assert_eq!(
        dead_lettered.len(),
        1,
        "case {case:?}: expected exactly one dead letter, got {dead_lettered:?}"
    );
    assert_eq!(
        attribute(&dead_lettered[0], RULE_ARN_ATTRIBUTE),
        rule_arn,
        "case {case:?}: expected the dead letter to name the rule, got {:?}",
        dead_lettered[0]
    );
    assert_is_the_object_event(
        case,
        &dead_lettered[0],
        Change::Written,
        bucket,
        withheld_key,
    );
    assert_withheld_class(case, &dead_lettered[0]);
    let leaked = quiet(&router, case, &target_url).await;
    assert!(
        leaked.is_empty(),
        "case {case:?}: a withheld delivery leaves the target empty, got {leaked:?}"
    );

    // The one thing that changes: the queue's own document now grants the send.
    write_policy(&router, case, &target_url, &granting(&target_arn)).await;
    let granted_key = "repair/after-the-grant.bin";
    make_the_change(&router, case, Change::Written, bucket, granted_key).await;
    let delivered = held(&router, case, &target_url, 1).await;
    assert_eq!(
        delivered.len(),
        1,
        "case {case:?}: expected the granted queue to receive the next event, got {delivered:?}"
    );
    assert_is_the_object_event(case, &delivered[0], Change::Written, bucket, granted_key);
    let dead_lettered = quiet(&router, case, &dead_letter_url).await;
    assert!(
        dead_lettered.is_empty(),
        "case {case:?}: a delivery that went through is not dead-lettered, got {dead_lettered:?}"
    );
}
