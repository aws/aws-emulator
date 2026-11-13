// SPDX-License-Identifier: CC0-1.0

//! `Publish` DELIVERS the message to the LAMBDA FUNCTIONS subscribed to the topic, end to end
//! over the wire (awsQuery for SNS, `restJson1` for Lambda) through the real
//! `MultiServiceRouter`, with Lambda mounted on the same engine so an invocation is observed
//! where that service holds it: on the function's asynchronous invocation queue.
//!
//! The RULE asserted here: "lambda – delivery of JSON-encoded message to an Lambda function"
//! (`SubscribeInput$Protocol`) and SNS "delivers the message to each endpoint that is
//! subscribed to the topic" (`Publish`) — so every confirmed `lambda` subscription of the
//! topic published to is invoked asynchronously with the message in the `Records` envelope a
//! function's event carries, and that invocation is made only where the FUNCTION's own
//! resource-based policy grants `lambda:InvokeFunction` to the service principal
//! `sns.amazonaws.com` for that topic. A function that has granted nothing is not invoked at
//! all, so a permissioned subscription and an unpermissioned one differ in what the client can
//! observe.
//!
//! The rows differ along what the rule generalises over: which document the function carries
//! (none, a grant naming no source, a grant scoped to the topic published to, a grant scoped
//! to another topic, a grant of another action, a grant to another principal), the form of the
//! published message (plain, with a subject and attributes, published as a structure of
//! per-protocol branches), and how many endpoints — functions and queues — are subscribed to
//! the one topic.
//!
//! What a delivered run then COMPUTES is the Lambda service's own concern and is asserted
//! where a function's package is built; what is pinned here is the event SNS hands it, which
//! is the half this service owns.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use smithy_server_runtime::MultiServiceRouter;

use crate::sqs_subscription_delivery::element;
use crate::topics_subscriptions_publish::percent_encode;

/// The Region every call below is signed for, and so the Region every ARN here names.
const REGION: &str = "us-east-1";

/// The subscription protocol whose endpoint is a function.
pub(crate) const LAMBDA_PROTOCOL: &str = "lambda";

/// The IAM action an invocation of a function performs, which a grant has to name.
pub(crate) const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The service principal SNS performs a delivery as.
pub(crate) const SNS_PRINCIPAL: &str = "sns.amazonaws.com";

/// A well-formed Lambda execution role ARN, satisfying that model's `RoleArn` pattern.
pub(crate) const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// A deployment package's bytes, base64-encoded as `FunctionCode$ZipFile` carries them.
/// Nothing here runs it: what this suite asserts is the event SNS handed the function.
pub(crate) const ZIP_FILE: &str = "UEsDBAoAAAAAAAAAAAA=";

/// How long a wait for an invocation lasts before giving up. An invocation is made as soon as
/// the publish that owes it is answered, so this is many deliveries' worth of slack and not a
/// span the behaviour depends on.
pub(crate) const DELIVERY_BUDGET: Duration = Duration::from_secs(10);

/// How long a wait lasts before concluding that NOTHING was invoked.
const QUIET_BUDGET: Duration = Duration::from_millis(1_500);

/// How often a wait looks.
pub(crate) const LOOK_INTERVAL: Duration = Duration::from_millis(25);

/// SNS, Lambda and Amazon SQS over ONE shared engine: a subscription's endpoint is a function,
/// and the invocation is asserted on the function.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

