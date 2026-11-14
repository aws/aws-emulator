// SPDX-License-Identifier: CC0-1.0

//! What a RUNNING function's own `Publish` is decided by, and where the notification it
//! published then arrives: Lambda, Amazon SNS, Amazon SQS, IAM and the token service on ONE
//! engine, a `python3.x` function whose module-scope code does `import boto3` and builds an
//! Amazon SNS client, and every publication read back off the queues the topic fans out to.
//!
//! RULE: a call an AWS SDK makes from inside a running function is made as a session of that
//! function's execution role, so Amazon SNS decides `sns:Publish` against the policies in
//! force on that role — a statement covering BOTH the action and the topic the call names
//! admits it, and the call answers HTTP 200 with the `MessageId` the service minted, while a
//! role whose statements cover the topic but not this action is refused `AuthorizationError`
//! (the `awsQueryError` code of `AuthorizationErrorException`). A publication Amazon SNS
//! accepted is then handed to EVERY subscription of that topic whose endpoint's own policy
//! admits this service for the topic published to, and an `sqs`-protocol subscriber receives a
//! `Notification` envelope whose `Message` member is the published `Message` byte for byte,
//! whatever text it carries, beside the `MessageId` and `TopicArn` of the publication.
//!
//! A refusal the handler catches is the handler's own business: the invocation is still
//! answered HTTP 200 with no `FunctionError`, because the run returned normally.
//!
//! Five crates have to agree for that to hold — IAM holds the role and its inline document,
//! the token service mints the session the function's calls are signed with, Lambda runs the
//! handler and supplies it the SDK, Amazon SNS takes the decision and fans the notification
//! out, and Amazon SQS holds the queue policies that admit it — which is why the suite stands
//! up all five on one engine.
//!
//! What the neighbours pin and this one does not repeat: `handler_notification_sdk` drives the
//! SHAPE of every Amazon SNS call the SDK binds for a role granted `sns:*` on `*`, and
//! `handler_aws_sdk` the seam itself. What only this suite reaches is the GRANULARITY of the
//! decision — which action and which topic one statement covers — and the fan-out that follows
//! a publication the decision admitted.
//!
//! The rows differ along what the rule generalises over: the TEXT the publication carries (one
//! plain line, a JSON document, text carrying newlines and tabs, text outside ASCII, text far
//! longer than a line), WHICH topic the call names and how the role's statement covers it (the
//! topic named exactly, a topic covered by a resource pattern, a topic covered for another
//! action only), and — for the fan-out — how the subscribed queue's policy admits this service
//! (scoped by `aws:SourceArn` to the topic published to, unscoped, or scoped to a DIFFERENT
//! topic of the same account). Each row asserts both halves: what the SDK gave the handler,
//! and what the subscribed queues hold afterwards, so a publication that answered a MessageId
//! and delivered nothing, and a delivery to a queue whose policy withheld it, both fail here.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]) and makes exactly ONE
//! invocation: every row is a call that one run makes, because an invocation of its own costs
//! this target a share of the run budget every other invocation suite draws on.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoding, the XML element reader and the IAM call every suite in this
// target writes a role with, owned by the suite that first needed them rather than copied.
use crate::event_source_mapping_role_access::{element, form_encode};
use crate::event_source_mappings::{await_function_active, call, call_sqs, router_on};
// The one .zip writer this target packages a running handler with.
use crate::synchronous_invoke_run::package;

/// The Region every request below is signed for, and so the Region the topics and the queues
/// belong to.
const REGION: &str = "us-east-1";

/// The account every ARN below names, which is the account this emulator serves.
const ACCOUNT: &str = "000000000000";

/// Where `CreateFunction` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The execution role the function runs as, the function's own name, and the inline identity
/// policy the role carries its grants in.
const ROLE_NAME: &str = "lambda-fan-out-publisher";
const FUNCTION: &str = "fan-out-caller";
const INLINE_POLICY: &str = "publish-grants";

/// The topic the role's statement names EXACTLY, which is the topic the fan-out is read off.
const GRANTED_TOPIC: &str = "fan-out-granted";

/// A topic no statement names on its own: it is covered by a resource PATTERN, so a grant
/// written over a family of topics is what admits the publication to it.
const PATTERNED_TOPIC: &str = "fan-out-wild-orders";

/// The resource pattern that statement carries: every topic of this account and Region whose
/// name begins this way, which is how a least-privilege role is written for a family.
const TOPIC_PATTERN_PREFIX: &str = "fan-out-wild-";

