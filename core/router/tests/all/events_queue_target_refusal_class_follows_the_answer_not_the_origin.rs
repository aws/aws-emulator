// SPDX-License-Identifier: CC0-1.0

//! WHICH class a dead letter carries when an Amazon SQS target's own resource policy
//! withheld the delivery, and what that class does NOT depend on: where the event came
//! from, which bus carried it, what the target's `RetryPolicy` allows, and whether a
//! sibling target of the same rule was delivered.
//!
//! RULE: the dead letter's `ERROR_CODE` is the class of the error CODE the target's own
//! service answered the refused call under, and the sibling `ERROR_MESSAGE` carries that
//! answer in full. Two consequences this suite pins:
//!
//! * which class a delivery the documents in force over the target withheld lands in does
//!   not depend on where the event came from; and
//! * for a queue target whose documents withheld the `sqs:SendMessage`, that answer is
//!   Amazon SQS's own not-found refusal: that service models no authorization
//!   failure on
//!   `SendMessage` at all (its errors are `InvalidAddress`, `InvalidMessageContents`,
//!   `InvalidSecurity`, the `KMS.*` family, `QueueDoesNotExist`, `RequestThrottled` and
//!   `UnsupportedOperation`, `smithy_query(query="operation", operation="SendMessage",
//!   model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`),
//!   and does not disclose a queue it will not let the caller write to, so it answers
//!   `QueueDoesNotExist` — HTTP 400 under the `awsQueryError` code
//!   `AWS.SimpleQueueService.NonExistentQueue` (`smithy_query(query="shape",
//!   shape="QueueDoesNotExist", model_path="…")`) — so the class read off it is the
//!   RESOURCE one.
//!
//! The class is therefore NOT a reading of whether the queue is there: that the queue
//! existed all along is something a client sees by repairing the policy and watching the
//! next event arrive on the same queue url, which the second test here does.
//!
//! The rows differ along what the rule generalises over, and the recorded case that
//! motivated this suite is one of them: the ORIGIN of the event (a CloudTrail-derived
//! management call of another service, published onto the account's default bus by
//! CloudTrail's own fan-out, versus an event a client published with `PutEvents`), the BUS
//! that carried it (the default one, a custom one), the FORM the withholding takes (no
//! policy at all, a grant whose `aws:SourceArn` condition names a DIFFERENT rule, an
//! explicit `Deny` beside a grant), what the target's `RetryPolicy` allows (none at all, no
//! retries, several) and whether a PERMITTED sibling target of the same rule took the same
//! event. A control row whose queue grants the send receives the event and parks no dead
//! letter, so no row passes because nothing was delivered anywhere.
//!
//! Only a cross-service suite can pin this: the management call is served by Elastic Load
//! Balancing (`awsQuery`) and recorded by CloudTrail (`awsJson1_1`), which decides the
//! fan-out; the matching, the send and the dead-letter write belong to EventBridge
//! (`awsJson1_1`); both queues and the document that withholds the send belong to Amazon
//! SQS (`awsJson1_0`); and the trail's destination bucket belongs to Amazon S3 — all on the
//! ONE engine `core/router/src/lib.rs` mounts.
//!
//! The suites for WHICH documents admit a send and what a refused delivery reports are
//! `service-com-amazonaws-eventbridge-awsevents`'s own
//! `tests/all/queue_target_policy_authorization.rs` and
//! `tests/all/target_refusal_class_follows_the_answering_service.rs`; the deployed-stack
//! pipeline is `tests/all/cloudformation_events_queue_target_dead_letter_boundary.rs`. This
//! one is the suite for the ORIGIN of the event making no difference to either.

use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// The Region every request below is signed for, and the one the trail logs in.
const REGION: &str = "us-west-2";

/// The service principal EventBridge hands a resource-policy-governed target over as.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// The action a delivery to a queue target performs on that queue.
const SEND_MESSAGE: &str = "sqs:SendMessage";

