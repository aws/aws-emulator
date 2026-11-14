// SPDX-License-Identifier: CC0-1.0

//! What a configuration set's Amazon EventBridge event destination DELIVERS: the events one
//! accepted send puts on the bus, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this service,
//! Amazon EventBridge and Amazon SQS mounted on ONE engine, which is what makes the bus a
//! send publishes onto the bus EventBridge routes — and asserts each event where a client
//! observes one: on the queue a rule's target enqueues the whole envelope into, read through
//! `sqs:ReceiveMessage`.
//!
//! The RULE these cases pin: for each event an accepted send produces, every event
//! destination of the configuration set the message named that is `Enabled`, whose
//! `MatchingEventTypes` names that event's type and that carries an `EventBridgeDestination`
//! publishes ONE event onto the bus that destination's `EventBusArn` names, carrying this
//! service's own event source, a `detail-type` naming the email event, and as its `detail`
//! the very notification document every other destination kind of that destination receives
//! — so a rule of that bus matching the source, or one of those detail types, hands its
//! targets the whole envelope; and every other destination publishes nothing.
//!
//! The table spans what the rule generalises over: which events the send's recipients
//! produce (acceptance only, a delivery, a failure, a complaint, several at once), how many
//! recipients one send has and whether they react differently, which event types the
//! destination matches (one, several, every value of `EventType`), and whether the
//! destination is on at all. The recorded case — a set with an EventBridge destination, a
//! bouncing recipient and then a deliverable one, with a rule matching the source — is two
//! rows of it.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::ComAmazonawsSesv2SimpleemailserviceV2State;
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for, and so the Region of the bus, the queue and
/// the rule.
const REGION: &str = "us-east-1";

/// The sender every send is from, and the domain identity that authorises it.
const SENDER: &str = "sender@senders.example.com";
const SENDER_DOMAIN: &str = "senders.example.com";

/// The subject every send carries.
const SUBJECT: &str = "a message under test";

/// The `source` Amazon SES publishes a configuration set's email events under, which is
/// what the rule below matches on.
const SES_EVENT_SOURCE: &str = "aws.ses";

/// The message tag SES applies to every send made through a configuration set, which the
/// published document reports under `mail.tags`.
const CONFIGURATION_SET_TAG: &str = "ses:configuration-set";

/// Every value of the `EventType` enum
/// (`smithy_query(query="shape",shape="EventType",model_path="services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/com-amazonaws-sesv2-simpleemailservice-v2.json")`).
const EVERY_EVENT_TYPE: &[&str] = &[
    "BOUNCE",
    "CLICK",
    "COMPLAINT",
    "DELIVERY",
    "DELIVERY_DELAY",
    "OPEN",
    "REJECT",
    "RENDERING_FAILURE",
    "SEND",
    "SUBSCRIPTION",
];

/// The mailbox-simulator addresses a send below is addressed to, and what each does with the
/// message (`state::event_publication::outcome`): `success` takes it, `bounce` is an unknown
/// user, `complaint` takes it and reports it.
const SUCCESS: &str = "success@simulator.amazonses.com";
const BOUNCING: &str = "bounce@simulator.amazonses.com";
const COMPLAINING: &str = "complaint@simulator.amazonses.com";
const ALSO_SUCCESS: &str = "ooto@simulator.amazonses.com";

/// The `x-amz-target` prefixes and JSON versions the two other surfaces serve (each
/// crate's `OPERATIONS.md` "Wire protocol").
const EVENTS_TARGET_PREFIX: &str = "AWSEvents";
const SQS_TARGET_PREFIX: &str = "AmazonSQS";

/// The service principal EventBridge invokes a resource-policy-governed target as.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// How many times the queue is read while waiting for the events a row expects.
///
/// An email event is published ASYNCHRONOUSLY — the message is accepted and answered before
/// any event of it exists — so the events arrive on a task of their own. Each pass reads the
/// queue again and yields to the runtime; nothing here waits for a SPAN, and the loop stops
/// as soon as what it is waiting for is there.
const READS_WHILE_PUBLISHING: usize = 2_000;