/// A topic the role's documents DO cover — for another action of this same service — so a
/// refusal of a publication to it is about the action and not about the topic or its
/// existence.
const READABLE_TOPIC: &str = "fan-out-readable";

/// The three queues subscribed to [`GRANTED_TOPIC`], named for the policy each carries: one
/// scoped by `aws:SourceArn` to that topic, one granting this service the send with no
/// condition at all, and one scoped to ANOTHER topic of the account — which is also subscribed
/// to [`PATTERNED_TOPIC`], the topic its condition does name.
const SCOPED_QUEUE: &str = "fan-out-sink-scoped";
const UNSCOPED_QUEUE: &str = "fan-out-sink-unscoped";
const OTHER_SOURCE_QUEUE: &str = "fan-out-sink-other-source";

/// The AWS-managed policy a Lambda execution role is created with: it grants the log writes a
/// function makes and nothing else, so a role carrying only it has no grant on Amazon SNS.
const BASIC_EXECUTION_POLICY: &str =
    "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole";

/// The `Type` member every notification envelope declares.
const NOTIFICATION_TYPE: &str = "Notification";

/// The error code Amazon SNS refuses a call the policies in force do not permit with:
/// `AuthorizationErrorException`'s `awsQueryError` code.
const AUTHORIZATION_ERROR: &str = "AuthorizationError";

/// How long a round trip through another service is waited for: the condition is looked at
/// again until it holds or this budget is spent, so nothing here is timed.
const CLOCK_BUDGET: Duration = Duration::from_secs(60);

/// How long between two looks.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// Lambda, Amazon SNS, Amazon SQS, IAM and the token service over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's SDK call is dispatched back into this
/// same router, so the topic it publishes to is the topic this suite subscribed.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. Nothing here is signed: the suite's own calls present the account's own published
/// credential.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One awsQuery call to Amazon SNS that must be served, as the XML it carried.
async fn sns(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version=2010-03-31");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("sns"))
        .body(bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "setup: expected HTTP 200 from SNS {action}, got {status} / {text}"
    );
    text
}

/// One Amazon SQS call that must be served.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call_sqs(router, operation, body).await;
    assert_eq!(
        status, 200,
        "setup: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One awsQuery call to IAM that must be served.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) {
    let (status, text) = crate::event_source_mapping_role_access::iam(router, action, params).await;
    assert_eq!(
        status, 200,
        "setup: expected HTTP 200 from IAM {action}, got {status} / {text}"
    );
}

/// The ARN of a topic of this account in this Region.
fn topic_arn(name: &str) -> String {
    format!("arn:aws:sns:{REGION}:{ACCOUNT}:{name}")
}

/// The ARN of the execution role, as IAM records it.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{ROLE_NAME}")
}

/// The trust policy a Lambda execution role carries.
fn trust_document() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "lambda.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// The inline identity document the execution role carries: the publication grant written
/// three different ways, which is what makes the decision's granularity visible.
///
/// One statement names [`GRANTED_TOPIC`] exactly; one names every topic of the account and
/// Region matching [`TOPIC_PATTERN_PREFIX`]; and one covers [`READABLE_TOPIC`] for a READ of
/// the topic rather than for a publication.
fn grant_document() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": "sns:Publish",
                "Resource": topic_arn(GRANTED_TOPIC),
            },
            {
                "Effect": "Allow",
                "Action": "sns:Publish",
                "Resource": format!(
                    "arn:aws:sns:{REGION}:{ACCOUNT}:{TOPIC_PATTERN_PREFIX}*"
                ),
            },
            {
                "Effect": "Allow",
                "Action": "sns:GetTopicAttributes",
                "Resource": topic_arn(READABLE_TOPIC),
            },
        ],
    })
    .to_string()
}

/// The queue policy AWS documents for a queue subscribed to a topic: the service principal
/// `sns.amazonaws.com`, allowed `sqs:SendMessage` on that queue — scoped by `aws:SourceArn` to
/// `source` when one is named, and unconditioned when none is.
fn grant_to_sns(queue_arn: &str, source: Option<&str>) -> String {
    let mut statement = json!({
        "Sid": "AllowSNSDelivery",
        "Effect": "Allow",
        "Principal": {"Service": "sns.amazonaws.com"},
        "Action": "sqs:SendMessage",
        "Resource": queue_arn,
    });
    if let Some(source) = source {
        statement["Condition"] = json!({"ArnEquals": {"aws:SourceArn": source}});
    }
    json!({ "Version": "2012-10-17", "Statement": [statement] }).to_string()
}