/// The `detail-type` CloudTrail's management-event fan-out carries
/// (`service_common::api_call_events::API_CALL_DETAIL_TYPE`).
const API_CALL_DETAIL_TYPE: &str = "AWS API Call via CloudTrail";

/// The `source` and `detail.eventSource` of the management call this suite makes:
/// `aws.<endpoint-prefix>` of the serving service, and the `cloudTrailEventSource` that
/// service's model publishes.
const BALANCING_SOURCE: &str = "aws.elasticloadbalancing";
const BALANCING_EVENT_SOURCE: &str = "elasticloadbalancing.amazonaws.com";

/// The management call itself — a write, so it reaches a plain `ENABLED` rule.
const MANAGEMENT_CALL: &str = "CreateTargetGroup";

/// The API versions the two `awsQuery` services this suite calls are addressed under.
const BALANCING_VERSION: &str = "2015-12-01";
const EC2_VERSION: &str = "2016-11-15";

/// The target group each management call creates: an `instance` target group, which is
/// addressed inside a VPC and receives traffic on a port, so both are stated.
const TARGET_GROUP_PROTOCOL: &str = "HTTP";
const TARGET_GROUP_PORT: &str = "80";
const TARGET_GROUP_TYPE: &str = "instance";
const VPC_CIDR: &str = "10.0.0.0/16";

/// The `source` and `detail-type` of the events a row publishes itself, and the name of
/// the bus a row that uses one creates.
const PUBLISHED_SOURCE: &str = "sim2real.events.target-refusal-origin";
const PUBLISHED_DETAIL_TYPE: &str = "Queue Target Origin Probe";
const CUSTOM_BUS: &str = "target-refusal-origin-bus";

/// The bus CloudTrail's fan-out publishes onto, which is the only one it publishes onto.
const DEFAULT_BUS: &str = "default";

/// The dead-letter message attributes "EventBridge adds … to the dead-letter message".
const RULE_ARN_ATTRIBUTE: &str = "RULE_ARN";
const TARGET_ARN_ATTRIBUTE: &str = "TARGET_ARN";
const ERROR_CODE_ATTRIBUTE: &str = "ERROR_CODE";
const ERROR_MESSAGE_ATTRIBUTE: &str = "ERROR_MESSAGE";

/// EventBridge's class for a delivery answered under a target service's not-found code,
/// which is what Amazon SQS answers a send it will not disclose the queue for.
const NO_RESOURCE: &str = "NO_RESOURCE";

/// The fixed sentence Amazon SQS answers a send the documents in force over the queue do
/// not permit — RECORDED, and written out here rather than read from any implementation,
/// so a wrong answer cannot make this suite pass. It states both the absence and the want
/// of access without saying which holds, and carries nothing of the request.
const QUEUE_NOT_DISCLOSED_SENTENCE: &str =
    "The specified queue does not exist or you do not have access to it.";

/// One answering service's refusal, as the dead letter's `ERROR_MESSAGE` carries it: the
/// error code that answer quotes and the status beside it.
struct AnswerClass {
    code: &'static str,
    status: u16,
}

/// The two answers a refused delivery to a queue target can read.
///
/// `AWS.SimpleQueueService.NonExistentQueue` is `QueueDoesNotExist`'s `awsQueryError` code
/// and `AccessDenied` the code an authorization refusal is answered under; a dead letter
/// has to quote exactly one of them, which is what makes an answer naming two a failure of
/// this suite rather than a detail it ignores.
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

/// How long the arrival of what a row expects is waited for, and how long between two
/// looks. Generous: each wait is on the CONDITION (the message is there), and the deadline
/// is only the point at which waiting longer cannot help.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(60);
const LOOK_INTERVAL: Duration = Duration::from_millis(25);

/// How many looks a queue that must stay EMPTY is given before it is believed.
const QUIET_LOOKS: usize = 4;

