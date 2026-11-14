// SPDX-License-Identifier: CC0-1.0

//! The TOPIC RULE DESTINATION lifecycle and the gate it exists for, end to end over the
//! real `MultiServiceRouter` with the IoT control plane and the IoT data plane mounted on
//! one engine.
//!
//! The rule under test, in one sentence: an HTTP topic rule destination is created
//! `IN_PROGRESS` and challenged at its `confirmationUrl` with a token; the token coming
//! back through `ConfirmTopicRuleDestination` completes the confirmation, after which the
//! destination rests at `DISABLED`; only `UpdateTopicRuleDestination` moves it to
//! `ENABLED`; and a rule's `http` action delivers to its endpoint URL only while a
//! destination that admits that URL is `ENABLED` — "The destination must be confirmed prior
//! to use" (`CreateTopicRuleDestination`), `TopicRuleDestinationStatus` for each status's
//! meaning, and `HttpAction$confirmationUrl` ("The value of the confirmation URL must be a
//! prefix of the endpoint URL") for which destination admits which action.
//!
//! The ENDPOINT these cases confirm and deliver to is the emulator's own IoT DATA PLANE:
//! the destination's `confirmationUrl` is a `Publish` of a retained message, so the
//! challenge this service sends and the rule traffic it delivers are observed exactly where
//! the owner of an endpoint observes them — as what arrived at the endpoint, read back
//! through that service's own `GetRetainedMessage`. Nothing is mocked and no socket is
//! opened: the request goes over the emulator's in-process transport.
//!
//! The tables span the class rather than one recorded case:
//!
//! * the create/challenge table varies the confirmation URL (a path-only URL, one that
//!   already carries a query string, one with a longer path) and asserts the challenge
//!   request itself — method, the token as a query parameter, and the document's members;
//! * the DELIVERY table is the differential the whole feature exists for: one endpoint, one
//!   rule, and a message published at EVERY status the destination passes through
//!   (`IN_PROGRESS`, `DISABLED`, `ENABLED`, `DISABLED` again), with delivery happening at
//!   exactly one of them;
//! * the GATE table varies what decides whether an action is admitted — the destination's
//!   status, an endpoint URL under the confirmed prefix and one outside it, an action
//!   naming its own `confirmationUrl`, and values carrying a substitution template;
//! * the PER-DESTINATION table decides every row from ONE publish of one rule carrying an
//!   action per endpoint, so enabling one destination is shown not to open an endpoint whose
//!   own destination rests at `DISABLED`, is still `IN_PROGRESS`, or does not exist;
//! * the refusal table varies the way a request can be wrong — an unknown ARN, an unknown
//!   token, a status no caller can ask for, enabling an unconfirmed destination, a second
//!   create for one URL, and a configuration naming a destination kind this emulator does
//!   not create.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use aws_smithy_runtime_api::client::http::{
    HttpConnector, HttpConnectorFuture, SharedHttpConnector,
};
use aws_smithy_runtime_api::client::orchestrator::{HttpRequest, HttpResponse};
use aws_smithy_runtime_api::http::StatusCode;
use aws_smithy_types::body::SdkBody;
use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::topic_rule_destinations::{
    challenge_destination, deliver_to_endpoint, http_action_is_allowed, http_action_unperformable,
    InProcessTransport, RegisteredTopicRuleDestination, CONFIRMATION_SPAN,
    DESTINATION_CONFIRMATION_MESSAGE_TYPE, DESTINATION_DISABLED, DESTINATION_ENABLED,
    DESTINATION_ERROR, DESTINATION_IN_PROGRESS,
};
use service_com_amazonaws_iot_awsiotservice::ops::types::{HttpAction, HttpActionHeader};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::describe_endpoint::percent_encode;

/// The region every call below is signed for.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

/// How long a challenge or a delivery is waited for before a case fails: far longer than
/// anything this emulator takes, so a slow machine does not fail a case.
const ARRIVAL_DEADLINE: Duration = Duration::from_secs(10);

/// How often an endpoint is read while waiting for something to arrive at it.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long past the confirmation span a case waits before requiring `DISABLED`.
fn past_the_confirmation() -> Duration {
    CONFIRMATION_SPAN + Duration::from_secs(1)
}

// ---------------------------------------------------------------------------
// The emulator
// ---------------------------------------------------------------------------