/// How many further reads confirm that what the queue holds has settled — so a row that
/// expects fewer events than the bus ends up carrying, or none at all, fails on the
/// comparison rather than on a race.
const SETTLED_READS: usize = 64;

/// One event a row expects on the bus: the `detail-type` its envelope carries, the
/// `eventType` its detail is of, and the addresses the detail itself names.
#[derive(Clone, Copy)]
struct Expected {
    detail_type: &'static str,
    event_type: &'static str,
    /// The recipients the detail's own list is about, in the order the send addressed them.
    addresses: &'static [&'static str],
    /// `(bounceType, bounceSubType)` for a failure, `None` for every other event.
    bounce: Option<(&'static str, &'static str)>,
}

/// One row of the table.
struct Row {
    case: &'static str,
    /// The configuration set the row creates and sends through.
    set: &'static str,
    /// `EventDestinationDefinition$Enabled`.
    enabled: bool,
    /// The event types the destination matches, as the definition names them.
    matching: &'static [&'static str],
    /// The recipients of the one send, in the order the request lists them.
    recipients: &'static [&'static str],
    /// The events the bus must carry afterwards, and nothing besides.
    expected: &'static [Expected],
}

const ROWS: &[Row] = &[
    Row {
        case: "a bouncing recipient under a destination matching every event type",
        set: "bounce-bus-set",
        enabled: true,
        matching: EVERY_EVENT_TYPE,
        recipients: &[BOUNCING],
        expected: &[
            Expected {
                detail_type: "Email Send",
                event_type: "Send",
                addresses: &[BOUNCING],
                bounce: None,
            },
            Expected {
                detail_type: "Email Bounce",
                event_type: "Bounce",
                addresses: &[BOUNCING],
                bounce: Some(("Permanent", "General")),
            },
        ],
    },
    Row {
        case: "a deliverable recipient under a destination matching every event type",
        set: "delivery-bus-set",
        enabled: true,
        matching: EVERY_EVENT_TYPE,
        recipients: &[SUCCESS],
        expected: &[
            Expected {
                detail_type: "Email Send",
                event_type: "Send",
                addresses: &[SUCCESS],
                bounce: None,
            },
            Expected {
                detail_type: "Email Delivery",
                event_type: "Delivery",
                addresses: &[SUCCESS],
                bounce: None,
            },
        ],
    },
    Row {
        case: "a recipient that reports the message",
        set: "complaint-bus-set",
        enabled: true,
        matching: EVERY_EVENT_TYPE,
        recipients: &[COMPLAINING],
        expected: &[
            Expected {
                detail_type: "Email Send",
                event_type: "Send",
                addresses: &[COMPLAINING],
                bounce: None,
            },
            Expected {
                detail_type: "Email Delivery",
                event_type: "Delivery",
                addresses: &[COMPLAINING],
                bounce: None,
            },
            Expected {
                detail_type: "Email Complaint",
                event_type: "Complaint",
                addresses: &[COMPLAINING],
                bounce: None,
            },
        ],
    },
    Row {
        case: "two recipients of one send that react differently",
        set: "mixed-recipients-set",
        enabled: true,
        matching: EVERY_EVENT_TYPE,
        recipients: &[BOUNCING, SUCCESS],
        expected: &[
            Expected {
                detail_type: "Email Send",
                event_type: "Send",
                addresses: &[BOUNCING, SUCCESS],
                bounce: None,
            },
            Expected {
                detail_type: "Email Bounce",
                event_type: "Bounce",
                addresses: &[BOUNCING],
                bounce: Some(("Permanent", "General")),
            },
            Expected {
                detail_type: "Email Delivery",
                event_type: "Delivery",
                addresses: &[SUCCESS],
                bounce: None,
            },
        ],
    },
    Row {
        case: "two recipients that both take the message are reported together",
        set: "two-deliveries-set",
        enabled: true,
        matching: &["DELIVERY"],
        recipients: &[SUCCESS, ALSO_SUCCESS],
        expected: &[Expected {
            detail_type: "Email Delivery",
            event_type: "Delivery",
            addresses: &[SUCCESS, ALSO_SUCCESS],
            bounce: None,
        }],
    },
    Row {
        case: "a destination matching one event type publishes only that one",
        set: "bounce-only-set",
        enabled: true,
        matching: &["BOUNCE"],
        recipients: &[BOUNCING],
        expected: &[Expected {
            detail_type: "Email Bounce",
            event_type: "Bounce",
            addresses: &[BOUNCING],
            bounce: Some(("Permanent", "General")),
        }],
    },
    Row {
        case: "a destination matching two of the types this send produces publishes both",
        set: "send-and-delivery-set",
        enabled: true,
        matching: &["SEND", "DELIVERY"],
        recipients: &[SUCCESS],
        expected: &[
            Expected {
                detail_type: "Email Send",
                event_type: "Send",
                addresses: &[SUCCESS],
                bounce: None,
            },
            Expected {
                detail_type: "Email Delivery",
                event_type: "Delivery",
                addresses: &[SUCCESS],
                bounce: None,
            },
        ],
    },
    Row {
        case: "a destination that is off publishes nothing",
        set: "switched-off-bus-set",
        enabled: false,
        matching: EVERY_EVENT_TYPE,
        recipients: &[BOUNCING],
        expected: &[],
    },
    Row {
        case: "a destination matching only event types this send never produces publishes \
               nothing",
        set: "unmatched-type-bus-set",
        enabled: true,
        matching: &["OPEN", "CLICK", "RENDERING_FAILURE"],
        recipients: &[SUCCESS],
        expected: &[],
    },
];