/// Where the event a row's rule matches comes from.
#[derive(Debug, Clone, Copy)]
enum Origin {
    /// A management call of ANOTHER service, which CloudTrail publishes onto the account's
    /// default bus while a trail of the Region is logging.
    ManagementCall,
    /// An event a client published itself, onto the named bus.
    Published(&'static str),
}

/// How the row's target queue withholds the send — or grants it, for the control row.
#[derive(Debug, Clone, Copy)]
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

/// One row: where its event comes from, what its target queue's document does with the
/// send, what its `RetryPolicy` allows, and whether a PERMITTED sibling target of the same
/// rule takes the same event.
struct Row {
    case: &'static str,
    origin: Origin,
    document: Document,
    /// `MaximumRetryAttempts`, or `None` for a target carrying no `RetryPolicy` at all.
    retry_attempts: Option<i32>,
    sibling: bool,
}

/// The rows, in the order they run. The first is the recorded case this suite was written
/// for; the rest vary one axis of it at a time.
const ROWS: [Row; 5] = [
    Row {
        case: "a CloudTrail-derived management event, a queue with no policy, no retries, \
               beside a permitted sibling",
        origin: Origin::ManagementCall,
        document: Document::None,
        retry_attempts: Some(0),
        sibling: true,
    },
    Row {
        case: "a CloudTrail-derived management event, a grant scoped to another rule, \
               retries allowed, no sibling",
        origin: Origin::ManagementCall,
        document: Document::ScopedToAnotherRule,
        retry_attempts: Some(3),
        sibling: false,
    },
    Row {
        case: "a client-published event on the default bus, a policy denying the send \
               beside granting it, no retry policy at all",
        origin: Origin::Published(DEFAULT_BUS),
        document: Document::DenyBesideGrant,
        retry_attempts: None,
        sibling: true,
    },
    Row {
        case: "a client-published event on a custom bus, a queue with no policy",
        origin: Origin::Published(CUSTOM_BUS),
        document: Document::None,
        retry_attempts: Some(0),
        sibling: false,
    },
    Row {
        case: "a CloudTrail-derived management event whose target queue grants the send",
        origin: Origin::ManagementCall,
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

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes
/// the request. The signature itself is never checked here.
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

/// A CloudTrail call (`awsJson1_1`, `CloudTrail_20131101.<Operation>`) that must answer
/// 200.
async fn cloudtrail(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) {
    let (status, parsed) =
        rpc(router, "cloudtrail", "CloudTrail_20131101", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudTrail {operation}, got {status} with {parsed}"
    );
}

/// One form body: the action, the version and the parameters.
fn form(action: &str, version: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    body
}

/// One form-encoded call of the service `signing_name` addresses, which must answer 200.
async fn query(
    router: &MultiServiceRouter,
    case: &str,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(form(action, version, params)))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {signing_name} {action}, got {status} with {body}"
    );
    body
}

/// The bucket a trail delivers its log files into, with the policy that lets CloudTrail
/// write into it.
///
/// The create states the Region as its `LocationConstraint`, which a bucket outside the
/// partition's leader Region is created with.
async fn given_bucket(router: &MultiServiceRouter, case: &str, bucket: &str) {
    let put = |uri: String, content_type: &'static str, body: Bytes| {
        http::Request::builder()
            .method("PUT")
            .uri(uri)
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("content-type", content_type)
            .header("authorization", authorization("s3"))
            .body(body)
            .expect("the test request is well formed")
    };
    let configuration = format!(
        "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <LocationConstraint>{REGION}</LocationConstraint>\
         </CreateBucketConfiguration>"
    );
    let response = router
        .dispatch(put(
            format!("/{bucket}"),
            "application/xml",
            Bytes::from(configuration),
        ))
        .await;
    assert!(
        (200..300).contains(&response.status().as_u16()),
        "case {case:?}: CreateBucket({bucket:?}) must succeed, got {} with {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AWSCloudTrailWrite",
            "Effect": "Allow",
            "Principal": { "Service": "cloudtrail.amazonaws.com" },
            "Action": "s3:PutObject",
            "Resource": format!("arn:aws:s3:::{bucket}/*"),
            "Condition": {
                "StringEquals": { "s3:x-amz-acl": "bucket-owner-full-control" },
            },
        }],
    })
    .to_string();
    let response = router
        .dispatch(put(
            format!("/{bucket}?policy"),
            "application/json",
            Bytes::from(policy),
        ))
        .await;
    assert!(
        (200..300).contains(&response.status().as_u16()),
        "case {case:?}: PutBucketPolicy({bucket:?}) must succeed, got {} with {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
}