/// A message far longer than one line, built rather than spelled so its bytes differ along it:
/// a truncation, a reordering or a re-encoding of it shows in the comparison. Well inside what
/// one publication may carry ("up to 256 KB", `PublishInput$Message`).
fn long_message() -> String {
    (0..256).map(|index| format!("{index:03}-segment ")).collect()
}

/// One publication the handler makes: the case, the topic it names, and the text it carries.
struct Published {
    case: &'static str,
    topic: String,
    message: String,
}

/// Every publication the role's documents admit, in the order one run makes them: the text
/// rows against the topic named exactly, then the row against the topic a resource PATTERN
/// covers.
fn admitted() -> Vec<Published> {
    let mut rows: Vec<Published> = [
        ("one plain line of text", "from-lambda-sdk".to_string()),
        (
            "a JSON document carried as text",
            json!({"orderId": "A-1", "items": [1, 2, 3]}).to_string(),
        ),
        (
            "text carrying newlines, tabs and runs of spacing",
            "first line\n\tsecond\tline   \n\nlast".to_string(),
        ),
        (
            "text outside ASCII",
            "présentation ☃ 日本語 — ok".to_string(),
        ),
        ("text far longer than a line", long_message()),
    ]
    .into_iter()
    .map(|(case, message)| Published {
        case,
        topic: topic_arn(GRANTED_TOPIC),
        message,
    })
    .collect();
    rows.push(Published {
        case: "a topic the role's statement covers by resource pattern",
        topic: topic_arn(PATTERNED_TOPIC),
        message: "patterned-grant".to_string(),
    });
    rows
}

/// The handler every invocation runs: it makes the Amazon SNS calls the event names, in order,
/// and reports what each one answered — so the invocation's payload IS what the SDK gave the
/// guest.
///
/// The client is built at MODULE scope, the way a deployment package holding only the handler
/// module builds one, so the credentials the runtime supplies have to be in force before the
/// handler itself runs.
const HANDLER_SOURCE: &str = r#"
import boto3
import botocore.exceptions

CLIENT = boto3.client("sns")


def lambda_handler(event, context):
    results = []
    for asked in event["calls"]:
        try:
            answered = getattr(CLIENT, asked["method"])(**asked["parameters"])
        except botocore.exceptions.ClientError as error:
            results.append(
                {
                    "ok": False,
                    "code": error.response["Error"]["Code"],
                    "message": error.response["Error"].get("Message"),
                }
            )
            continue
        results.append(
            {
                "ok": True,
                "status": answered["ResponseMetadata"]["HTTPStatusCode"],
                "answered": {
                    name: value
                    for name, value in answered.items()
                    if name != "ResponseMetadata"
                },
            }
        )
    return {"results": results}
"#;

/// Invoke the function with `event` and answer the payload, asserting the invocation itself
/// answered 200 with no function error — the handler reports a refusal in its own payload, so a
/// function error here is the crash the runtime must not produce.
async fn invoked(router: &MultiServiceRouter, case: &str, event: &Value) -> Value {
    let answered = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{FUNCTION}/invocations"),
        event,
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from Invoke, got {} ({})",
        answered.status,
        answered.text()
    );
    let signal = answered.header("x-amz-function-error");
    assert_eq!(
        signal, None,
        "case {case:?}: expected no FunctionError, got {signal:?} with {}",
        answered.text()
    );
    answered.json()
}