/// One answered request: its status and parsed body.
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// A string at `pointer`, or `""` when the body carries nothing there.
    fn text(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator under test: this service, EventBridge and Amazon SQS on ONE engine, the
/// shape `core/router/src/lib.rs` mounts every service in.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(Arc::clone(&engine)),
        );
        Emulator { router, engine }
    }

    /// One call on this service's restJson1 surface.
    async fn ses(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.{REGION}.amazonaws.com{path}"))
            .header("host", format!("email.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header("authorization", authorization("ses"))
            .body(payload)
            .expect("build request");
        self.answered(request).await
    }

    /// The same call, requiring the 200 the operation's output shape declares.
    async fn ses_ok(&self, case: &str, method: &str, path: &str, body: Option<Value>) -> Reply {
        let reply = self.ses(method, path, body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: {method} {path} must be served, got {} {}",
            reply.status, reply.body
        );
        reply
    }

    /// One `awsJson` RPC call against the service whose signing name is `signing_name`.
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
            .expect("build request");
        let reply = self.answered(request).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: {signing_name}:{operation} must be served, got {} {}",
            reply.status, reply.body
        );
        reply.body
    }

    async fn events(&self, case: &str, operation: &str, body: &Value) -> Value {
        self.rpc(case, "events", EVENTS_TARGET_PREFIX, "1.1", operation, body)
            .await
    }

    async fn sqs(&self, case: &str, operation: &str, body: &Value) -> Value {
        self.rpc(case, "sqs", SQS_TARGET_PREFIX, "1.0", operation, body)
            .await
    }

    async fn answered(&self, request: http::Request<bytes::Bytes>) -> Reply {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let body = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    /// The account with a verified sending domain, so every send below is authorised.
    async fn given_verified_sender(&self, case: &str) {
        self.ses_ok(
            case,
            "POST",
            "/v2/email/identities",
            Some(json!({"EmailIdentity": SENDER_DOMAIN})),
        )
        .await;
        // Bring the verification transition the clock owes the identity into the past, so a
        // send does not race the span.
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

    /// A configuration set carrying one EventBridge event destination, onto the account's
    /// default bus.
    async fn given_destination(
        &self,
        case: &str,
        set: &str,
        enabled: bool,
        matching: &[&str],
    ) {
        self.ses_ok(
            case,
            "POST",
            "/v2/email/configuration-sets",
            Some(json!({"ConfigurationSetName": set})),
        )
        .await;
        self.ses_ok(
            case,
            "POST",
            &format!("/v2/email/configuration-sets/{set}/event-destinations"),
            Some(json!({
                "EventDestinationName": "sim2real-bus",
                "EventDestination": {
                    "Enabled": enabled,
                    "MatchingEventTypes": matching,
                    "EventBridgeDestination": {"EventBusArn": default_bus_arn()},
                },
            })),
        )
        .await;
    }

    /// A queue whose own policy admits the EventBridge delivery; answers its url.
    async fn given_queue(&self, case: &str, name: &str) -> String {
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
        // "For Lambda functions, Amazon SNS topics and Amazon SQS queues, EventBridge relies
        // on resource-based policies" (`PutTargets`), so the queue grants the send itself.
        let policy = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"Service": EVENTS_PRINCIPAL},
                "Action": "sqs:SendMessage",
                "Resource": arn,
            }],
        })
        .to_string();
        self.sqs(
            case,
            "SetQueueAttributes",
            &json!({"QueueUrl": url, "Attributes": {"Policy": policy}}),
        )
        .await;
        (url, arn).0
    }

    /// A rule of the default bus matching `pattern`, with the queue at `queue_arn` as its
    /// one target.
    async fn given_rule(&self, case: &str, name: &str, pattern: &Value, queue_arn: &str) {
        self.events(
            case,
            "PutRule",
            &json!({"Name": name, "EventPattern": pattern.to_string(), "State": "ENABLED"}),
        )
        .await;
        let answered = self
            .events(
                case,
                "PutTargets",
                &json!({
                    "Rule": name,
                    "Targets": [{"Id": "queue", "Arn": queue_arn}],
                }),
            )
            .await;
        assert_eq!(
            answered["FailedEntryCount"],
            json!(0),
            "case {case:?}: PutTargets reported a failed entry: {answered}"
        );
    }

    /// `SendEmail` through `set` to `recipients`, answering the `MessageId` it minted.
    async fn send(&self, case: &str, set: &str, recipients: &[&str]) -> String {
        let request = json!({
            "FromEmailAddress": SENDER,
            "Destination": {"ToAddresses": recipients},
            "ConfigurationSetName": set,
            "Content": {"Simple": {
                "Subject": {"Data": SUBJECT},
                "Body": {"Text": {"Data": "hello"}},
            }},
        });
        let reply = self
            .ses_ok(case, "POST", "/v2/email/outbound-emails", Some(request))
            .await;
        let message_id = reply.text("/MessageId");
        assert!(
            !message_id.is_empty(),
            "case {case:?}: an accepted send must answer a MessageId, got {}",
            reply.body
        );
        message_id
    }

    /// Every message waiting on the queue at `url` right now, as the parsed envelope each
    /// body carries.
    async fn received(&self, case: &str, url: &str) -> Vec<Value> {
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
                    panic!("case {case:?}: a delivered event is not JSON ({error}): {body:?}")
                })
            })
            .collect()
    }
}