/// A trail of the Region, logging — the condition CloudTrail's fan-out hangs off.
async fn given_logging_trail(router: &MultiServiceRouter, case: &str, name: &str) {
    given_bucket(router, case, &format!("{name}-logs")).await;
    cloudtrail(
        router,
        case,
        "CreateTrail",
        &json!({ "Name": name, "S3BucketName": format!("{name}-logs") }),
    )
    .await;
    cloudtrail(router, case, "StartLogging", &json!({ "Name": name })).await;
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
/// target names it by — read back from the queue rather than built here, which is also
/// what a document naming the queue is then written from ([`write_policy`]).
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
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Everything `url` holds over [`QUIET_LOOKS`] looks — for a queue that must stay empty.
async fn quiet(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let mut taken: Vec<Value> = Vec::new();
    for _ in 0..QUIET_LOOKS {
        taken.extend(look(router, case, url).await);
        tokio::time::sleep(LOOK_INTERVAL).await;
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

/// The event a dead-lettered or delivered message carries, parsed.
fn envelope(case: &str, message: &Value) -> Value {
    let body = message["Body"].as_str().unwrap_or_default();
    serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("case {case:?}: a delivered body is JSON ({error}): {body}"))
}

/// The `EventPattern` a row's rule matches its own event with.
fn pattern(origin: Origin) -> Value {
    match origin {
        Origin::ManagementCall => json!({
            "source": [BALANCING_SOURCE],
            "detail-type": [API_CALL_DETAIL_TYPE],
            "detail": {
                "eventSource": [BALANCING_EVENT_SOURCE],
                "eventName": [MANAGEMENT_CALL],
            },
        }),
        Origin::Published(_) => json!({
            "source": [PUBLISHED_SOURCE],
            "detail-type": [PUBLISHED_DETAIL_TYPE],
        }),
    }
}

/// Install the row's rule on its bus with the targets it describes, and answer the rule's
/// own ARN — the one the dead letter has to name.
async fn given_rule(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    bus: &str,
    origin: Origin,
    targets: Value,
) -> String {
    let rule = events(
        router,
        case,
        "PutRule",
        &json!({
            "Name": name,
            "EventBusName": bus,
            "State": "ENABLED",
            "EventPattern": pattern(origin).to_string(),
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
        &json!({ "Rule": name, "EventBusName": bus, "Targets": targets }),
    )
    .await;
    assert_eq!(
        put["FailedEntryCount"],
        json!(0),
        "case {case:?}: every target must be installed, got {put}"
    );
    rule_arn
}

/// Make the row's event happen: either the management call CloudTrail fans out, or a
/// publication of the row's own event onto the row's bus. `token` is what the envelope is
/// recognised by — the target group's name for the call, the detail's token for the
/// publication — and `vpc` the VPC an `instance` target group is created in.
async fn make_the_event(
    router: &MultiServiceRouter,
    case: &str,
    origin: Origin,
    token: &str,
    vpc: &str,
) {
    match origin {
        Origin::ManagementCall => {
            query(
                router,
                case,
                "elasticloadbalancingv2",
                BALANCING_VERSION,
                MANAGEMENT_CALL,
                &[
                    ("Name", token),
                    ("Protocol", TARGET_GROUP_PROTOCOL),
                    ("Port", TARGET_GROUP_PORT),
                    ("TargetType", TARGET_GROUP_TYPE),
                    ("VpcId", vpc),
                ],
            )
            .await;
        }
        Origin::Published(bus) => {
            let published = events(
                router,
                case,
                "PutEvents",
                &json!({
                    "Entries": [{
                        "EventBusName": bus,
                        "Source": PUBLISHED_SOURCE,
                        "DetailType": PUBLISHED_DETAIL_TYPE,
                        "Detail": json!({ "token": token }).to_string(),
                    }],
                }),
            )
            .await;
            assert_eq!(
                published["FailedEntryCount"],
                json!(0),
                "case {case:?}: the entry must be accepted, got {published}"
            );
        }
    }
}

/// Assert the envelope that reached a sink is the row's own event: the `source` and
/// `detail-type` it was published with, and the call or token it carries.
fn assert_is_the_event(case: &str, message: &Value, origin: Origin, token: &str) {
    let carried = envelope(case, message);
    match origin {
        Origin::ManagementCall => {
            assert_eq!(
                (carried["source"].clone(), carried["detail-type"].clone()),
                (json!(BALANCING_SOURCE), json!(API_CALL_DETAIL_TYPE)),
                "case {case:?}: expected the management call's own envelope, got {carried}"
            );
            assert_eq!(
                carried.pointer("/detail/eventName"),
                Some(&json!(MANAGEMENT_CALL)),
                "case {case:?}: expected the record of {MANAGEMENT_CALL}, got {carried}"
            );
        }
        Origin::Published(_) => {
            assert_eq!(
                (carried["source"].clone(), carried["detail-type"].clone()),
                (json!(PUBLISHED_SOURCE), json!(PUBLISHED_DETAIL_TYPE)),
                "case {case:?}: expected the published envelope, got {carried}"
            );
            assert_eq!(
                carried.pointer("/detail/token"),
                Some(&json!(token)),
                "case {case:?}: expected the published detail, got {carried}"
            );
        }
    }
}

/// Assert the dead letter's two attributes name ONE failure: the answer Amazon SQS gives a
/// send the documents in force over the queue do not permit, and the class that answer's own
/// code belongs to.
///
/// The pair is read the way a consumer reads it: the error code quoted in `ERROR_MESSAGE`
/// has to be one answer of [`ANSWER_CLASSES`], with the status that answer carries, and
/// `ERROR_CODE` has to be the class of that code. So a
/// letter whose halves belong to different failures fails here however plausible
/// each half is on its own.
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
    // A send the queue's own documents withheld reads that service's own fixed sentence under
    // its not-found code, so the class beside it is the resource one.
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

/// A VPC of the account, created through Amazon EC2 itself, so the `instance` target group
/// each management call creates is addressed inside one that really exists.
async fn given_vpc(router: &MultiServiceRouter, case: &str) -> String {
    let created = query(
        router,
        case,
        "ec2",
        EC2_VERSION,
        "CreateVpc",
        &[("CidrBlock", VPC_CIDR)],
    )
    .await;
    element(&created, "vpcId")
}

/// The class a withheld delivery is dead-lettered under follows the ANSWER the target's
/// service gave, and nothing about where the event came from.
#[tokio::test]
async fn a_withheld_queue_deliverys_class_does_not_depend_on_the_events_origin() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let router = router();
        let vpc = match row.origin {
            Origin::ManagementCall => {
                given_logging_trail(&router, case, &format!("origin-trail-{index}")).await;
                given_vpc(&router, case).await
            }
            Origin::Published(bus) => {
                if bus != DEFAULT_BUS {
                    events(&router, case, "CreateEventBus", &json!({ "Name": bus })).await;
                }
                String::new()
            }
        };

        // The rule a grant may be scoped to instead of this row's own, installed first so
        // its ARN is there to scope with. It carries no target, so it delivers nothing.
        let other_rule = if matches!(row.document, Document::ScopedToAnotherRule) {
            let installed = events(
                &router,
                case,
                "PutRule",
                &json!({
                    "Name": format!("origin-other-rule-{index}"),
                    "State": "ENABLED",
                    "EventPattern": json!({ "source": ["sim2real.events.other"] }).to_string(),
                }),
            )
            .await;
            installed["RuleArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a RuleArn, got {installed}"))
                .to_string()
        } else {
            String::new()
        };

        // Every queue before any rule of this row, so no queue this setup creates is a
        // management write the row's own rule could already have matched.
        let (dead_letter_url, dead_letter_arn) =
            given_granting_queue(&router, case, &format!("origin-dlq-{index}")).await;
        let (target_url, target_arn) =
            given_queue(&router, case, &format!("origin-target-{index}")).await;
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
            Some(given_granting_queue(&router, case, &format!("origin-sibling-{index}")).await)
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
        let bus = match row.origin {
            Origin::ManagementCall => DEFAULT_BUS,
            Origin::Published(bus) => bus,
        };
        let rule_arn = given_rule(
            &router,
            case,
            &format!("origin-rule-{index}"),
            bus,
            row.origin,
            json!(targets),
        )
        .await;

        let token = format!("origin-event-{index}");
        make_the_event(&router, case, row.origin, &token, &vpc).await;

        if let Some((sibling_url, _)) = &sibling {
            let delivered = held(&router, case, sibling_url, 1).await;
            assert_eq!(
                delivered.len(),
                1,
                "case {case:?}: expected the permitted sibling to receive the event, got \
                 {delivered:?}"
            );
            assert_is_the_event(case, &delivered[0], row.origin, &token);
        }

        if matches!(row.document, Document::Grants) {
            // The control row: a granted queue takes the event, and nothing is dead-lettered.
            let delivered = held(&router, case, &target_url, 1).await;
            assert_is_the_event(case, &delivered[0], row.origin, &token);
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
        assert_is_the_event(case, letter, row.origin, &token);
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

/// The class says nothing about whether the queue is THERE: the same queue url takes the
/// next matching event as soon as its policy grants the send, with the rule, the target and
/// the queue otherwise untouched.
#[tokio::test]
async fn a_granted_policy_delivers_the_next_management_event_to_the_same_queue() {
    let case = "a dead-lettered management event redelivered once the policy grants the send";
    let router = router();
    given_logging_trail(&router, case, "origin-repair-trail").await;
    let vpc = given_vpc(&router, case).await;

    let (dead_letter_url, dead_letter_arn) =
        given_granting_queue(&router, case, "origin-repair-dlq").await;
    let (target_url, target_arn) = given_queue(&router, case, "origin-repair-target").await;
    let rule_arn = given_rule(
        &router,
        case,
        "origin-repair-rule",
        DEFAULT_BUS,
        Origin::ManagementCall,
        json!([{
            "Id": "withheld",
            "Arn": target_arn,
            "DeadLetterConfig": { "Arn": dead_letter_arn },
            "RetryPolicy": { "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 },
        }]),
    )
    .await;

    let withheld = "origin-repair-first";
    make_the_event(&router, case, Origin::ManagementCall, withheld, &vpc).await;
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
    assert_withheld_class(case, &dead_lettered[0]);
    let leaked = quiet(&router, case, &target_url).await;
    assert!(
        leaked.is_empty(),
        "case {case:?}: a withheld delivery leaves the target empty, got {leaked:?}"
    );

    // The one thing that changes: the queue's own document now grants the send.
    write_policy(&router, case, &target_url, &granting(&target_arn)).await;
    let repaired = "origin-repair-second";
    make_the_event(&router, case, Origin::ManagementCall, repaired, &vpc).await;
    let delivered = held(&router, case, &target_url, 1).await;
    assert_eq!(
        delivered.len(),
        1,
        "case {case:?}: expected the granted queue to receive the next event, got {delivered:?}"
    );
    assert_is_the_event(case, &delivered[0], Origin::ManagementCall, repaired);
    let dead_lettered = quiet(&router, case, &dead_letter_url).await;
    assert!(
        dead_lettered.is_empty(),
        "case {case:?}: a delivery that went through is not dead-lettered, got {dead_lettered:?}"
    );
}
