// SPDX-License-Identifier: CC0-1.0

//! A SESv2 send's email events reach the LAMBDA TARGET of an EventBridge rule, and what the
//! invoked function wrote is readable on the queue it wrote it to — over the WHOLE
//! `build_router` service set the binary serves.
//!
//! The RULE pinned here: **for each event an accepted send produces, every event destination
//! of the configuration set the message named that is `Enabled`, whose `MatchingEventTypes`
//! names that event's type and that carries an `EventBridgeDestination` publishes ONE event
//! onto the bus that destination's `EventBusArn` names — `source` `aws.ses`, a `detail-type`
//! naming the email event, and as `detail` the event document SES publishes for it (its
//! `eventType`, the `mail` object with the minted `MessageId` and the configuration set among
//! its tags, and the object the event itself carries) — and EventBridge then routes that
//! envelope by its CONTENT like any other, so a rule whose `EventPattern` matches hands it
//! UNCHANGED to its targets, and a Lambda target whose own resource-based policy admits
//! `lambda:InvokeFunction` to `events.amazonaws.com` is invoked with it.**
//!
//! Six crates have to agree for that to hold (SESv2 publishes, EventBridge matches and hands
//! over, Lambda decides the invocation against the function's policy and then RUNS the
//! handler, CloudFormation provisions the queue, the function, the rule and the grant, IAM
//! holds the execution role the handler's own write is judged against, and Amazon SQS holds
//! what the handler wrote), so the suite lives here rather than in any one crate, and it runs
//! over the production service set — so the in-process publication SESv2 makes is claimed and
//! routed by whichever mounted model a client's own `events`-scoped call would reach.
//!
//! What only this suite can reach is that last point. The crate-local suites
//! `services/service-com-amazonaws-sesv2-simpleemailservice-v2/tests/all/event_bridge_event_destination_delivery.rs`
//! (the publication rule over a table of destinations and recipient reactions) and
//! `.../event_bridge_rule_invokes_a_lambda_target.rs` (the same pipeline over the six crates
//! alone) pin the rule itself; `crate::ses_send_events_reach_a_subscribed_queue` pins the same
//! publication to an SNS event destination.
//!
//! The rows differ along what the rule generalises over: what the send's recipient does with
//! the message — a failure and a delivery — and so which email events the bus carries.

use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::outputs_of;
use crate::sns_lambda_subscription_permission_boundary::{
    cloudformation, drain, iam, lambda, trust_policy, ACCOUNT, CREATE_COMPLETE, LOOK_INTERVAL,
};

/// The Region every request here is signed for — the one the shared callers above sign for.
const REGION: &str = "us-east-1";

/// The domain identity that authorises the sends, and the sender it covers.
const SENDER_DOMAIN: &str = "senders.example.com";
const SENDER: &str = "sender@senders.example.com";

/// The `source` Amazon SES publishes a configuration set's email events under, which is what
/// the deployed rule matches on.
const SES_EVENT_SOURCE: &str = "aws.ses";

/// The message tag SES applies to every send made through a configuration set, which the
/// published document reports under `mail.tags`.
const CONFIGURATION_SET_TAG: &str = "ses:configuration-set";

/// The service principal EventBridge hands a resource-policy-governed target over as, and the
/// action such a handover performs.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The execution role the declared function is created with, provisioned in IAM before the
/// stack so the template names a role that service really holds.
const ROLE_NAME: &str = "ses-event-relay-exec";

/// How long each observation of work the clock owes — the identity verifying, the function
/// becoming `Active`, the publication and the handover — waits before giving up. Generous on
/// purpose: what is under test is that the work happens at all, never how fast this host gets
/// to it.
const BUDGET: Duration = Duration::from_secs(120);

/// The handler the stack declares inline: it writes the whole envelope EventBridge handed it
/// to the relay queue the same stack declares, which is where the invocation is observed. The
/// queue is addressed by NAME through Amazon SQS's own `GetQueueUrl`, so the source carries
/// nothing the deployment had to compute.
const RELAY_SOURCE: &str = r#"
import json
import boto3


def lambda_handler(event, context):
    client = boto3.client("sqs")
    url = client.get_queue_url(QueueName="RELAY_QUEUE_NAME")["QueueUrl"]
    client.send_message(QueueUrl=url, MessageBody=json.dumps(event))
    return {"relayed": 1}
"#;