/// The `authorization` header a call signed for `signing_name` carries: the credential scope
/// is what routes the request to the service that serves it.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=00"
    )
}

/// The ARN of the account's default event bus in this Region, which is the only bus an
/// `EventBridgeDestination` supports (`EventBridgeDestination$EventBusArn`).
fn default_bus_arn() -> String {
    format!(
        "arn:aws:events:{REGION}:{}:event-bus/default",
        engine::emulator_account_id()
    )
}

/// The addresses the detail of a `Send`/`Bounce`/`Delivery`/`Complaint` event names, read
/// from the member that event's own document carries them in.
fn addresses_of(event_type: &str, detail: &Value) -> Vec<String> {
    let listed = |pointer: &str, member: Option<&str>| -> Vec<String> {
        detail
            .pointer(pointer)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| match member {
                Some(member) => entry
                    .get(member)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                None => entry.as_str().unwrap_or_default().to_string(),
            })
            .collect()
    };
    match event_type {
        // The acceptance is about the message, so the addresses it names are the ones the
        // message was accepted for.
        "Send" => listed("/mail/destination", None),
        "Bounce" => listed("/bounce/bouncedRecipients", Some("emailAddress")),
        "Delivery" => listed("/delivery/recipients", None),
        "Complaint" => listed("/complaint/complainedRecipients", Some("emailAddress")),
        other => panic!("no event of this suite is of type {other:?}"),
    }
}

