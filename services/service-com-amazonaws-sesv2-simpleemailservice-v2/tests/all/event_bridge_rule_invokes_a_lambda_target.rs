// SPDX-License-Identifier: CC0-1.0

//! The whole pipeline a client wires over email events, end to end over HTTP: a send under a
//! configuration set whose event destination names an event bus publishes its events onto
//! that bus, an enabled rule matching this service's event source matches each of them, and
//! the rule's Lambda target RUNS — the handler relaying what it was handed onto a queue a
//! client then drains.
//!
//! The RULE pinned here is the one [`super::event_bridge_event_destination_delivery`] states
//! over the publication itself, carried one hop further: what an `EventBridgeDestination`
//! publishes is an ordinary event of that bus, so every consumer of that bus — a rule's
//! target included — is handed the whole envelope, and the target's own code can read the
//! `detail` the email event is of.
//!
//! Six crates have to agree for that to hold (this service publishes, EventBridge matches and
//! hands over, Lambda runs the handler and supplies it an SDK, IAM holds the execution role's
//! grants, the token service vends the session the handler's call is signed as, Amazon SQS
//! serves the relay queue), so the suite mounts them on ONE engine.
//!
//! The rows differ along what the recipients of the one send do with it — a failure and a
//! delivery — which is what decides which email events the pipeline carries.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::ComAmazonawsSesv2SimpleemailserviceV2State;
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for.
const REGION: &str = "us-east-1";

/// The sender every send is from, and the domain identity that authorises it.
const SENDER: &str = "sender@senders.example.com";
const SENDER_DOMAIN: &str = "senders.example.com";

/// The `source` Amazon SES publishes a configuration set's email events under, which is what
/// the rule matches on.
const SES_EVENT_SOURCE: &str = "aws.ses";

/// The mailbox-simulator addresses the rows send to.
const SUCCESS: &str = "success@simulator.amazonses.com";
const BOUNCING: &str = "bounce@simulator.amazonses.com";

/// The principal EventBridge invokes a resource-policy-governed target as, and the action
/// such an invocation performs.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The module and attribute the deployment package carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long each observation of work the clock owes — the function becoming `Active`, the
/// publication, the delivery and the handler's own relay — waits before giving up, and how
/// long between two looks. The deadline is generous: what is under test is that the work
/// happens at all, never how fast this host gets to it.
const BUDGET: Duration = Duration::from_secs(120);
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// How long one invocation is given, as the function's own `Timeout`: well past what the
/// handler needs on any host.
const FUNCTION_TIMEOUT: u64 = 60;

/// The handler every row deploys. It relays the WHOLE envelope EventBridge handed it onto the
/// queue named in its environment, so what the target was invoked with is readable through
/// Amazon SQS afterwards.
const RELAY_SOURCE: &str = r#"
import json
import os

import boto3


def lambda_handler(event, context):
    boto3.client("sqs").send_message(
        QueueUrl=os.environ["RELAY_QUEUE_URL"],
        MessageBody=json.dumps(event, sort_keys=True),
    )
    return {"relayed": event["detail-type"]}
"#;

/// One event a row expects to be relayed: its `detail-type` and the `eventType` its detail
/// is of.
#[derive(Clone, Copy)]
struct Relayed {
    detail_type: &'static str,
    event_type: &'static str,
}

/// One row of the table.
struct Row {
    case: &'static str,
    /// The configuration set the row creates and sends through.
    set: &'static str,
    /// The one recipient of the send.
    recipient: &'static str,
    /// The events the handler must be invoked with, and nothing besides.
    expected: &'static [Relayed],
}

const ROWS: &[Row] = &[
    Row {
        case: "a bouncing recipient reaches the rule's function with the failure",
        set: "relay-bounce-set",
        recipient: BOUNCING,
        expected: &[
            Relayed {
                detail_type: "Email Send",
                event_type: "Send",
            },
            Relayed {
                detail_type: "Email Bounce",
                event_type: "Bounce",
            },
        ],
    },
    Row {
        case: "a deliverable recipient reaches the rule's function with the delivery",
        set: "relay-delivery-set",
        recipient: SUCCESS,
        expected: &[
            Relayed {
                detail_type: "Email Send",
                event_type: "Send",
            },
            Relayed {
                detail_type: "Email Delivery",
                event_type: "Delivery",
            },
        ],
    },
];