/// One email event a row expects on the relay queue: the envelope's `detail-type` and the
/// `eventType` its detail is of.
#[derive(Clone, Copy)]
struct Relayed {
    detail_type: &'static str,
    event_type: &'static str,
}

/// One row of the table.
struct Row {
    case: &'static str,
    /// The stack, the configuration set and the queue of this row, which are named after it so
    /// two rows never share one.
    stack: &'static str,
    /// The one recipient of the send, a mailbox of the SES simulator.
    recipient: &'static str,
    /// The events the rule's function must be handed, and nothing besides.
    expected: &'static [Relayed],
}

const ROWS: &[Row] = &[
    Row {
        case: "a bouncing recipient reaches the rule's function with the failure",
        stack: "ses-events-bounce",
        recipient: "bounce@simulator.amazonses.com",
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
        stack: "ses-events-delivery",
        recipient: "success@simulator.amazonses.com",
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

/// The whole emulator over one engine, exactly as the binary assembles it.
fn router() -> MultiServiceRouter {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(engine);
    // Signature verification is the front door's concern, not this suite's.
    router.set_request_authentication(false);
    router
}

/// One SESv2 call: `restJson1` in SESv2's own `/v2/email/…` path space, routed by a SigV4
/// credential scope naming the signing name the SES family shares.
async fn sesv2(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match &body {
        Some(value) => Bytes::from(value.to_string()),
        None => Bytes::new(),
    };
    let host = format!("email.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", &host)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/ses/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The same call, which must answer the 200 its output shape declares.
async fn sesv2_ok(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let (status, parsed) = sesv2(router, method, path, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SESv2 {method} {path}, got {status} with {parsed}"
    );
    parsed
}

/// A verified sending domain, so the send below is authorised. The verification is a
/// transition the CLOCK owes the identity, so this polls the identity's own read for it rather
/// than waiting out a span.
async fn given_verified_sender(router: &MultiServiceRouter, case: &str) {
    sesv2_ok(
        router,
        case,
        "POST",
        "/v2/email/identities",
        Some(json!({"EmailIdentity": SENDER_DOMAIN})),
    )
    .await;
    let deadline = Instant::now() + BUDGET;
    loop {
        let reported = sesv2_ok(
            router,
            case,
            "GET",
            &format!("/v2/email/identities/{SENDER_DOMAIN}"),
            None,
        )
        .await;
        if reported["VerifiedForSendingStatus"] == json!(true) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: SESv2 never reported {SENDER_DOMAIN} verified for sending, last read \
             {reported}"
        );
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// The configuration set `set` with one EventBridge event destination, publishing the send,
/// delivery and bounce events of its sends onto the account's default bus.
async fn given_event_destination(router: &MultiServiceRouter, case: &str, set: &str) {
    sesv2_ok(
        router,
        case,
        "POST",
        "/v2/email/configuration-sets",
        Some(json!({"ConfigurationSetName": set})),
    )
    .await;
    sesv2_ok(
        router,
        case,
        "POST",
        &format!("/v2/email/configuration-sets/{set}/event-destinations"),
        Some(json!({
            "EventDestinationName": "bus",
            "EventDestination": {
                "Enabled": true,
                "MatchingEventTypes": ["SEND", "DELIVERY", "BOUNCE"],
                "EventBridgeDestination": {
                    "EventBusArn": format!(
                        "arn:aws:events:{REGION}:{ACCOUNT}:event-bus/default"
                    ),
                },
            },
        })),
    )
    .await;
}

/// `SendEmail` through `set` to `recipient`, answering the `MessageId` it minted.
async fn send(router: &MultiServiceRouter, case: &str, set: &str, recipient: &str) -> String {
    let answered = sesv2_ok(
        router,
        case,
        "POST",
        "/v2/email/outbound-emails",
        Some(json!({
            "FromEmailAddress": SENDER,
            "ConfigurationSetName": set,
            "Destination": {"ToAddresses": [recipient]},
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

/// The identity policy the execution role carries, so the write the handler makes is served: a
/// handler calls another service with temporary credentials for its execution role, so
/// resolving the relay queue's URL by name and the write itself are both judged against the
/// policies in force on that role.
async fn execution_role(router: &MultiServiceRouter) {
    iam(
        router,
        "CreateRole",
        &[
            ("RoleName", ROLE_NAME),
            ("AssumeRolePolicyDocument", &trust_policy()),
        ],
    )
    .await;
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ["sqs:GetQueueUrl", "sqs:SendMessage"],
            "Resource": "*",
        }],
    })
    .to_string();
    iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", ROLE_NAME),
            ("PolicyName", "write-the-relay-queue"),
            ("PolicyDocument", &document),
        ],
    )
    .await;
}

/// The template one row deploys: the relay queue, the function that writes it, the grant that
/// admits this service's invocation of that function, and a rule matching SESv2's own event
/// source with that function as its one target.
fn template(stack: &str) -> String {
    let relay = format!("{stack}-relay");
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Relay": {
                "Type": "AWS::SQS::Queue",
                "Properties": {"QueueName": relay},
            },
            "Consumer": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": format!("{stack}-consumer"),
                    "Role": format!("arn:aws:iam::{ACCOUNT}:role/{ROLE_NAME}"),
                    "Runtime": "python3.12",
                    "Handler": "index.lambda_handler",
                    "Timeout": 30,
                    "Code": {"ZipFile": RELAY_SOURCE.replace("RELAY_QUEUE_NAME", &relay)},
                },
            },
            "InvokeConsumer": {
                "Type": "AWS::Lambda::Permission",
                "Properties": {
                    "FunctionName": {"Fn::GetAtt": ["Consumer", "Arn"]},
                    "Action": INVOKE_ACTION,
                    "Principal": EVENTS_PRINCIPAL,
                },
            },
            "EmailEvents": {
                "Type": "AWS::Events::Rule",
                "Properties": {
                    "Name": format!("{stack}-rule"),
                    "EventPattern": {"source": [SES_EVENT_SOURCE]},
                    "State": "ENABLED",
                    "Targets": [{
                        "Id": "consumer",
                        "Arn": {"Fn::GetAtt": ["Consumer", "Arn"]},
                    }],
                },
            },
        },
        "Outputs": {
            "RelayUrl": {"Value": {"Ref": "Relay"}},
        },
    })
    .to_string()
}