/// Assert ONE delivered envelope against the event `expected` describes, for the send that
/// minted `message_id` through `set`.
fn assert_event(case: &str, expected: &Expected, envelope: &Value, set: &str, message_id: &str) {
    let detail = &envelope["detail"];
    assert_eq!(
        envelope["source"],
        json!(SES_EVENT_SOURCE),
        "case {case:?}: expected source {SES_EVENT_SOURCE:?}, got {:?}",
        envelope["source"]
    );
    assert_eq!(
        envelope["detail-type"],
        json!(expected.detail_type),
        "case {case:?}: expected detail-type {:?}, got {:?}",
        expected.detail_type,
        envelope["detail-type"]
    );
    assert_eq!(
        detail["eventType"],
        json!(expected.event_type),
        "case {case:?}: expected eventType {:?}, got {:?}",
        expected.event_type,
        detail["eventType"]
    );
    assert_eq!(
        detail["mail"]["messageId"],
        json!(message_id),
        "case {case:?}: expected mail.messageId {message_id:?}, got {:?}",
        detail["mail"]["messageId"]
    );
    assert_eq!(
        detail["mail"]["tags"][CONFIGURATION_SET_TAG],
        json!([set]),
        "case {case:?}: expected mail.tags[{CONFIGURATION_SET_TAG:?}] to name {set:?}, got {:?}",
        detail["mail"]["tags"]
    );
    let actual = addresses_of(expected.event_type, detail);
    let wanted: Vec<String> = expected.addresses.iter().map(|at| at.to_string()).collect();
    assert_eq!(
        actual, wanted,
        "case {case:?}: expected the {:?} event to name {wanted:?}, got {actual:?}",
        expected.event_type
    );
    if let Some((bounce_type, sub_type)) = expected.bounce {
        assert_eq!(
            detail["bounce"]["bounceType"],
            json!(bounce_type),
            "case {case:?}: expected bounce.bounceType {bounce_type:?}, got {:?}",
            detail["bounce"]["bounceType"]
        );
        assert_eq!(
            detail["bounce"]["bounceSubType"],
            json!(sub_type),
            "case {case:?}: expected bounce.bounceSubType {sub_type:?}, got {:?}",
            detail["bounce"]["bounceSubType"]
        );
    }
}

/// The envelopes the queue holds once the events `wanted` many are all there, and then once
/// what it holds has settled.
async fn settled_envelopes(
    emulator: &Emulator,
    case: &str,
    url: &str,
    wanted: usize,
) -> Vec<Value> {
    let mut held: Vec<Value> = Vec::new();
    for _ in 0..READS_WHILE_PUBLISHING {
        if held.len() >= wanted {
            break;
        }
        held.extend(emulator.received(case, url).await);
        ::tokio::task::yield_now().await;
    }
    for _ in 0..SETTLED_READS {
        ::tokio::task::yield_now().await;
        let again = emulator.received(case, url).await;
        assert!(
            again.is_empty(),
            "case {case:?}: the bus kept delivering events: {again:?} on top of {held:?}"
        );
    }
    held
}

#[tokio::test]
async fn a_send_publishes_one_bus_event_per_matched_email_event() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        emulator.given_verified_sender(case).await;
        emulator
            .given_destination(case, row.set, row.enabled, row.matching)
            .await;
        let url = emulator.given_queue(case, "ses-events-queue").await;
        let arn = format!(
            "arn:aws:sqs:{REGION}:{}:ses-events-queue",
            engine::emulator_account_id()
        );
        emulator
            .given_rule(
                case,
                "ses-events-rule",
                &json!({"source": [SES_EVENT_SOURCE]}),
                &arn,
            )
            .await;

        // Nothing reaches the bus before the send: attaching a destination publishes
        // nothing.
        let before = emulator.received(case, &url).await;
        assert!(
            before.is_empty(),
            "case {case:?}: attaching a destination must publish no event, got {before:?}"
        );

        let message_id = emulator.send(case, row.set, row.recipients).await;
        let held = settled_envelopes(&emulator, case, &url, row.expected.len()).await;
        assert_eq!(
            held.len(),
            row.expected.len(),
            "case {case:?}: expected {} event(s) on the bus, got {held:?}",
            row.expected.len()
        );
        // One event per expectation, matched by its own `detail-type`: the bus carries no
        // order of its own across separate publications.
        for expected in row.expected {
            let envelope = held
                .iter()
                .find(|envelope| envelope["detail-type"] == json!(expected.detail_type))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: no {:?} event among {held:?}",
                        expected.detail_type
                    )
                });
            assert_event(case, expected, envelope, row.set, &message_id);
        }
    }
}