pub(crate) fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    Emulator { router, engine }
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// the request. Neither the key id nor the signature is a credential: this harness turns
/// request authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One awsQuery form call against SNS that must SUCCEED, as its body text.
pub(crate) async fn sns(emulator: &Emulator, action: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version=2010-03-31");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&percent_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("sns"))
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

/// One `restJson1` call against Lambda, with whatever status it answered.
pub(crate) async fn lambda(
    emulator: &Emulator,
    method: &str,
    uri: &str,
    body: Value,
) -> (u16, Value) {
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
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One `awsJson1_0` call against SQS that must SUCCEED.
pub(crate) async fn sqs(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization("sqs"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "{operation}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// A function called `name`, brought to `Active` — the state an invocation needs — by moving
/// the activation instant the store holds into the past, which is what waiting would do. Its
/// ARN, which is what a subscription names it by.
pub(crate) async fn given_function(emulator: &Emulator, name: &str) -> String {
    let (status, created) = lambda(
        emulator,
        "POST",
        "/2015-03-31/functions",
        json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": ZIP_FILE },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateFunction {name}: expected HTTP 201, got {status} with {created}"
    );
    let arn = created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateFunction {name}: no FunctionArn in {created}"))
        .to_string();
    {
        let state = emulator
            .engine
            .state::<ComAmazonawsLambdaAwsgirapiserviceState>();
        let mut store = state.lock();
        let past = SystemTime::now() - Duration::from_secs(1);
        for record in store.functions.values_mut() {
            record.active_due = past;
        }
    }
    arn
}

/// Add one statement to the function's resource-based policy, through the call a client (or a
/// CloudFormation `AWS::Lambda::Permission`) adds it with.
pub(crate) async fn grant(
    emulator: &Emulator,
    function: &str,
    statement_id: &str,
    action: &str,
    principal: &str,
    source_arn: Option<&str>,
) {
    let mut body = json!({
        "StatementId": statement_id,
        "Action": action,
        "Principal": principal,
    });
    if let Some(source_arn) = source_arn {
        body["SourceArn"] = Value::from(source_arn);
    }
    let (status, added) = lambda(
        emulator,
        "POST",
        &format!("/2015-03-31/functions/{function}/policy"),
        body,
    )
    .await;
    assert_eq!(
        status, 201,
        "AddPermission {function}: expected HTTP 201, got {status} with {added}"
    );
}

/// Subscribe `endpoint` to `topic` over `protocol`, and report the subscription's ARN.
pub(crate) async fn subscribe(
    emulator: &Emulator,
    topic: &str,
    protocol: &str,
    endpoint: &str,
) -> String {
    let subscribed = sns(
        emulator,
        "Subscribe",
        &[
            ("TopicArn", topic),
            ("Protocol", protocol),
            ("Endpoint", endpoint),
            ("ReturnSubscriptionArn", "true"),
        ],
    )
    .await;
    element(&subscribed, "SubscriptionArn")
}

/// Publish `message` to `topic`, and report the `MessageId` the publish answered with.
pub(crate) async fn publish(emulator: &Emulator, topic: &str, params: &[(&str, &str)]) -> String {
    let mut all: Vec<(&str, &str)> = vec![("TopicArn", topic)];
    all.extend_from_slice(params);
    let published = sns(emulator, "Publish", &all).await;
    element(&published, "MessageId")
}

/// Every event Lambda holds for `function`, oldest first.
fn events_of(emulator: &Emulator, function: &str) -> Vec<Value> {
    let state = emulator
        .engine
        .state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(function)
        .map(|record| {
            record
                .async_invocations
                .iter()
                .map(|entry| {
                    serde_json::from_slice(&entry.payload).unwrap_or_else(|error| {
                        panic!(
                            "the event handed to {function} must be a JSON document, got \
                             {error} for {:?}",
                            String::from_utf8_lossy(&entry.payload)
                        )
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Wait until Lambda holds `expected` events for `function`, and report them.
pub(crate) async fn invoked(emulator: &Emulator, function: &str, expected: usize) -> Vec<Value> {
    let deadline = Instant::now() + DELIVERY_BUDGET;
    loop {
        let held = events_of(emulator, function);
        if held.len() >= expected {
            return held;
        }
        if Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Wait out [`QUIET_BUDGET`] and report every event Lambda holds for `function` — nothing, for
/// a function nothing was allowed to invoke.
pub(crate) async fn quiet(emulator: &Emulator, function: &str) -> Vec<Value> {
    let deadline = Instant::now() + QUIET_BUDGET;
    while Instant::now() < deadline {
        let held = events_of(emulator, function);
        if !held.is_empty() {
            return held;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    events_of(emulator, function)
}

/// One row of the permission table: the statement the function carries, and whether a publish
/// to the subscribed topic must then invoke it.
struct Permission {
    case: &'static str,
    /// The action the statement grants, or `None` for a function carrying no policy at all.
    action: Option<&'static str>,
    /// The principal it grants to.
    principal: &'static str,
    /// Whether the statement is scoped to the topic published to, to another topic, or to no
    /// source at all.
    source: Source,
    invoked: bool,
}

/// Which source a grant is scoped to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    /// No `SourceArn`: every source of the principal is granted.
    Any,
    /// The topic the message is published to.
    SubscribedTopic,
    /// Another topic of the same account.
    OtherTopic,
}

fn permissions() -> Vec<Permission> {
    vec![
        Permission {
            case: "a grant to this service naming no source",
            action: Some(INVOKE_ACTION),
            principal: SNS_PRINCIPAL,
            source: Source::Any,
            invoked: true,
        },
        Permission {
            case: "a grant scoped to the topic published to",
            action: Some(INVOKE_ACTION),
            principal: SNS_PRINCIPAL,
            source: Source::SubscribedTopic,
            invoked: true,
        },
        Permission {
            case: "no policy on the function at all",
            action: None,
            principal: SNS_PRINCIPAL,
            source: Source::Any,
            invoked: false,
        },
        Permission {
            case: "a grant scoped to another topic",
            action: Some(INVOKE_ACTION),
            principal: SNS_PRINCIPAL,
            source: Source::OtherTopic,
            invoked: false,
        },
        Permission {
            case: "a grant of another action",
            action: Some("lambda:GetFunction"),
            principal: SNS_PRINCIPAL,
            source: Source::Any,
            invoked: false,
        },
        Permission {
            case: "a grant to another service principal",
            action: Some(INVOKE_ACTION),
            principal: "events.amazonaws.com",
            source: Source::Any,
            invoked: false,
        },
    ]
}

/// Every row: a publish invokes the subscribed function exactly where that function's own
/// resource-based policy grants this service the invocation, and not otherwise.
#[tokio::test]
async fn a_publish_invokes_a_subscribed_function_only_where_its_policy_grants_it() {
    for (index, row) in permissions().iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let topic = element(
            &sns(&emulator, "CreateTopic", &[("Name", &format!("grants-{index}"))]).await,
            "TopicArn",
        );
        let other_topic = element(
            &sns(&emulator, "CreateTopic", &[("Name", &format!("other-{index}"))]).await,
            "TopicArn",
        );
        let function = format!("grant-probe-{index}");
        let function_arn = given_function(&emulator, &function).await;
        if let Some(action) = row.action {
            let source = match row.source {
                Source::Any => None,
                Source::SubscribedTopic => Some(topic.as_str()),
                Source::OtherTopic => Some(other_topic.as_str()),
            };
            grant(
                &emulator,
                &function,
                "sns-invoke",
                action,
                row.principal,
                source,
            )
            .await;
        }
        let subscription = subscribe(&emulator, &topic, LAMBDA_PROTOCOL, &function_arn).await;
        let message_id = publish(&emulator, &topic, &[("Message", "a marker body")]).await;

        if row.invoked {
            let events = invoked(&emulator, &function, 1).await;
            assert_eq!(
                events.len(),
                1,
                "case {case:?}: expected exactly one invocation of the subscribed function, \
                 got {events:?}"
            );
            let record = &events[0]["Records"][0];
            assert_eq!(
                record["Sns"]["Message"].as_str(),
                Some("a marker body"),
                "case {case:?}: expected the published message in the event, got {:?}",
                events[0]
            );
            assert_eq!(
                record["Sns"]["MessageId"].as_str(),
                Some(message_id.as_str()),
                "case {case:?}: expected the publish's own MessageId {message_id:?}, got {:?}",
                events[0]
            );
            assert_eq!(
                record["EventSubscriptionArn"].as_str(),
                Some(subscription.as_str()),
                "case {case:?}: expected the receiving subscription {subscription:?}, got {:?}",
                events[0]
            );
        } else {
            let events = quiet(&emulator, &function).await;
            assert!(
                events.is_empty(),
                "case {case:?}: expected the function never to be invoked, got {events:?}"
            );
        }
    }
}

/// One row of the message table: what is published, and the body the event must carry.
struct Message {
    case: &'static str,
    /// The `Publish` parameters beside the topic.
    params: Vec<(&'static str, &'static str)>,
    /// The `Sns.Message` the function's event must carry.
    body: &'static str,
    /// The `Sns.Subject` it must carry, if any.
    subject: Option<&'static str>,
    /// The attributes it must carry, as `(name, type, value)`.
    attributes: Vec<(&'static str, &'static str, &'static str)>,
}

fn messages() -> Vec<Message> {
    vec![
        Message {
            case: "a plain message",
            params: vec![("Message", "plain body")],
            body: "plain body",
            subject: None,
            attributes: Vec::new(),
        },
        Message {
            case: "a message with a subject",
            params: vec![("Message", "subject body"), ("Subject", "a subject line")],
            body: "subject body",
            subject: Some("a subject line"),
            attributes: Vec::new(),
        },
        Message {
            case: "a message carrying an attribute",
            params: vec![
                ("Message", "attributed body"),
                ("MessageAttributes.entry.1.Name", "kind"),
                ("MessageAttributes.entry.1.Value.DataType", "String"),
                ("MessageAttributes.entry.1.Value.StringValue", "probe"),
            ],
            body: "attributed body",
            subject: None,
            attributes: vec![("kind", "String", "probe")],
        },
        Message {
            // A per-protocol structure: "You can define other top-level keys that define the
            // message you want to send to a specific transport protocol"
            // (`PublishInput$MessageStructure`), so the `lambda` branch is what a subscribed
            // function receives.
            case: "a structured message naming a lambda branch",
            params: vec![
                (
                    "Message",
                    "{\"default\":\"the default branch\",\"lambda\":\"the lambda branch\"}",
                ),
                ("MessageStructure", "json"),
            ],
            body: "the lambda branch",
            subject: None,
            attributes: Vec::new(),
        },
        Message {
            case: "a structured message naming only a default branch",
            params: vec![
                ("Message", "{\"default\":\"the default branch\"}"),
                ("MessageStructure", "json"),
            ],
            body: "the default branch",
            subject: None,
            attributes: Vec::new(),
        },
    ]
}

/// Every row: the event a subscribed function is handed is the `Records` envelope around the
/// notification of the message published, in the form its own protocol receives it.
#[tokio::test]
async fn the_event_a_subscribed_function_receives_is_the_records_envelope() {
    for (index, row) in messages().iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let topic = element(
            &sns(
                &emulator,
                "CreateTopic",
                &[("Name", &format!("envelope-{index}"))],
            )
            .await,
            "TopicArn",
        );
        let function = format!("envelope-probe-{index}");
        let function_arn = given_function(&emulator, &function).await;
        grant(
            &emulator,
            &function,
            "sns-invoke",
            INVOKE_ACTION,
            SNS_PRINCIPAL,
            Some(&topic),
        )
        .await;
        let subscription = subscribe(&emulator, &topic, LAMBDA_PROTOCOL, &function_arn).await;
        let message_id = publish(&emulator, &topic, &row.params).await;

        let events = invoked(&emulator, &function, 1).await;
        assert_eq!(
            events.len(),
            1,
            "case {case:?}: expected exactly one invocation, got {events:?}"
        );
        let event = &events[0];
        let records = event["Records"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            records.len(),
            1,
            "case {case:?}: expected one record per published message, got {event}"
        );
        let record = &records[0];
        assert_eq!(
            record["EventSource"].as_str(),
            Some("aws:sns"),
            "case {case:?}: expected the record to name this service as its source, got {event}"
        );
        assert_eq!(
            record["EventVersion"].as_str(),
            Some("1.0"),
            "case {case:?}: expected the record's schema version, got {event}"
        );
        assert_eq!(
            record["EventSubscriptionArn"].as_str(),
            Some(subscription.as_str()),
            "case {case:?}: expected the receiving subscription {subscription:?}, got {event}"
        );
        let notification = &record["Sns"];
        assert_eq!(
            notification["Type"].as_str(),
            Some("Notification"),
            "case {case:?}: expected a Notification, got {event}"
        );
        assert_eq!(
            notification["MessageId"].as_str(),
            Some(message_id.as_str()),
            "case {case:?}: expected the publish's own MessageId {message_id:?}, got {event}"
        );
        assert_eq!(
            notification["TopicArn"].as_str(),
            Some(topic.as_str()),
            "case {case:?}: expected the topic published to, got {event}"
        );
        assert_eq!(
            notification["Message"].as_str(),
            Some(row.body),
            "case {case:?}: expected the body {:?}, got {event}",
            row.body
        );
        assert_eq!(
            notification["Subject"].as_str(),
            row.subject,
            "case {case:?}: expected the subject {:?}, got {event}",
            row.subject
        );
        assert!(
            notification["Timestamp"].is_string(),
            "case {case:?}: expected the instant the message was published, got {event}"
        );
        // The link that cancels THE SUBSCRIPTION THE EVENT WAS DELIVERED TO.
        assert_eq!(
            notification["UnsubscribeUrl"].as_str(),
            Some(
                format!(
                    "https://sns.{REGION}.amazonaws.com/?Action=Unsubscribe&\
                     SubscriptionArn={subscription}"
                )
                .as_str()
            ),
            "case {case:?}: expected the unsubscribe link of this subscription, got {event}"
        );
        // The record is signed like every other enveloped notification, under the member
        // names a function's event spells them by.
        assert_eq!(
            notification["SignatureVersion"].as_str(),
            Some("1"),
            "case {case:?}: expected the topic's default signature version, got {event}"
        );
        assert!(
            notification["Signature"]
                .as_str()
                .is_some_and(|signature| !signature.is_empty()),
            "case {case:?}: expected the record to carry a signature, got {event}"
        );
        assert!(
            notification["SigningCertUrl"]
                .as_str()
                .is_some_and(|url| url.contains(".pem")),
            "case {case:?}: expected the record to name the signing certificate, got {event}"
        );
        // Always present, as an empty object for a message published with no attributes.
        let carried = notification["MessageAttributes"]
            .as_object()
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a MessageAttributes member, got {event}")
            });
        assert_eq!(
            carried.len(),
            row.attributes.len(),
            "case {case:?}: expected {} attribute(s), got {event}",
            row.attributes.len()
        );
        for (name, data_type, value) in &row.attributes {
            assert_eq!(
                carried[*name]["Type"].as_str(),
                Some(*data_type),
                "case {case:?}: expected the attribute {name:?} to carry type {data_type:?}, \
                 got {event}"
            );
            assert_eq!(
                carried[*name]["Value"].as_str(),
                Some(*value),
                "case {case:?}: expected the attribute {name:?} to carry {value:?}, got {event}"
            );
        }
    }
}

/// A function called `name` whose package holds `source` as its handler module, brought to
/// `Active` the way [`given_function`] brings one. Its ARN.
async fn given_function_running(emulator: &Emulator, name: &str, source: &str) -> String {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    let (status, created) = lambda(
        emulator,
        "POST",
        "/2015-03-31/functions",
        json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(buffer.into_inner()),
            },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateFunction {name}: expected HTTP 201, got {status} with {created}"
    );
    let arn = created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateFunction {name}: no FunctionArn in {created}"))
        .to_string();
    {
        let state = emulator
            .engine
            .state::<ComAmazonawsLambdaAwsgirapiserviceState>();
        let mut store = state.lock();
        let past = SystemTime::now() - Duration::from_secs(1);
        for record in store.functions.values_mut() {
            record.active_due = past;
        }
    }
    arn
}

/// A handler that forwards the message of every SNS record it is handed to the queue whose
/// url is formatted into it, which is how a subscriber of a topic is observed from outside
/// the function.
const FORWARDING_HANDLER: &str = r#"
import boto3


def lambda_handler(event, context):
    client = boto3.client("sqs")
    forwarded = 0
    for record in event["Records"]:
        client.send_message(
            QueueUrl="{queue_url}",
            MessageBody=record["Sns"]["Message"],
        )
        forwarded += 1
    return {"forwarded": forwarded}
"#;

/// The whole chain a client wires up: a publish to the topic invokes the subscribed function,
/// the function RUNS, and what it did is observable where it wrote — on the queue.
///
/// This is the end-to-end half of the rule: the rows above pin the event SNS hands the
/// function, and this pins that the invocation really reaches a running handler.
#[tokio::test]
async fn a_permissioned_subscription_runs_the_function_which_is_observed_where_it_wrote() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "forwarded")]).await,
        "TopicArn",
    );
    let created = sqs(&emulator, "CreateQueue", &json!({"QueueName": "sink"})).await;
    let queue_url = created["QueueUrl"]
        .as_str()
        .expect("CreateQueue must report a QueueUrl")
        .to_string();
    let function = "forwarding-probe";
    let function_arn = given_function_running(
        &emulator,
        function,
        &FORWARDING_HANDLER.replace("{queue_url}", &queue_url),
    )
    .await;
    grant(
        &emulator,
        function,
        "sns-invoke",
        INVOKE_ACTION,
        SNS_PRINCIPAL,
        Some(&topic),
    )
    .await;
    subscribe(&emulator, &topic, LAMBDA_PROTOCOL, &function_arn).await;

    let marker = "a forwarded marker";
    publish(&emulator, &topic, &[("Message", marker)]).await;

    let deadline = Instant::now() + DELIVERY_BUDGET;
    let bodies = loop {
        let received = sqs(
            &emulator,
            "ReceiveMessage",
            &json!({"QueueUrl": queue_url, "MaxNumberOfMessages": 10}),
        )
        .await;
        let bodies: Vec<String> = received["Messages"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
            .collect();
        if !bodies.is_empty() || Instant::now() >= deadline {
            break bodies;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    };
    assert!(
        bodies.iter().any(|body| body == marker),
        "case \"a permissioned lambda subscription\": expected the function to have forwarded \
         {marker:?} to the sink queue, got {bodies:?}"
    );
}

#[tokio::test]
async fn one_publish_reaches_every_subscribed_endpoint_of_the_topic() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "fan-out")]).await,
        "TopicArn",
    );
    let other_topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "fan-out-other")]).await,
        "TopicArn",
    );

    // (case, the function's name, the topic it subscribes to, how many events it must receive)
    let rows: &[(&str, &str, bool, usize)] = &[
        ("the first function subscribed to the topic", "fan-one", true, 1),
        ("a second function subscribed to the same topic", "fan-two", true, 1),
        ("a function subscribed to another topic", "fan-other", false, 0),
    ];

    let mut functions: Vec<(&str, &str, usize)> = Vec::new();
    for (case, name, same_topic, expected) in rows {
        let function_arn = given_function(&emulator, name).await;
        grant(
            &emulator,
            name,
            "sns-invoke",
            INVOKE_ACTION,
            SNS_PRINCIPAL,
            None,
        )
        .await;
        let subscribed_to = if *same_topic { &topic } else { &other_topic };
        subscribe(&emulator, subscribed_to, LAMBDA_PROTOCOL, &function_arn).await;
        functions.push((case, name, *expected));
    }

    // A QUEUE subscribed to the same topic: one publish fans out over every protocol the
    // topic carries subscriptions for, not just one of them.
    let created = sqs(&emulator, "CreateQueue", &json!({"QueueName": "fan-out-queue"})).await;
    let queue_url = created["QueueUrl"]
        .as_str()
        .expect("CreateQueue must report a QueueUrl")
        .to_string();
    let attributes = sqs(
        &emulator,
        "GetQueueAttributes",
        &json!({"QueueUrl": queue_url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let queue_arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .expect("GetQueueAttributes must report a QueueArn")
        .to_string();
    sqs(
        &emulator,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": queue_url,
            "Attributes": {
                "Policy": json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Sid": "AllowSNSDelivery",
                        "Effect": "Allow",
                        "Principal": {"Service": SNS_PRINCIPAL},
                        "Action": "sqs:SendMessage",
                        "Resource": queue_arn,
                    }],
                })
                .to_string(),
            },
        }),
    )
    .await;
    subscribe(&emulator, &topic, "sqs", &queue_arn).await;

    publish(&emulator, &topic, &[("Message", "one fan-out body")]).await;

    for (case, name, expected) in &functions {
        let events = if *expected == 0 {
            quiet(&emulator, name).await
        } else {
            invoked(&emulator, name, *expected).await
        };
        assert_eq!(
            events.len(),
            *expected,
            "case {case:?}: expected {expected} invocation(s) of {name:?}, got {events:?}"
        );
        for event in &events {
            assert_eq!(
                event["Records"][0]["Sns"]["Message"].as_str(),
                Some("one fan-out body"),
                "case {case:?}: expected the published body in the event, got {event}"
            );
        }
    }

    let received = sqs(
        &emulator,
        "ReceiveMessage",
        &json!({"QueueUrl": queue_url, "MaxNumberOfMessages": 10}),
    )
    .await;
    let bodies = received["Messages"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        bodies.len(),
        1,
        "case \"the queue subscribed to the same topic\": expected one message, got {received}"
    );
    let envelope: Value = serde_json::from_str(bodies[0]["Body"].as_str().unwrap_or_default())
        .expect("the delivered body must be the notification envelope");
    assert_eq!(
        envelope["Message"].as_str(),
        Some("one fan-out body"),
        "case \"the queue subscribed to the same topic\": expected the published body, got \
         {envelope}"
    );
}