/// Poll `GetFunctionConfiguration` until Lambda first reports `name` `Active` — "during this
/// time, you can't invoke or modify the function" (`CreateFunction`).
async fn wait_until_active(router: &MultiServiceRouter, case: &str, name: &str) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let (_, reported) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        if reported["State"] == json!("Active") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: Lambda never reported {name} Active, last read {reported}"
        );
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Drain the relay queue until it has carried `wanted` envelopes, reporting each parsed.
async fn relayed(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    wanted: usize,
) -> Vec<Value> {
    let deadline = Instant::now() + BUDGET;
    let mut held: Vec<Value> = Vec::new();
    loop {
        for body in drain(router, url).await {
            held.push(serde_json::from_str::<Value>(&body).unwrap_or_else(|error| {
                panic!("case {case:?}: a relayed event is not JSON ({error}): {body:?}")
            }));
        }
        if held.len() >= wanted || Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Every row: the send's email events reach the rule's function, which writes each envelope it
/// was handed to the relay queue.
#[tokio::test]
async fn a_sends_email_events_reach_the_lambda_target_of_a_rule_on_its_bus() {
    for row in ROWS {
        let case = row.case;
        let router = router();
        execution_role(&router).await;
        cloudformation(
            &router,
            "CreateStack",
            &[("StackName", row.stack), ("TemplateBody", &template(row.stack))],
        )
        .await;
        let (status, described) = wait_for_stack_status(&router, row.stack, CREATE_COMPLETE).await;
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to deploy, got {status} with {described}"
        );
        let relay_url = outputs_of(&described)
            .get("RelayUrl")
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the stack to publish RelayUrl, got {described}")
            });
        wait_until_active(&router, case, &format!("{}-consumer", row.stack)).await;

        given_verified_sender(&router, case).await;
        given_event_destination(&router, case, row.stack).await;

        let message_id = send(&router, case, row.stack, row.recipient).await;
        let held = relayed(&router, case, &relay_url, row.expected.len()).await;
        assert_eq!(
            held.len(),
            row.expected.len(),
            "case {case:?}: expected the rule's function to be handed {} event(s), got {held:?}",
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
            assert_eq!(
                envelope["detail"]["mail"]["tags"][CONFIGURATION_SET_TAG],
                json!([row.stack]),
                "case {case:?}: expected the configuration set among the mail tags, got {:?}",
                envelope["detail"]["mail"]["tags"]
            );
        }
    }
}