#[tokio::test]
async fn a_rule_matching_one_detail_type_receives_only_that_email_event() {
    // The detail-type is what a rule over ONE email event matches on: a rule naming only the
    // failure receives the failure and not the acceptance, though the destination publishes
    // both onto the same bus.
    let case = "a rule naming only the failure's detail-type";
    let set = "detail-type-rule-set";
    let emulator = Emulator::new();
    emulator.given_verified_sender(case).await;
    emulator
        .given_destination(case, set, true, EVERY_EVENT_TYPE)
        .await;
    let url = emulator.given_queue(case, "bounce-only-queue").await;
    let arn = format!(
        "arn:aws:sqs:{REGION}:{}:bounce-only-queue",
        engine::emulator_account_id()
    );
    emulator
        .given_rule(
            case,
            "bounce-only-rule",
            &json!({"source": [SES_EVENT_SOURCE], "detail-type": ["Email Bounce"]}),
            &arn,
        )
        .await;

    let message_id = emulator.send(case, set, &[BOUNCING]).await;
    let held = settled_envelopes(&emulator, case, &url, 1).await;
    assert_eq!(
        held.len(),
        1,
        "case {case:?}: expected exactly the failure on the bus, got {held:?}"
    );
    assert_event(
        case,
        &Expected {
            detail_type: "Email Bounce",
            event_type: "Bounce",
            addresses: &[BOUNCING],
            bounce: Some(("Permanent", "General")),
        },
        &held[0],
        set,
        &message_id,
    );
}

#[tokio::test]
async fn a_send_without_a_configuration_set_publishes_nothing() {
    // The destination belongs to a configuration set, so a send that names none — and a send
    // that names another set — produces events no destination of this set receives.
    let case = "a send naming no configuration set";
    let set = "unused-bus-set";
    let other = "other-bus-set";
    let emulator = Emulator::new();
    emulator.given_verified_sender(case).await;
    emulator
        .given_destination(case, set, true, EVERY_EVENT_TYPE)
        .await;
    emulator
        .ses_ok(
            case,
            "POST",
            "/v2/email/configuration-sets",
            Some(json!({"ConfigurationSetName": other})),
        )
        .await;
    let url = emulator.given_queue(case, "quiet-queue").await;
    let arn = format!(
        "arn:aws:sqs:{REGION}:{}:quiet-queue",
        engine::emulator_account_id()
    );
    emulator
        .given_rule(
            case,
            "quiet-rule",
            &json!({"source": [SES_EVENT_SOURCE]}),
            &arn,
        )
        .await;

    emulator
        .ses_ok(
            case,
            "POST",
            "/v2/email/outbound-emails",
            Some(json!({
                "FromEmailAddress": SENDER,
                "Destination": {"ToAddresses": [BOUNCING]},
                "Content": {"Simple": {
                    "Subject": {"Data": SUBJECT},
                    "Body": {"Text": {"Data": "hello"}},
                }},
            })),
        )
        .await;
    emulator.send(case, other, &[BOUNCING]).await;

    for _ in 0..SETTLED_READS {
        let held = emulator.received(case, &url).await;
        assert!(
            held.is_empty(),
            "case {case:?}: neither send names the set carrying the destination, so nothing \
             must reach the bus, got {held:?}"
        );
        ::tokio::task::yield_now().await;
    }
}