/// One HTTP answer, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// The error message a modelled failure carries.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `topicRuleDestination` member a create or a read reports.
    fn destination(&self) -> &Value {
        &self.body["topicRuleDestination"]
    }

    /// The reported destination's `status`.
    fn status_value(&self) -> String {
        self.destination()["status"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The reported destination's `arn`.
    fn arn(&self) -> String {
        self.destination()["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator under test: the IoT control plane and the IoT data plane on ONE shared
/// engine, the shape `core/router/src/lib.rs` mounts them in. The engine is kept so a case
/// can drive the challenge over a transport of its own.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iotdataplane_iotmoonrakerservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// Dispatch one request signed for `signing_name`, answered with whatever the service
    /// said.
    async fn send(&self, method: &str, uri: &str, signing_name: &str, body: Vec<u8>) -> Reply {
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header("authorization", authorization(signing_name))
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// One IoT control-plane call (restJson1 over the `iot` signing name).
    async fn iot(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        self.send(
            method,
            &format!("https://iot.{REGION}.amazonaws.com{path}"),
            "iot",
            payload,
        )
        .await
    }

    /// `CreateTopicRuleDestination`: `POST /destinations`.
    async fn create_destination(&self, configuration: &Value) -> Reply {
        self.iot(
            "POST",
            "/destinations",
            Some(&json!({ "destinationConfiguration": configuration })),
        )
        .await
    }

    /// `CreateTopicRuleDestination` of an HTTP destination for `confirmation_url`.
    async fn create_http_destination(&self, confirmation_url: &str) -> Reply {
        self.create_destination(&json!({
            "httpUrlConfiguration": { "confirmationUrl": confirmation_url },
        }))
        .await
    }

    /// `GetTopicRuleDestination`: `GET /destinations/{arn+}`.
    async fn get_destination(&self, arn: &str) -> Reply {
        self.iot("GET", &format!("/destinations/{arn}"), None).await
    }

    /// `ConfirmTopicRuleDestination`: `GET /confirmdestination/{confirmationToken+}`.
    async fn confirm_destination(&self, token: &str) -> Reply {
        self.iot("GET", &format!("/confirmdestination/{token}"), None)
            .await
    }

    /// `UpdateTopicRuleDestination`: `PATCH /destinations`.
    async fn update_destination(&self, arn: &str, status: &str) -> Reply {
        self.iot(
            "PATCH",
            "/destinations",
            Some(&json!({ "arn": arn, "status": status })),
        )
        .await
    }

    /// `CreateTopicRule`: `POST /rules/{ruleName}`, whose body IS the `topicRulePayload`.
    async fn create_topic_rule(&self, case: &str, rule_name: &str, payload: &Value) {
        let reply = self
            .iot("POST", &format!("/rules/{rule_name}"), Some(payload))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: CreateTopicRule({rule_name}) answered {} {:?}",
            reply.status, reply.body
        );
    }

    /// `Publish` of the IoT data plane: `POST /topics/{topic}` at QoS 1. The rules engine
    /// runs inside this call, so an action's delivery has happened by the time it answers.
    async fn publish(&self, case: &str, topic: &str, payload: &[u8]) {
        let reply = self
            .send(
                "POST",
                &format!(
                    "https://iotdata.{REGION}.amazonaws.com/topics/{}?qos=1",
                    percent_encode(topic)
                ),
                "iotdata",
                payload.to_vec(),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: Publish({topic}) answered {} {:?}",
            reply.status, reply.body
        );
    }

    /// What the endpoint topic `topic` last received, read through the data plane's own
    /// `GetRetainedMessage`, or `None` while nothing has arrived there.
    async fn received(&self, topic: &str) -> Option<Vec<u8>> {
        let reply = self
            .send(
                "GET",
                &format!(
                    "https://iotdata.{REGION}.amazonaws.com/retainedMessage/{}",
                    percent_encode(topic)
                ),
                "iotdata",
                Vec::new(),
            )
            .await;
        if reply.status != 200 {
            return None;
        }
        let encoded = reply.body["payload"].as_str()?;
        aws_smithy_types::base64::decode(encoded).ok()
    }

    /// What the endpoint topic `topic` received, waited for up to [`ARRIVAL_DEADLINE`].
    async fn received_within(&self, case: &str, topic: &str) -> Vec<u8> {
        let deadline = SystemTime::now() + ARRIVAL_DEADLINE;
        loop {
            if let Some(received) = self.received(topic).await {
                return received;
            }
            assert!(
                SystemTime::now() < deadline,
                "case {case:?}: nothing arrived at the endpoint topic {topic:?} within \
                 {ARRIVAL_DEADLINE:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// The destination's status, waited for until it is `expected` or the deadline passes;
    /// answered with the last status read either way.
    async fn status_within(&self, case: &str, arn: &str, expected: &str) -> String {
        let deadline = SystemTime::now() + ARRIVAL_DEADLINE;
        loop {
            let read = self.get_destination(arn).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: GetTopicRuleDestination answered {} {:?}",
                read.status, read.body
            );
            let status = read.status_value();
            if status == expected || SystemTime::now() >= deadline {
                return status;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`: the scope is
/// how the router picks the claiming service. Signature verification is off in this
/// harness, so the value itself is never checked.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// The URL of one endpoint this emulator serves itself: a retained `Publish` of the IoT
/// data plane, which records what arrived at it.
fn endpoint_url(topic: &str) -> String {
    format!("https://iotdata.{REGION}.amazonaws.com/topics/{topic}?qos=1&retain=true")
}

/// The confirmation document that arrived at an endpoint, as JSON.
fn document(case: &str, received: &[u8]) -> Value {
    serde_json::from_slice(received).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: the challenge document must be JSON, got {error} for {:?}",
            String::from_utf8_lossy(received)
        )
    })
}

/// The `confirmationToken` of a challenge document.
fn token_of(case: &str, received: &[u8]) -> String {
    let document = document(case, received);
    document["confirmationToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the challenge names no token: {document}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// A transport that records the challenge request itself
// ---------------------------------------------------------------------------

/// One request an endpoint was sent, as the endpoint sees it.
#[derive(Debug, Clone)]
struct Captured {
    method: String,
    uri: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

/// An endpoint that records every request it is sent and answers 200 — the in-process
/// transport a challenge can be sent over when a case asserts the REQUEST rather than what
/// the endpoint did with it.
#[derive(Debug, Clone)]
struct Recorder {
    captured: Arc<Mutex<Vec<Captured>>>,
}

impl Recorder {
    fn new() -> Self {
        Self {
            captured: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn transport(&self) -> InProcessTransport {
        InProcessTransport::over_in_process_endpoint(SharedHttpConnector::new(self.clone()))
    }

    fn captured(&self) -> Vec<Captured> {
        self.captured
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl HttpConnector for Recorder {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        self.captured
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Captured {
                method: request.method().to_string(),
                uri: request.uri().to_string(),
                headers: request
                    .headers()
                    .iter()
                    .map(|(name, value)| (name.to_ascii_lowercase(), value.to_string()))
                    .collect(),
                body: request.body().bytes().unwrap_or_default().to_vec(),
            });
        let status = StatusCode::try_from(200).expect("200 is a status");
        let response = HttpResponse::new(status, SdkBody::empty());
        HttpConnectorFuture::new(Box::pin(async move { Ok(response) }))
    }
}

// ---------------------------------------------------------------------------
// Create, and the challenge the created destination's endpoint receives
// ---------------------------------------------------------------------------

/// A created HTTP destination is `IN_PROGRESS`, reports the URL it will be confirmed at,
/// and its endpoint receives a `DestinationConfirmation` document naming it and the token
/// that confirms it.
#[tokio::test]
async fn a_created_destination_is_in_progress_and_its_endpoint_is_challenged() {
    // (case, the endpoint topic the confirmation URL addresses)
    let rows: [(&str, &str); 3] = [
        ("a single-level endpoint topic", "confirm-one"),
        ("a longer endpoint topic name", "confirm-two-for-this-destination"),
        ("an endpoint topic with punctuation", "confirm_three-3"),
    ];
    let emulator = Emulator::new();

    for (case, topic) in rows {
        let url = endpoint_url(topic);
        let created = emulator.create_http_destination(&url).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateTopicRuleDestination answered {} {:?}",
            created.status, created.body
        );
        let arn = created.arn();
        assert!(
            arn.starts_with(&format!(
                "arn:aws:iot:{REGION}:{ACCOUNT_ID}:ruledestination/http/"
            )) && arn.len() > format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:ruledestination/http/").len(),
            "case {case:?}: expected an HTTP rule-destination ARN, got {arn:?}"
        );
        assert_eq!(
            created.status_value(),
            DESTINATION_IN_PROGRESS,
            "case {case:?}: expected a created destination to be {DESTINATION_IN_PROGRESS}, \
             got {:?}",
            created.destination()
        );
        assert_eq!(
            created.destination()["httpUrlProperties"]["confirmationUrl"],
            json!(url),
            "case {case:?}: expected the created destination to report its confirmation \
             URL, got {:?}",
            created.destination()
        );

        // The reader reports the same record the create answered with.
        let read = emulator.get_destination(&arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetTopicRuleDestination answered {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.destination(),
            created.destination(),
            "case {case:?}: expected the reader to report the created destination member \
             for member, got {:?}",
            read.destination()
        );

        // The challenge the create sent off its response path: observed where the owner of
        // the endpoint observes it.
        let received = emulator.received_within(case, topic).await;
        let document = document(case, &received);
        assert_eq!(
            document["messageType"],
            json!(DESTINATION_CONFIRMATION_MESSAGE_TYPE),
            "case {case:?}: expected a {DESTINATION_CONFIRMATION_MESSAGE_TYPE} document, \
             got {document}"
        );
        assert_eq!(
            document["arn"],
            json!(arn),
            "case {case:?}: expected the challenge to name the created destination, got \
             {document}"
        );
        let token = token_of(case, &received);
        assert!(
            !token.is_empty(),
            "case {case:?}: expected the challenge to carry a token, got {document}"
        );
        assert!(
            document["enableUrl"]
                .as_str()
                .is_some_and(|url| url.ends_with(&format!("/confirmdestination/{token}"))),
            "case {case:?}: expected the challenge's enableUrl to lead to this emulator's \
             ConfirmTopicRuleDestination for its own token, got {document}"
        );
    }
}

/// The challenge REQUEST carries its token as a query parameter on the configured
/// confirmation URL, whatever query string that URL already had.
#[tokio::test]
async fn the_challenge_request_carries_its_token_as_a_query_parameter() {
    // (case, the confirmation URL, the separator the token must be added with)
    let rows: [(&str, &str, char); 3] = [
        (
            "a URL with no query string",
            "https://example.test/iot-confirm",
            '?',
        ),
        (
            "a URL that already carries a query string",
            "https://example.test/iot-confirm?stage=test",
            '&',
        ),
        (
            "a URL with a longer path",
            "https://example.test/a/b/c/iot-confirm",
            '?',
        ),
    ];
    let emulator = Emulator::new();

    for (case, url, separator) in rows {
        let created = emulator.create_http_destination(url).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateTopicRuleDestination answered {} {:?}",
            created.status, created.body
        );
        let arn = created.arn();

        // The challenge sent over a transport that records the request, so what is asserted
        // is the request this emulator renders for the configured URL.
        let recorder = Recorder::new();
        challenge_destination(
            &emulator.engine,
            &recorder.transport(),
            "https://emulator.invalid",
            &arn,
        )
        .await;
        let captured = recorder.captured();
        assert_eq!(
            captured.len(),
            1,
            "case {case:?}: expected exactly one challenge request, got {captured:?}"
        );
        let request = &captured[0];
        assert_eq!(
            request.method, "POST",
            "case {case:?}: expected the challenge to be a POST, got {:?}",
            request.method
        );
        let token = document(case, &request.body)["confirmationToken"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            request.uri,
            format!("{url}{separator}confirmationToken={token}"),
            "case {case:?}: expected the challenge to carry its token as a query \
             parameter, got {:?}",
            request.uri
        );
        assert_eq!(
            request.headers.get("content-type").map(String::as_str),
            Some("application/json"),
            "case {case:?}: expected a JSON challenge document, got {:?}",
            request.headers
        );
    }
}

// ---------------------------------------------------------------------------
// The gate: what a rule's http action delivers, at each status
// ---------------------------------------------------------------------------

/// The differential the whole feature exists for: one endpoint, one enabled rule, and a
/// message published at every status the destination passes through — delivered at
/// `ENABLED` and at no other.
#[tokio::test]
async fn rule_traffic_reaches_the_endpoint_only_while_its_destination_is_enabled() {
    let case = "the destination's status decides delivery";
    let endpoint_topic = "rule-endpoint";
    let url = endpoint_url(endpoint_topic);
    let emulator = Emulator::new();

    let created = emulator.create_http_destination(&url).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreateTopicRuleDestination answered {} {:?}",
        created.status, created.body
    );
    let arn = created.arn();

    // The rule publishes to the endpoint URL the destination is about. Its own topic is not
    // the endpoint topic, so what the endpoint receives selects no rule of its own.
    emulator
        .create_topic_rule(
            case,
            "destination_gate",
            &json!({
                "sql": "SELECT * FROM 'telemetry/+'",
                "ruleDisabled": false,
                "actions": [{ "http": { "url": url } }],
            }),
        )
        .await;

    // The challenge is what the endpoint has received so far, and carries the token.
    let challenge = emulator.received_within(case, endpoint_topic).await;
    let token = token_of(case, &challenge);

    // (row, the status the destination is in when the message is published, the marker, is
    // it delivered)
    let mut row = 0_usize;
    let mut last_received = challenge.clone();
    for (status, delivered) in [
        (DESTINATION_IN_PROGRESS, false),
        (DESTINATION_DISABLED, false),
        (DESTINATION_ENABLED, true),
        (DESTINATION_DISABLED, false),
    ] {
        row += 1;
        let row_case = format!("{case}: row {row} at {status}");
        // Put the destination in the row's status. `IN_PROGRESS` is where the create left
        // it; the confirmed statuses are reached the way a client reaches them.
        match (row, status) {
            (1, _) => {}
            (2, _) => {
                let confirmed = emulator.confirm_destination(&token).await;
                assert_eq!(
                    confirmed.status, 200,
                    "case {row_case:?}: ConfirmTopicRuleDestination answered {} {:?}",
                    confirmed.status, confirmed.body
                );
                tokio::time::sleep(past_the_confirmation()).await;
            }
            (_, status) => {
                let updated = emulator.update_destination(&arn, status).await;
                assert_eq!(
                    updated.status, 200,
                    "case {row_case:?}: UpdateTopicRuleDestination answered {} {:?}",
                    updated.status, updated.body
                );
            }
        }
        let reported = emulator.status_within(&row_case, &arn, status).await;
        assert_eq!(
            reported, status,
            "case {row_case:?}: expected the destination to be {status}, got {reported}"
        );

        let marker = format!("marker-{row}-{status}");
        emulator
            .publish(&row_case, "telemetry/room", marker.as_bytes())
            .await;
        let received = emulator.received_within(&row_case, endpoint_topic).await;
        if delivered {
            assert_eq!(
                String::from_utf8_lossy(&received),
                marker,
                "case {row_case:?}: expected the published message to be delivered to the \
                 endpoint, got {:?}",
                String::from_utf8_lossy(&received)
            );
        } else {
            assert_eq!(
                received, last_received,
                "case {row_case:?}: expected NOTHING to be delivered to the endpoint, but \
                 it received {:?}",
                String::from_utf8_lossy(&received)
            );
        }
        last_received = received;
    }
}

/// Which destination admits which `http` action: the prefix relation
/// `HttpAction$confirmationUrl` states, the status gate, and the values this engine does
/// not render.
#[tokio::test]
async fn an_http_action_is_admitted_by_an_enabled_destination_that_prefixes_its_url() {
    let confirmed = "https://example.test/iot";
    let now = SystemTime::now();

    // (case, the destination's confirmation URL and status, the action, admitted)
    let rows: [(&str, &str, &str, HttpAction, bool); 9] = [
        (
            "the endpoint URL is the confirmed URL",
            confirmed,
            DESTINATION_ENABLED,
            action(confirmed, None, &[]),
            true,
        ),
        (
            "the endpoint URL is under the confirmed prefix",
            confirmed,
            DESTINATION_ENABLED,
            action("https://example.test/iot/events?x=1", None, &[]),
            true,
        ),
        (
            "the endpoint URL is outside the confirmed prefix",
            confirmed,
            DESTINATION_ENABLED,
            action("https://example.test/other", None, &[]),
            false,
        ),
        (
            "another host entirely",
            confirmed,
            DESTINATION_ENABLED,
            action("https://elsewhere.test/iot", None, &[]),
            false,
        ),
        (
            "the action names the confirmed URL as its confirmationUrl",
            confirmed,
            DESTINATION_ENABLED,
            action("https://example.test/iot/events", Some(confirmed), &[]),
            true,
        ),
        (
            "the action's confirmationUrl is not a prefix of its url",
            confirmed,
            DESTINATION_ENABLED,
            action(
                "https://example.test/iot/events",
                Some("https://example.test/elsewhere"),
                &[],
            ),
            false,
        ),
        (
            "the destination has not been confirmed",
            confirmed,
            DESTINATION_IN_PROGRESS,
            action(confirmed, None, &[]),
            false,
        ),
        (
            "the destination is confirmed and disabled",
            confirmed,
            DESTINATION_DISABLED,
            action(confirmed, None, &[]),
            false,
        ),
        (
            "the destination's confirmation failed",
            confirmed,
            DESTINATION_ERROR,
            action(confirmed, None, &[]),
            false,
        ),
    ];

    for (case, confirmation_url, status, action, expected) in rows {
        let mut destination = RegisteredTopicRuleDestination::new(
            "arn:aws:iot:us-east-1:000000000000:ruledestination/http/one".to_string(),
            confirmation_url.to_string(),
            "token".to_string(),
            now,
        );
        destination.status = status.to_string();
        let destinations = BTreeMap::from([(destination.arn.clone(), destination)]);
        let actual = http_action_is_allowed(&destinations, &action);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected http_action_is_allowed == {expected}, got {actual} \
             for {action:?} against a {status} destination of {confirmation_url:?}"
        );
    }

    // An action carrying a value this engine does not render from the message is not
    // performed at all, whatever the destinations say.
    // (case, the action, the reason it is not performed)
    let unperformable: [(&str, HttpAction, bool); 5] = [
        ("a plain action", action(confirmed, None, &[]), false),
        (
            "a substitution template in the url",
            action("https://example.test/iot/${topic()}", None, &[]),
            true,
        ),
        (
            "a substitution template in the confirmationUrl",
            action(
                "https://example.test/iot/events",
                Some("https://example.test/${id}"),
                &[],
            ),
            true,
        ),
        (
            "a substitution template in a header",
            action(confirmed, None, &[("x-device", "${clientId()}")]),
            true,
        ),
        (
            "a literal header",
            action(confirmed, None, &[("x-device", "sensor")]),
            false,
        ),
    ];
    for (case, action, expected) in unperformable {
        let actual = http_action_unperformable(&action).is_some();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected http_action_unperformable.is_some() == {expected}, \
             got {:?} for {action:?}",
            http_action_unperformable(&action)
        );
    }
}

/// The delivery request carries the message and the headers the action named — except the
/// ones this emulator's own request dispatch reads, which a rule does not get to choose
/// because the endpoint is reached over that dispatch.
#[tokio::test]
async fn a_delivery_carries_the_actions_headers_but_not_the_ones_dispatch_reads() {
    let url = "https://example.test/iot/events";
    // (case, the header the action names, does it travel)
    let rows: [(&str, (&str, &str), bool); 5] = [
        ("a header of the destination's own", ("x-device", "sensor"), true),
        ("a content type", ("content-type", "application/json"), true),
        ("the host that selects the service", ("Host", "s3.amazonaws.com"), false),
        (
            "an authorization the rule chose",
            ("authorization", "AWS4-HMAC-SHA256 Credential=AKIDROGUE/20260101/us-east-1/s3/aws4_request"),
            false,
        ),
        ("a target header", ("X-Amz-Target", "DynamoDB_20120810.GetItem"), false),
    ];

    for (case, (name, value), travels) in rows {
        let recorder = Recorder::new();
        let delivered = deliver_to_endpoint(
            &recorder.transport(),
            &action(url, None, &[(name, value)]),
            b"{\"marker\":\"one\"}",
        )
        .await;
        assert!(
            delivered.is_ok(),
            "case {case:?}: the endpoint took the delivery, got {delivered:?}"
        );
        let captured = recorder.captured();
        assert_eq!(
            captured.len(),
            1,
            "case {case:?}: expected exactly one delivery request, got {captured:?}"
        );
        let request = &captured[0];
        assert_eq!(
            request.body,
            b"{\"marker\":\"one\"}".to_vec(),
            "case {case:?}: expected the selected message as the body, got {:?}",
            String::from_utf8_lossy(&request.body)
        );
        let carried = request.headers.get(&name.to_ascii_lowercase());
        if travels {
            assert_eq!(
                carried.map(String::as_str),
                Some(value),
                "case {case:?}: expected the action's header to travel, got {:?}",
                request.headers
            );
        } else {
            assert_ne!(
                carried.map(String::as_str),
                Some(value),
                "case {case:?}: expected the header this emulator's dispatch reads NOT to \
                 be the rule's value, got {:?}",
                request.headers
            );
        }
    }
}

/// One `http` action: the endpoint URL, the confirmation URL it names (if any) and its
/// headers.
fn action(url: &str, confirmation_url: Option<&str>, headers: &[(&str, &str)]) -> HttpAction {
    HttpAction {
        url: url.to_string(),
        confirmation_url: confirmation_url.map(str::to_string),
        headers: (!headers.is_empty()).then(|| {
            headers
                .iter()
                .map(|(key, value)| HttpActionHeader {
                    key: (*key).to_string(),
                    value: (*value).to_string(),
                })
                .collect()
        }),
        auth: None,
        enable_batching: None,
        batch_config: None,
    }
}

// ---------------------------------------------------------------------------
// A fresh challenge, and the token it supersedes
// ---------------------------------------------------------------------------

/// Setting the status back to `IN_PROGRESS` challenges the endpoint again, and the token
/// the superseded challenge carried no longer confirms anything.
#[tokio::test]
async fn a_requested_challenge_supersedes_the_token_the_last_one_carried() {
    let case = "a resent challenge";
    let endpoint_topic = "resend-endpoint";
    let url = endpoint_url(endpoint_topic);
    let emulator = Emulator::new();

    let created = emulator.create_http_destination(&url).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreateTopicRuleDestination answered {} {:?}",
        created.status, created.body
    );
    let arn = created.arn();
    let first = token_of(case, &emulator.received_within(case, endpoint_topic).await);

    let resent = emulator
        .update_destination(&arn, DESTINATION_IN_PROGRESS)
        .await;
    assert_eq!(
        resent.status, 200,
        "case {case:?}: UpdateTopicRuleDestination(IN_PROGRESS) answered {} {:?}",
        resent.status, resent.body
    );
    // The endpoint receives a second challenge, carrying a token that is not the first.
    let deadline = SystemTime::now() + ARRIVAL_DEADLINE;
    let second = loop {
        let token = token_of(case, &emulator.received_within(case, endpoint_topic).await);
        if token != first {
            break token;
        }
        assert!(
            SystemTime::now() < deadline,
            "case {case:?}: no fresh challenge arrived at the endpoint within \
             {ARRIVAL_DEADLINE:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };

    let stale = emulator.confirm_destination(&first).await;
    assert_eq!(
        (stale.status, stale.error_code.as_deref()),
        (400, Some("InvalidRequestException")),
        "case {case:?}: expected the superseded token to confirm nothing, got {} {:?}",
        stale.status,
        stale.body
    );
    assert_eq!(
        emulator.status_within(case, &arn, DESTINATION_IN_PROGRESS).await,
        DESTINATION_IN_PROGRESS,
        "case {case:?}: expected the destination to stay {DESTINATION_IN_PROGRESS}"
    );

    let confirmed = emulator.confirm_destination(&second).await;
    assert_eq!(
        confirmed.status, 200,
        "case {case:?}: the fresh token must confirm, got {} {:?}",
        confirmed.status, confirmed.body
    );
    // Confirming does not itself move the status: the clock does, and it rests at DISABLED.
    assert_eq!(
        emulator.get_destination(&arn).await.status_value(),
        DESTINATION_IN_PROGRESS,
        "case {case:?}: expected the destination to still be {DESTINATION_IN_PROGRESS} \
         immediately after the confirmation"
    );
    tokio::time::sleep(past_the_confirmation()).await;
    assert_eq!(
        emulator.status_within(case, &arn, DESTINATION_DISABLED).await,
        DESTINATION_DISABLED,
        "case {case:?}: expected a confirmed destination to rest at {DESTINATION_DISABLED}"
    );
    // A token already confirmed changes nothing: the destination stays where it rests.
    let again = emulator.confirm_destination(&second).await;
    assert_eq!(
        again.status, 200,
        "case {case:?}: confirming an already confirmed destination answered {} {:?}",
        again.status, again.body
    );
    assert_eq!(
        emulator.get_destination(&arn).await.status_value(),
        DESTINATION_DISABLED,
        "case {case:?}: expected a repeated confirmation to change nothing"
    );
}

/// The gate is PER DESTINATION, not per account: one publish, one rule carrying an `http`
/// action for each of four endpoints, and only the endpoint whose own destination is
/// `ENABLED` receives the message — the destination that rests at `DISABLED`, the one still
/// `IN_PROGRESS`, and the endpoint no destination was ever created for each receive nothing
/// from the same publish.
///
/// This is the whole lifecycle driven through the modelled API (`POST /destinations`,
/// `GET /confirmdestination/{confirmationToken+}`, `PATCH /destinations`,
/// `GET /destinations/{arn+}` and the data plane's `Publish`), with the endpoints' own view
/// read back through the data plane: enabling ONE destination must not open any of the
/// others.
#[tokio::test]
async fn one_enabled_destination_does_not_open_another_endpoint() {
    // (case, the endpoint topic, the status its destination is driven to, does the publish
    // reach it). The rows differ along what the gate reads: the destination's status, and
    // whether a destination admitting that endpoint exists at all.
    let rows: [(&str, &str, Option<&str>, bool); 4] = [
        (
            "its destination is enabled",
            "gate-endpoint-enabled",
            Some(DESTINATION_ENABLED),
            true,
        ),
        (
            "its destination rests at disabled",
            "gate-endpoint-disabled",
            Some(DESTINATION_DISABLED),
            false,
        ),
        (
            "its destination is unconfirmed",
            "gate-endpoint-unconfirmed",
            Some(DESTINATION_IN_PROGRESS),
            false,
        ),
        (
            "no destination admits it",
            "gate-endpoint-unregistered",
            None,
            false,
        ),
    ];
    let emulator = Emulator::new();

    // Every endpoint a destination is created for is challenged, so what it has received by
    // the time the message is published is that challenge — the baseline each row's
    // "nothing was delivered" is measured against.
    let mut baseline: Vec<Option<Vec<u8>>> = Vec::new();
    for (case, topic, wanted, _) in rows {
        let url = endpoint_url(topic);
        let Some(wanted) = wanted else {
            baseline.push(None);
            continue;
        };
        let created = emulator.create_http_destination(&url).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateTopicRuleDestination answered {} {:?}",
            created.status, created.body
        );
        let arn = created.arn();
        assert_eq!(
            created.status_value(),
            DESTINATION_IN_PROGRESS,
            "case {case:?}: expected a created destination to be {DESTINATION_IN_PROGRESS}, \
             got {:?}",
            created.status_value()
        );
        let challenge = emulator.received_within(case, topic).await;
        baseline.push(Some(challenge.clone()));

        if wanted != DESTINATION_IN_PROGRESS {
            // The token the endpoint was handed is what completes the confirmation; the move
            // out of `IN_PROGRESS` is the clock's, so the row waits for the status rather
            // than for a span.
            let token = token_of(case, &challenge);
            let confirmed = emulator.confirm_destination(&token).await;
            assert_eq!(
                confirmed.status, 200,
                "case {case:?}: ConfirmTopicRuleDestination answered {} {:?}",
                confirmed.status, confirmed.body
            );
            let rested = emulator
                .status_within(case, &arn, DESTINATION_DISABLED)
                .await;
            assert_eq!(
                rested, DESTINATION_DISABLED,
                "case {case:?}: expected a confirmed destination to rest at \
                 {DESTINATION_DISABLED}, got {rested}"
            );
        }
        if wanted == DESTINATION_ENABLED {
            let enabled = emulator.update_destination(&arn, DESTINATION_ENABLED).await;
            assert_eq!(
                enabled.status, 200,
                "case {case:?}: UpdateTopicRuleDestination({DESTINATION_ENABLED}) answered \
                 {} {:?}",
                enabled.status, enabled.body
            );
        }
        let reported = emulator.status_within(case, &arn, wanted).await;
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the destination to be {wanted}, got {reported}"
        );
    }

    // ONE rule, one action per endpoint: every row is decided from the same publish.
    let actions: Vec<Value> = rows
        .iter()
        .map(|(_, topic, _, _)| json!({ "http": { "url": endpoint_url(topic) } }))
        .collect();
    emulator
        .create_topic_rule(
            "the gated rule",
            "per_destination_gate",
            &json!({
                "sql": "SELECT * FROM 'gated/+'",
                "ruleDisabled": false,
                "actions": actions,
            }),
        )
        .await;

    let marker = "per-destination-gate-marker";
    emulator
        .publish("the gated rule", "gated/room", marker.as_bytes())
        .await;

    // The rules engine runs INSIDE the publish (`Publish` awaits it), so by the time the
    // publish has answered every row is decided and no row waits for anything.
    for ((case, topic, _, delivered), before) in rows.iter().zip(baseline) {
        if *delivered {
            let received = emulator.received(topic).await;
            assert_eq!(
                received.as_deref().map(String::from_utf8_lossy),
                Some(std::borrow::Cow::Borrowed(marker)),
                "case {case:?}: expected the published message at the endpoint, got {:?}",
                received.as_deref().map(String::from_utf8_lossy)
            );
        } else {
            let received = emulator.received(topic).await;
            assert_eq!(
                received,
                before,
                "case {case:?}: expected NOTHING to be delivered to the endpoint, but it \
                 received {:?}",
                received.as_deref().map(String::from_utf8_lossy)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// Every way a destination request can be wrong is refused promptly, with the modelled
/// error and the family's own sentence — never with a wait and never with a 200.
#[tokio::test]
async fn a_request_no_destination_can_serve_is_refused_with_a_modelled_error() {
    let emulator = Emulator::new();
    let url = endpoint_url("refusals-endpoint");
    let created = emulator.create_http_destination(&url).await;
    assert_eq!(
        created.status, 200,
        "the destination the refusal cases are about must be created, got {} {:?}",
        created.status, created.body
    );
    let arn = created.arn();
    let unknown_arn = format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:ruledestination/http/nobody");

    // A read of an ARN this account holds no destination under.
    let missing = emulator.get_destination(&unknown_arn).await;
    assert_eq!(
        (missing.status, missing.error_code.as_deref()),
        (400, Some("InvalidRequestException")),
        "an unknown ARN must be refused, got {} {:?}",
        missing.status,
        missing.body
    );
    assert_eq!(
        missing.message(),
        format!("Topic rule destination {unknown_arn} not found"),
        "an unknown ARN must be refused in the family's own sentence, got {:?}",
        missing.message()
    );

    // An update of an ARN this account holds no destination under.
    let missing_update = emulator
        .update_destination(&unknown_arn, DESTINATION_IN_PROGRESS)
        .await;
    assert_eq!(
        (
            missing_update.status,
            missing_update.error_code.as_deref(),
            missing_update.message()
        ),
        (
            400,
            Some("InvalidRequestException"),
            format!("Topic rule destination {unknown_arn} not found")
        ),
        "an update of an unknown ARN must be refused the same way, got {} {:?}",
        missing_update.status,
        missing_update.body
    );

    // A token no outstanding challenge carries.
    let bad_token = emulator.confirm_destination("not-a-token").await;
    assert_eq!(
        (
            bad_token.status,
            bad_token.error_code.as_deref(),
            bad_token.message()
        ),
        (
            400,
            Some("InvalidRequestException"),
            "Invalid confirmation token".to_string()
        ),
        "an unknown token must be refused, got {} {:?}",
        bad_token.status,
        bad_token.body
    );

    // A status an unconfirmed destination cannot be moved to, and the two statuses no
    // caller asks for.
    // (case, the status asked for)
    for (case, status) in [
        ("enabling an unconfirmed destination", DESTINATION_ENABLED),
        ("disabling an unconfirmed destination", DESTINATION_DISABLED),
        ("a status the service sets", DESTINATION_ERROR),
        ("a status a removal sets", "DELETING"),
    ] {
        let refused = emulator.update_destination(&arn, status).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {case:?}: expected {status} to be refused, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.message(),
            format!(
                "Topic rule destination status cannot be changed from \
                 {DESTINATION_IN_PROGRESS} to {status}"
            ),
            "case {case:?}: expected the refusal to name both statuses, got {:?}",
            refused.message()
        );
        assert_eq!(
            emulator.get_destination(&arn).await.status_value(),
            DESTINATION_IN_PROGRESS,
            "case {case:?}: a refused update must change nothing"
        );
    }

    // A second create for a URL this account already holds a destination for.
    let duplicate = emulator.create_http_destination(&url).await;
    assert_eq!(
        (duplicate.status, duplicate.error_code.as_deref()),
        (409, Some("ResourceAlreadyExistsException")),
        "a second create for one confirmation URL must be refused, got {} {:?}",
        duplicate.status,
        duplicate.body
    );
    assert_eq!(
        duplicate.body["resourceArn"],
        json!(arn),
        "the refusal must name the destination that already exists, got {:?}",
        duplicate.body
    );

    // A configuration this emulator creates no destination from.
    // (case, the configuration)
    for (case, configuration) in [
        ("no destination kind at all", json!({})),
        (
            "a VPC destination",
            json!({
                "vpcConfiguration": {
                    "subnetIds": ["subnet-0123456789abcdef0"],
                    "vpcId": "vpc-0123456789abcdef0",
                    "roleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/vpc-destination"),
                },
            }),
        ),
        (
            "a confirmation URL that is no URL",
            json!({ "httpUrlConfiguration": { "confirmationUrl": "not-a-url" } }),
        ),
        (
            "an empty confirmation URL",
            json!({ "httpUrlConfiguration": { "confirmationUrl": "" } }),
        ),
        (
            "a cleartext confirmation URL",
            json!({ "httpUrlConfiguration": { "confirmationUrl": "http://example.test/iot" } }),
        ),
        (
            "a confirmation URL carrying a fragment",
            json!({ "httpUrlConfiguration": { "confirmationUrl": "https://example.test/iot#x" } }),
        ),
    ] {
        let refused = emulator.create_destination(&configuration).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {case:?}: expected the create to be refused, got {} {:?}",
            refused.status,
            refused.body
        );
        assert!(
            !refused.message().is_empty(),
            "case {case:?}: a refusal must say what was wrong, got {:?}",
            refused.body
        );
    }
}