/// The notification envelopes the queue at `url` has been handed, keyed by the `MessageId` each
/// carries, once `expected` of them have arrived — looked at again until they have or the
/// budget is spent, so what is waited for is the CONDITION and not a span.
async fn envelopes(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    expected: usize,
) -> BTreeMap<String, Value> {
    let deadline = Instant::now() + CLOCK_BUDGET;
    let mut held: BTreeMap<String, Value> = BTreeMap::new();
    loop {
        let received = sqs(
            router,
            "ReceiveMessage",
            &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        for message in received["Messages"].as_array().cloned().unwrap_or_default() {
            let body = message["Body"].as_str().unwrap_or_default().to_string();
            let envelope: Value = serde_json::from_str(&body).unwrap_or_else(|error| {
                panic!("case {case:?}: expected a JSON envelope, got {error}: {body}")
            });
            let id = envelope["MessageId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a MessageId in the envelope, got {envelope}")
                })
                .to_string();
            held.insert(id, envelope);
        }
        if held.len() >= expected {
            return held;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {expected} notification(s) on the subscribed queue, got {} \
             ({:?})",
            held.len(),
            held.keys().collect::<Vec<_>>()
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Create a queue, grant this service the send on it the way `source` says, subscribe it to
/// each topic in `topics`, and answer its url.
async fn subscribed_queue(
    router: &MultiServiceRouter,
    name: &str,
    source: Option<&str>,
    topics: &[&str],
) -> String {
    let created = sqs(router, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no QueueUrl in {created}"))
        .to_string();
    let attributes = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no QueueArn in {attributes}"))
        .to_string();
    sqs(
        router,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "Attributes": { "Policy": grant_to_sns(&arn, source) },
        }),
    )
    .await;
    for topic in topics {
        let answered = sns(
            router,
            "Subscribe",
            &[
                ("TopicArn", topic),
                ("Protocol", "sqs"),
                ("Endpoint", &arn),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await;
        let subscription = element(&answered, "SubscriptionArn");
        assert!(
            subscription.starts_with(topic),
            "setup: expected the subscription ARN to extend {topic}, got {subscription}"
        );
    }
    url
}

/// RULE: a publication a running handler makes with the SDK its runtime supplies is decided
/// against the policies in force on the function's execution role — action and topic together —
/// and one the decision admits reaches every `sqs`-protocol subscriber whose queue policy
/// admits this service for that topic, carrying the published `Message` byte for byte.
#[tokio::test]
async fn a_granted_handlers_publication_fans_out_to_every_admitting_queue() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let router = router();

    // The role the function runs as, written with IAM's own operations, so the session the
    // runtime mints speaks for the record `iam:CreateRole` wrote.
    iam(
        &router,
        "CreateRole",
        &[
            ("RoleName", ROLE_NAME),
            ("AssumeRolePolicyDocument", &trust_document()),
        ],
    )
    .await;
    iam(
        &router,
        "AttachRolePolicy",
        &[
            ("RoleName", ROLE_NAME),
            ("PolicyArn", BASIC_EXECUTION_POLICY),
        ],
    )
    .await;
    iam(
        &router,
        "PutRolePolicy",
        &[
            ("RoleName", ROLE_NAME),
            ("PolicyName", INLINE_POLICY),
            ("PolicyDocument", &grant_document()),
        ],
    )
    .await;

    // The three topics, each created where a client creates one.
    for name in [GRANTED_TOPIC, PATTERNED_TOPIC, READABLE_TOPIC] {
        let created = sns(&router, "CreateTopic", &[("Name", name)]).await;
        let arn = element(&created, "TopicArn");
        assert_eq!(
            arn,
            topic_arn(name),
            "setup: expected the topic ARN of this account and Region, got {arn}"
        );
    }

    // The queues the fan-out is read off, each carrying the policy its name states.
    let granted = topic_arn(GRANTED_TOPIC);
    let patterned = topic_arn(PATTERNED_TOPIC);
    let scoped = subscribed_queue(&router, SCOPED_QUEUE, Some(&granted), &[&granted]).await;
    let unscoped = subscribed_queue(&router, UNSCOPED_QUEUE, None, &[&granted]).await;
    // Subscribed to BOTH topics, but admitting this service for the patterned one alone: the
    // same subscription path, decided per source topic.
    let other_source = subscribed_queue(
        &router,
        OTHER_SOURCE_QUEUE,
        Some(&patterned),
        &[&granted, &patterned],
    )
    .await;

    // The function itself, running as that role.
    let created = call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": FUNCTION,
            "Role": role_arn(),
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": 30,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "setup: expected HTTP 201 from CreateFunction, got {} ({})",
        created.status,
        created.text()
    );
    await_function_active(&router, "setup", FUNCTION).await;

    // ONE invocation makes every call: the admitted publications, the read the role IS granted
    // on the third topic, and the publication to that same topic the role is NOT granted.
    let rows = admitted();
    let mut calls: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "method": "publish",
                "parameters": { "TopicArn": row.topic, "Message": row.message },
            })
        })
        .collect();
    let read_case = "a read of a topic the role's statement covers for that action";
    calls.push(json!({
        "method": "get_topic_attributes",
        "parameters": { "TopicArn": topic_arn(READABLE_TOPIC) },
    }));
    let refused_case = "a publication to a topic the role's statements cover for another action";
    calls.push(json!({
        "method": "publish",
        "parameters": { "TopicArn": topic_arn(READABLE_TOPIC), "Message": "ungranted" },
    }));

    let payload = invoked(&router, "the handler's own calls", &json!({ "calls": calls })).await;
    let results = payload["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        results.len(),
        rows.len() + 2,
        "case \"the handler's own calls\": expected one result per call, got {payload}"
    );

    // Each admitted publication answered 200 with a MessageId of its own.
    let mut minted: Vec<String> = Vec::new();
    for (row, result) in rows.iter().zip(&results) {
        let case = row.case;
        assert_eq!(
            result["ok"],
            json!(true),
            "case {case:?}: expected the publication to be served, got {result}"
        );
        assert_eq!(
            result["status"],
            json!(200),
            "case {case:?}: expected the SDK to report HTTP 200, got {result}"
        );
        let id = result["answered"]["MessageId"].as_str().unwrap_or_default();
        assert!(
            !id.is_empty(),
            "case {case:?}: expected the service to mint a MessageId, got {result}"
        );
        assert!(
            !minted.iter().any(|seen| seen == id),
            "case {case:?}: expected a MessageId of its own, got the one already minted: {id}"
        );
        minted.push(id.to_string());
    }

    // The read the role IS granted on the third topic is served, so the refusal below is about
    // the ACTION and not about that topic or whether it exists.
    let read = &results[rows.len()];
    assert_eq!(
        read["ok"],
        json!(true),
        "case {read_case:?}: expected the read to be served, got {read}"
    );
    assert_eq!(
        read["answered"]["Attributes"]["TopicArn"],
        json!(topic_arn(READABLE_TOPIC)),
        "case {read_case:?}: expected the attributes of that topic, got {read}"
    );

    // And the publication to it is refused, with this service's own authorization code.
    let denied = &results[rows.len() + 1];
    assert_eq!(
        denied["ok"],
        json!(false),
        "case {refused_case:?}: expected the publication to be refused, got {denied}"
    );
    assert_eq!(
        denied["code"],
        json!(AUTHORIZATION_ERROR),
        "case {refused_case:?}: expected the service's own authorization refusal, got {denied}"
    );
    assert!(
        denied["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case {refused_case:?}: expected the service's own sentence beside the code, got {denied}"
    );

    // The other half: what the subscribed queues hold. The two queues whose policy admits this
    // service for the published topic were handed every publication to it; the message each
    // carries is the one published, byte for byte.
    let to_granted = rows.len() - 1;
    for (case, url) in [
        ("a queue policy scoped by aws:SourceArn to the published topic", &scoped),
        ("a queue policy granting the send with no condition", &unscoped),
    ] {
        let held = envelopes(&router, case, url, to_granted).await;
        assert_eq!(
            held.len(),
            to_granted,
            "case {case:?}: expected exactly the publications to that topic, got {:?}",
            held.keys().collect::<Vec<_>>()
        );
        for (row, id) in rows.iter().zip(&minted).take(to_granted) {
            let envelope = held.get(id).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the publication {:?} ({id}) on the queue, got {:?}",
                    row.case,
                    held.keys().collect::<Vec<_>>()
                )
            });
            assert_eq!(
                envelope["Type"],
                json!(NOTIFICATION_TYPE),
                "case {case:?}: expected a notification envelope for {:?}, got {envelope}",
                row.case
            );
            assert_eq!(
                envelope["TopicArn"],
                json!(row.topic),
                "case {case:?}: expected the envelope to name the published topic for {:?}, got \
                 {envelope}",
                row.case
            );
            assert_eq!(
                envelope["Message"].as_str(),
                Some(row.message.as_str()),
                "case {case:?}: expected the published message verbatim for {:?}, got {:?}",
                row.case,
                envelope["Message"]
            );
        }
    }

    // The queue whose policy admits this service for ANOTHER topic received the publication to
    // THAT topic and nothing of the one its condition does not name — the same subscription
    // path, decided per source topic.
    let case = "a queue policy scoped by aws:SourceArn to another topic of the account";
    let patterned_id = minted
        .last()
        .expect("every admitted row minted a MessageId")
        .clone();
    let held = envelopes(&router, case, &other_source, 1).await;
    assert_eq!(
        held.len(),
        1,
        "case {case:?}: expected the publication to the topic its condition names and no other, \
         got {:?}",
        held.keys().collect::<Vec<_>>()
    );
    let envelope = held.get(&patterned_id).unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected the publication to {patterned} ({patterned_id}), got {:?}",
            held.values().collect::<Vec<_>>()
        )
    });
    assert_eq!(
        envelope["TopicArn"],
        json!(patterned),
        "case {case:?}: expected the envelope to name the topic its condition admits, got \
         {envelope}"
    );
    assert_eq!(
        envelope["Message"].as_str(),
        Some(
            rows.last()
                .expect("the patterned row is the last admitted one")
                .message
                .as_str()
        ),
        "case {case:?}: expected the published message verbatim, got {:?}",
        envelope["Message"]
    );
}