/// The emulator under test: the six services this pipeline needs on ONE engine, the shape
/// `core/router/src/lib.rs` mounts every service in.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4: the credential scope is all the
        // header carries. The handler's OWN call is signed by the runtime with the credential
        // it was given.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Emulator { router, engine }
    }

    /// One answered request: its status and parsed body.
    async fn answered(&self, request: http::Request<bytes::Bytes>) -> (u16, Value) {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One `X-Amz-Target` RPC call that must be served.
    async fn rpc(
        &self,
        case: &str,
        signing_name: &str,
        target_prefix: &str,
        json_version: &str,
        operation: &str,
        body: &Value,
    ) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header(
                "content-type",
                format!("application/x-amz-json-{json_version}"),
            )
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header("authorization", authorization(signing_name))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let (status, parsed) = self.answered(request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {signing_name}:{operation} must be served, got {status} {parsed}"
        );
        parsed
    }

    async fn events(&self, case: &str, operation: &str, body: &Value) -> Value {
        self.rpc(case, "events", "AWSEvents", "1.1", operation, body)
            .await
    }

    async fn sqs(&self, case: &str, operation: &str, body: &Value) -> Value {
        self.rpc(case, "sqs", "AmazonSQS", "1.0", operation, body)
            .await
    }

    /// One call on this service's restJson1 surface that must be served.
    async fn ses(&self, case: &str, method: &str, path: &str, body: Option<Value>) -> Value {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(value.to_string()),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.{REGION}.amazonaws.com{path}"))
            .header("host", format!("email.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header("authorization", authorization("ses"))
            .body(payload)
            .expect("the test request is well formed");
        let (status, parsed) = self.answered(request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {method} {path} must be served, got {status} {parsed}"
        );
        parsed
    }

    /// One Lambda restJson1 call, as its status and parsed body.
    async fn lambda(&self, method: &str, path: &str, body: &Value) -> (u16, Value) {
        let payload = if body.is_null() {
            bytes::Bytes::new()
        } else {
            bytes::Bytes::from(body.to_string())
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://lambda.{REGION}.amazonaws.com{path}"))
            .header("host", format!("lambda.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header("authorization", authorization("lambda"))
            .body(payload)
            .expect("the test request is well formed");
        self.answered(request).await
    }

    /// One awsQuery call to IAM that must be served.
    async fn iam(&self, case: &str, action: &str, params: &[(&str, String)]) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization("iam"))
            .body(bytes::Bytes::from(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "case {case:?}: iam:{action} must be served, got {status} {answered}"
        );
    }

    /// The account with a verified sending domain, so every send below is authorised.
    async fn given_verified_sender(&self, case: &str) {
        self.ses(
            case,
            "POST",
            "/v2/email/identities",
            Some(json!({"EmailIdentity": SENDER_DOMAIN})),
        )
        .await;
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let past = SystemTime::now()
            .checked_sub(Duration::from_secs(1))
            .expect("an instant before now");
        for record in store.identities.values_mut() {
            record.verification_due_at = past;
        }
        store.settle(SystemTime::now());
    }

    /// A configuration set whose one event destination publishes the events of this send onto
    /// the account's default bus.
    async fn given_destination(&self, case: &str, set: &str) {
        self.ses(
            case,
            "POST",
            "/v2/email/configuration-sets",
            Some(json!({"ConfigurationSetName": set})),
        )
        .await;
        self.ses(
            case,
            "POST",
            &format!("/v2/email/configuration-sets/{set}/event-destinations"),
            Some(json!({
                "EventDestinationName": "relay-bus",
                "EventDestination": {
                    "Enabled": true,
                    "MatchingEventTypes": ["SEND", "DELIVERY", "BOUNCE"],
                    "EventBridgeDestination": {"EventBusArn": default_bus_arn()},
                },
            })),
        )
        .await;
    }

    /// The relay queue the handler sends what it was invoked with onto; answers its url and
    /// ARN.
    async fn given_relay_queue(&self, case: &str, name: &str) -> (String, String) {
        let created = self.sqs(case, "CreateQueue", &json!({"QueueName": name})).await;
        let url = created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateQueue reported no QueueUrl: {created}"))
            .to_string();
        let attributes = self
            .sqs(
                case,
                "GetQueueAttributes",
                &json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
            )
            .await;
        let arn = attributes["Attributes"]["QueueArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: GetQueueAttributes reported no QueueArn: {attributes}")
            })
            .to_string();
        (url, arn)
    }

    /// The execution role the function runs as: trusted by Lambda, and carrying the one grant
    /// the handler's own call needs. Answers the role's ARN.
    async fn given_execution_role(&self, case: &str, name: &str, queue_arn: &str) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"Service": "lambda.amazonaws.com"},
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        self.iam(
            case,
            "CreateRole",
            &[
                ("RoleName", name.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": "sqs:SendMessage",
                "Resource": queue_arn,
            }],
        })
        .to_string();
        self.iam(
            case,
            "PutRolePolicy",
            &[
                ("RoleName", name.to_string()),
                ("PolicyName", "relay-the-event".to_string()),
                ("PolicyDocument", document),
            ],
        )
        .await;
        format!("arn:aws:iam::{}:role/{name}", account())
    }

    /// The function the rule targets, past the transition its creation armed; answers its ARN.
    async fn given_function(
        &self,
        case: &str,
        name: &str,
        role_arn: &str,
        relay_url: &str,
    ) -> String {
        let (status, created) = self
            .lambda(
                "POST",
                "/2015-03-31/functions",
                &json!({
                    "FunctionName": name,
                    "Role": role_arn,
                    "Runtime": "python3.12",
                    "Handler": HANDLER,
                    "Timeout": FUNCTION_TIMEOUT,
                    "Environment": {"Variables": {"RELAY_QUEUE_URL": relay_url}},
                    "Code": {"ZipFile": package()},
                }),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateFunction must answer 201, got {status} {created}"
        );
        let arn = created["FunctionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateFunction reported no ARN: {created}"))
            .to_string();
        let deadline = Instant::now() + BUDGET;
        loop {
            let (_, configuration) = self
                .lambda(
                    "GET",
                    &format!("/2015-03-31/functions/{name}/configuration"),
                    &Value::Null,
                )
                .await;
            if configuration["State"].as_str() == Some("Active") {
                return arn;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: Lambda never reported {name:?} Active, last read {configuration}"
            );
            tokio::time::sleep(LOOK_AGAIN).await;
        }
    }

    /// Grant EventBridge the invocation on `function`: "for Lambda functions, Amazon SNS
    /// topics and Amazon SQS queues, EventBridge relies on resource-based policies"
    /// (`PutTargets`).
    async fn given_invoke_grant(&self, case: &str, function: &str) {
        let (status, granted) = self
            .lambda(
                "POST",
                &format!("/2015-03-31/functions/{function}/policy"),
                &json!({
                    "StatementId": "AllowEventBridgeInvoke",
                    "Action": INVOKE_ACTION,
                    "Principal": EVENTS_PRINCIPAL,
                }),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: AddPermission must answer 201, got {status} {granted}"
        );
    }

    /// A rule of the default bus matching this service's event source, with `function_arn` as
    /// its one target.
    async fn given_rule(&self, case: &str, name: &str, function_arn: &str) {
        self.events(
            case,
            "PutRule",
            &json!({
                "Name": name,
                "EventPattern": json!({"source": [SES_EVENT_SOURCE]}).to_string(),
                "State": "ENABLED",
            }),
        )
        .await;
        let answered = self
            .events(
                case,
                "PutTargets",
                &json!({"Rule": name, "Targets": [{"Id": "relay", "Arn": function_arn}]}),
            )
            .await;
        assert_eq!(
            answered["FailedEntryCount"],
            json!(0),
            "case {case:?}: PutTargets reported a failed entry: {answered}"
        );
    }

    /// `SendEmail` through `set` to `recipient`, answering the `MessageId` it minted.
    async fn send(&self, case: &str, set: &str, recipient: &str) -> String {
        let answered = self
            .ses(
                case,
                "POST",
                "/v2/email/outbound-emails",
                Some(json!({
                    "FromEmailAddress": SENDER,
                    "Destination": {"ToAddresses": [recipient]},
                    "ConfigurationSetName": set,
                    "Content": {"Simple": {
                        "Subject": {"Data": "a message under test"},
                        "Body": {"Text": {"Data": "hello"}},
                    }},
                })),
            )
            .await;
        let message_id = answered["MessageId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !message_id.is_empty(),
            "case {case:?}: an accepted send must answer a MessageId, got {answered}"
        );
        message_id
    }

    /// Every envelope waiting on the relay queue right now.
    async fn relayed(&self, case: &str, url: &str) -> Vec<Value> {
        let answered = self
            .sqs(
                case,
                "ReceiveMessage",
                &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
            )
            .await;
        answered["Messages"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|message| {
                let body = message["Body"].as_str().unwrap_or_default();
                serde_json::from_str::<Value>(body).unwrap_or_else(|error| {
                    panic!("case {case:?}: a relayed event is not JSON ({error}): {body:?}")
                })
            })
            .collect()
    }
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// the request. The signature is never checked here.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=00"
    )
}

/// The account every emulated resource of this suite belongs to, as the engine holds it.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// The ARN of the account's default event bus in this Region.
fn default_bus_arn() -> String {
    format!("arn:aws:events:{REGION}:{}:event-bus/default", account())
}

/// `application/x-www-form-urlencoded` for one awsQuery parameter value.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// A .zip deployment package holding the handler above as `lambda_function.py`, written in
/// memory the way a caller uploads one and base64-encoded as `FunctionCode$ZipFile` carries
/// it.
fn package() -> String {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(RELAY_SOURCE.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    base64::engine::general_purpose::STANDARD.encode(buffer.into_inner())
}

/// Drain the relay queue until it has carried `wanted` envelopes, or the budget is spent.
async fn drained(emulator: &Emulator, case: &str, url: &str, wanted: usize) -> Vec<Value> {
    let deadline = Instant::now() + BUDGET;
    let mut held: Vec<Value> = Vec::new();
    while held.len() < wanted {
        held.extend(emulator.relayed(case, url).await);
        if held.len() >= wanted || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    held
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_email_events_rule_invokes_its_function_with_the_published_envelope() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        let (relay_url, relay_arn) = emulator.given_relay_queue(case, "ses-event-relay").await;
        let role_arn = emulator
            .given_execution_role(case, "ses-event-relay-role", &relay_arn)
            .await;
        let function_arn = emulator
            .given_function(case, "ses-event-relay-fn", &role_arn, &relay_url)
            .await;
        emulator.given_invoke_grant(case, "ses-event-relay-fn").await;
        emulator
            .given_rule(case, "ses-events-relay-rule", &function_arn)
            .await;
        emulator.given_verified_sender(case).await;
        emulator.given_destination(case, row.set).await;

        let message_id = emulator.send(case, row.set, row.recipient).await;
        let held = drained(&emulator, case, &relay_url, row.expected.len()).await;
        assert_eq!(
            held.len(),
            row.expected.len(),
            "case {case:?}: expected the function to be invoked with {} event(s), got {held:?}",
            row.expected.len()
        );
        for expected in row.expected {
            let envelope = held
                .iter()
                .find(|envelope| envelope["detail-type"] == json!(expected.detail_type))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: no {:?} event among the relayed {held:?}",
                        expected.detail_type
                    )
                });
            assert_eq!(
                envelope["source"],
                json!(SES_EVENT_SOURCE),
                "case {case:?}: expected source {SES_EVENT_SOURCE:?}, got {:?}",
                envelope["source"]
            );
            assert_eq!(
                envelope["detail"]["eventType"],
                json!(expected.event_type),
                "case {case:?}: expected detail.eventType {:?}, got {:?}",
                expected.event_type,
                envelope["detail"]["eventType"]
            );
            assert_eq!(
                envelope["detail"]["mail"]["messageId"],
                json!(message_id),
                "case {case:?}: expected detail.mail.messageId {message_id:?}, got {:?}",
                envelope["detail"]["mail"]["messageId"]
            );
        }
    }
}
