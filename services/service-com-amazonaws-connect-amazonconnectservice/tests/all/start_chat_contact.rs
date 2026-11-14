// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `StartChatContact`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator serves with — with the Connect Participant Service
//! mounted beside Amazon Connect on ONE engine, because a started chat is only real if
//! that service serves it.
//!
//! The rule every case below asserts: a started chat is one chat held once across both
//! halves. `StartChatContact` mints the contact, the participant and the participant
//! token, records the chat on its instance, and registers the chat SESSION with the
//! participant service — so that token resolves there to this contact, this participant
//! and this display name, an `InitialMessage` is the transcript's first item, a
//! persistent chat carries its past session's transcript, and the response reports
//! exactly the connection information the request asked for and nothing else.
//!
//! Each table spans what the rule generalises over (the display name, the content type,
//! the requested connection types, the chat duration, the rehydration type), never one
//! recorded call.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use engine::InMemoryEngine;
use serde_json::{json, Value};
use service_com_amazonaws_connect_amazonconnectservice::state::{
    self as connect_state, ChatContactRecord, CHAT_CONTACTS_PER_INSTANCE_QUOTA,
    MAX_ATTRIBUTES_BYTES,
};
use service_com_amazonaws_connect_amazonconnectservice::EmulatedService as Connect;
use service_com_amazonaws_connectparticipant_amazonconnectparticipantservicelambda::EmulatedService as Participant;
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{connect_managed, looks_like_a_uuid};

/// The endpoint host every request addresses, and so the host a reported websocket URL
/// names.
const HOST: &str = "connect.us-east-1.amazonaws.com";

/// The Region the credential scope names, which is also the Region the handlers mint in.
const REGION: &str = "us-east-1";

/// One answered request: the HTTP status, the `x-amzn-errortype` code (empty on a
/// success) and the parsed JSON body.
struct Answer {
    status: u16,
    error_code: String,
    body: Value,
}

/// The two services of one chat, mounted on one shared engine: Amazon Connect, which
/// starts the chat, and the Connect Participant Service, which serves it.
struct Chat {
    router: MultiServiceRouter,
    engine: Arc<InMemoryEngine>,
}

impl Chat {
    fn new() -> Self {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(Connect::new(Arc::clone(&engine)));
        router.register_service(Participant::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// The shared store this emulator's handlers read, so a suite can seed the records a
    /// bound is reached at instead of driving a thousand requests to reach it.
    fn engine(&self) -> &InMemoryEngine {
        &self.engine
    }

    /// Send `body` to `uri`, signed for `signing_name` and carrying `bearer` as the
    /// participant/connection token when one is given.
    async fn send(
        &self,
        method: &str,
        uri: &str,
        signing_name: &str,
        bearer: Option<&str>,
        body: Option<Value>,
    ) -> Answer {
        let payload = body.map_or_else(Bytes::new, |value| {
            Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
        });
        let mut request = http::Request::builder()
            .method(method)
            .uri(format!("https://{HOST}{uri}"))
            .header("host", HOST)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=TESTKEY/20240101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host, Signature=abc"
                ),
            );
        if let Some(bearer) = bearer {
            request = request.header("x-amz-bearer", bearer);
        }
        let response = self
            .router
            .dispatch(request.body(payload).expect("a well-formed test request"))
            .await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error_code,
            body,
        }
    }

    /// An instance with its default flows, and the id of its inbound sample flow — what
    /// a chat is started against.
    async fn instance(&self, alias: &str) -> (String, String) {
        let created = self
            .send("PUT", "/instance", "connect", None, Some(connect_managed(alias)))
            .await;
        assert_eq!(
            created.status, 200,
            "CreateInstance {alias}: got {}",
            created.body
        );
        let instance_id = created.body["Id"].as_str().expect("Id").to_string();
        let flows = self
            .send(
                "GET",
                &format!("/contact-flows-summary/{instance_id}?contactFlowTypes=CONTACT_FLOW"),
                "connect",
                None,
                None,
            )
            .await;
        assert_eq!(
            flows.status, 200,
            "ListContactFlows {instance_id}: got {}",
            flows.body
        );
        let flow_id = flows.body["ContactFlowSummaryList"][0]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("the instance has no CONTACT_FLOW flow: {}", flows.body))
            .to_string();
        (instance_id, flow_id)
    }

    async fn start_chat(&self, body: Value) -> Answer {
        self.send("PUT", "/contact/chat", "connect", None, Some(body))
            .await
    }

    /// `CreateParticipantConnection` on the participant service, returning its answer.
    async fn participant_connection(&self, participant_token: &str, types: Value) -> Answer {
        self.send(
            "POST",
            "/participant/connection",
            "execute-api",
            Some(participant_token),
            Some(json!({"Type": types})),
        )
        .await
    }

    /// The connection token a participant token opens a connection with.
    async fn connection_token(&self, participant_token: &str) -> String {
        let answer = self
            .participant_connection(participant_token, json!(["CONNECTION_CREDENTIALS"]))
            .await;
        assert_eq!(
            answer.status, 200,
            "CreateParticipantConnection: got {}",
            answer.body
        );
        answer.body["ConnectionCredentials"]["ConnectionToken"]
            .as_str()
            .unwrap_or_else(|| panic!("no ConnectionToken: {}", answer.body))
            .to_string()
    }

    /// `SendMessage` on the participant service.
    async fn send_message(&self, connection_token: &str, content: &str) -> Answer {
        self.send(
            "POST",
            "/participant/message",
            "execute-api",
            Some(connection_token),
            Some(json!({"ContentType": "text/plain", "Content": content})),
        )
        .await
    }

    /// The whole transcript a connection token reaches, oldest item first.
    async fn transcript(&self, connection_token: &str) -> Vec<Value> {
        let answer = self
            .send(
                "POST",
                "/participant/transcript",
                "execute-api",
                Some(connection_token),
                Some(json!({"SortOrder": "ASCENDING", "MaxResults": 100})),
            )
            .await;
        assert_eq!(answer.status, 200, "GetTranscript: got {}", answer.body);
        answer.body["Transcript"]
            .as_array()
            .unwrap_or_else(|| panic!("no Transcript: {}", answer.body))
            .clone()
    }
}

/// A minimal valid `StartChatContact` body for `instance_id` / `flow_id`.
fn start_body(instance_id: &str, flow_id: &str, display_name: &str) -> Value {
    json!({
        "InstanceId": instance_id,
        "ContactFlowId": flow_id,
        "ParticipantDetails": {"DisplayName": display_name},
    })
}

/// A started chat, as the response reports it.
fn started(answer: &Answer) -> (String, String, String) {
    assert_eq!(answer.status, 200, "StartChatContact: got {}", answer.body);
    (
        answer.body["ContactId"]
            .as_str()
            .unwrap_or_else(|| panic!("no ContactId: {}", answer.body))
            .to_string(),
        answer.body["ParticipantId"]
            .as_str()
            .unwrap_or_else(|| panic!("no ParticipantId: {}", answer.body))
            .to_string(),
        answer.body["ParticipantToken"]
            .as_str()
            .unwrap_or_else(|| panic!("no ParticipantToken: {}", answer.body))
            .to_string(),
    )
}

/// Every display name a chat can be started for, spanning what the rule generalises
/// over: the shortest and longest the modelled `DisplayName` admits (length 1..256),
/// plus non-ASCII and punctuation.
fn display_names() -> Vec<(&'static str, String)> {
    vec![
        ("one character", "C".to_string()),
        ("an ordinary name", "Customer Pat".to_string()),
        ("non-ASCII", "Ана Иванова 🙂".to_string()),
        ("punctuation", "O'Brien-Smith, Jr.".to_string()),
        ("the longest the model admits", "n".repeat(256)),
    ]
}

/// The content types ONE chat message may carry, from `ChatMessage$ContentType`
/// (smithy_query(query="shape",shape="ChatMessage")): "Supported types are text/plain,
/// text/markdown, application/json, and
/// application/vnd.amazonaws.connect.message.interactive.response."
const CHAT_MESSAGE_CONTENT_TYPES: [&str; 4] = [
    "text/plain",
    "text/markdown",
    "application/json",
    "application/vnd.amazonaws.connect.message.interactive.response",
];

#[tokio::test]
async fn a_started_chat_is_the_session_the_participant_service_serves() {
    let chat = Chat::new();
    let (instance_id, flow_id) = chat.instance("chat-session").await;

    for (case, display_name) in display_names() {
        let answer = chat
            .start_chat(start_body(&instance_id, &flow_id, &display_name))
            .await;
        let (contact_id, participant_id, participant_token) = started(&answer);
        assert!(
            looks_like_a_uuid(&contact_id) && looks_like_a_uuid(&participant_id),
            "case {case:?}: expected minted UUID identifiers, got contact {contact_id:?} and \
             participant {participant_id:?}"
        );
        assert!(
            participant_token.len() > contact_id.len() && participant_token != contact_id,
            "case {case:?}: expected an opaque participant token distinct from the contact id, \
             got {participant_token:?}"
        );
        assert!(
            answer.body.get("ConnectionCredentials").is_none()
                && answer.body.get("Websocket").is_none()
                && answer.body.get("StreamingId").is_none()
                && answer.body.get("ContinuedFromContactId").is_none(),
            "case {case:?}: expected no connection, streaming or continuation information for a \
             request that asked for none, got {}",
            answer.body
        );

        // The participant service serves THIS chat for that token: the item it appends
        // carries the contact, the participant and the display name the chat was started
        // with — none of which its own API carries.
        let connection_token = chat.connection_token(&participant_token).await;
        let sent = chat.send_message(&connection_token, "hello").await;
        assert_eq!(sent.status, 200, "case {case:?}: SendMessage got {}", sent.body);
        let transcript = chat.transcript(&connection_token).await;
        assert_eq!(
            transcript.len(),
            1,
            "case {case:?}: expected the one sent message in the transcript, got {transcript:?}"
        );
        let item = &transcript[0];
        let reported = (
            item["ContactId"].as_str().unwrap_or_default(),
            item["ParticipantId"].as_str().unwrap_or_default(),
            item["DisplayName"].as_str().unwrap_or_default(),
            item["ParticipantRole"].as_str().unwrap_or_default(),
        );
        assert_eq!(
            reported,
            (
                contact_id.as_str(),
                participant_id.as_str(),
                display_name.as_str(),
                "CUSTOMER"
            ),
            "case {case:?}: expected the transcript item attributed to the started chat's \
             participant, got {item:?}"
        );
    }
}

#[tokio::test]
async fn an_initial_message_is_the_chats_first_transcript_item() {
    let chat = Chat::new();
    let (instance_id, flow_id) = chat.instance("chat-initial-message").await;

    // (case, the InitialMessage sent, how many items the transcript then holds)
    let mut rows: Vec<(String, Option<Value>, usize)> = vec![("no initial message".to_string(), None, 0)];
    for content_type in CHAT_MESSAGE_CONTENT_TYPES {
        rows.push((
            format!("an initial {content_type} message"),
            Some(json!({"ContentType": content_type, "Content": format!("{{\"hello\":\"{content_type}\"}}")})),
            1,
        ));
    }

    for (case, initial_message, expected_items) in rows {
        let mut body = start_body(&instance_id, &flow_id, "Customer Pat");
        if let Some(message) = initial_message.clone() {
            body["InitialMessage"] = message;
        }
        let answer = chat.start_chat(body).await;
        let (contact_id, participant_id, participant_token) = started(&answer);
        let connection_token = chat.connection_token(&participant_token).await;
        let transcript = chat.transcript(&connection_token).await;
        assert_eq!(
            transcript.len(),
            expected_items,
            "case {case:?}: expected {expected_items} transcript item(s), got {transcript:?}"
        );
        if let Some(message) = initial_message {
            let item = &transcript[0];
            let reported = (
                item["Content"].as_str().unwrap_or_default(),
                item["ContentType"].as_str().unwrap_or_default(),
                item["Type"].as_str().unwrap_or_default(),
                item["ParticipantId"].as_str().unwrap_or_default(),
                item["ContactId"].as_str().unwrap_or_default(),
            );
            assert_eq!(
                reported,
                (
                    message["Content"].as_str().unwrap_or_default(),
                    message["ContentType"].as_str().unwrap_or_default(),
                    "MESSAGE",
                    participant_id.as_str(),
                    contact_id.as_str()
                ),
                "case {case:?}: expected the initial message as the chat's first item, got \
                 {item:?}"
            );
        }
    }
}

#[tokio::test]
async fn connection_information_is_reported_only_for_the_types_requested() {
    let chat = Chat::new();
    let (instance_id, flow_id) = chat.instance("chat-connection-types").await;

    // (case, ConnectionTypes, expects a connection token, expects a websocket URL).
    // Every member of `ConnectionType`
    // (smithy_query(query="shape",shape="ConnectionType")) appears: the member's own doc
    // says "WEBSOCKET and CONNECTION_CREDENTIALS are the values this operation acts on.
    // No other value returns connection information."
    let rows: [(&str, Option<Value>, bool, bool); 7] = [
        ("the member absent", None, false, false),
        (
            "CONNECTION_CREDENTIALS alone",
            Some(json!(["CONNECTION_CREDENTIALS"])),
            true,
            false,
        ),
        ("WEBSOCKET alone", Some(json!(["WEBSOCKET"])), false, true),
        (
            "both values",
            Some(json!(["WEBSOCKET", "CONNECTION_CREDENTIALS"])),
            true,
            true,
        ),
        (
            "AUTHENTICATION_SESSION, which returns none",
            Some(json!(["AUTHENTICATION_SESSION"])),
            false,
            false,
        ),
        (
            "WEBRTC_CONNECTION, which returns none",
            Some(json!(["WEBRTC_CONNECTION"])),
            false,
            false,
        ),
        (
            "a connection type beside one that returns none",
            Some(json!(["WEBRTC_CONNECTION", "CONNECTION_CREDENTIALS"])),
            true,
            false,
        ),
    ];

    for (case, connection_types, expects_token, expects_websocket) in rows {
        let mut body = start_body(&instance_id, &flow_id, "Customer Pat");
        if let Some(types) = connection_types {
            body["ConnectionTypes"] = types;
        }
        let answer = chat.start_chat(body).await;
        let (contact_id, _, _) = started(&answer);
        let token = answer.body["ConnectionCredentials"]["ConnectionToken"].as_str();
        let url = answer.body["Websocket"]["Url"].as_str();
        assert_eq!(
            (token.is_some(), url.is_some()),
            (expects_token, expects_websocket),
            "case {case:?}: expected (connection token, websocket URL) presence \
             ({expects_token}, {expects_websocket}), got {}",
            answer.body
        );
        if let Some(url) = url {
            assert!(
                url.starts_with(&format!("wss://{HOST}/participant/")),
                "case {case:?}: expected a websocket URL on the host the request reached, got \
                 {url:?}"
            );
        }
        if let Some(token) = token {
            // The token is BACKED: it authenticates against the participant service as
            // this contact's connection, so a message sent with it lands in this chat.
            let sent = chat.send_message(token, "sent with the reported token").await;
            assert_eq!(
                sent.status, 200,
                "case {case:?}: expected the reported connection token to authenticate \
                 SendMessage, got {}",
                sent.body
            );
            let transcript = chat.transcript(token).await;
            assert_eq!(
                transcript
                    .iter()
                    .map(|item| item["ContactId"].as_str().unwrap_or_default().to_string())
                    .collect::<BTreeSet<String>>(),
                BTreeSet::from([contact_id.clone()]),
                "case {case:?}: expected the token to reach this contact's transcript, got \
                 {transcript:?}"
            );
        }
        if let (Some(token), Some(url)) = (token, url) {
            assert!(
                url.ends_with(token),
                "case {case:?}: expected one connection token in both answers, got {url:?} and \
                 {token:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_repeated_client_token_reports_the_contact_it_already_started() {
    let chat = Chat::new();
    let (instance_id, flow_id) = chat.instance("chat-idempotency").await;

    // (case, the ClientToken sent, whether it must report the first call's contact)
    let rows: [(&str, Option<&str>, bool); 4] = [
        ("the first call's token", Some("token-a"), true),
        ("that same token again", Some("token-a"), true),
        ("a different token", Some("token-b"), false),
        ("no token at all", None, false),
    ];

    let mut first: Option<(String, String, String)> = None;
    for (case, client_token, repeats) in rows {
        let mut body = start_body(&instance_id, &flow_id, "Customer Pat");
        if let Some(token) = client_token {
            body["ClientToken"] = json!(token);
        }
        let answer = chat.start_chat(body).await;
        let chat_started = started(&answer);
        match (&first, repeats) {
            (None, _) => first = Some(chat_started),
            (Some(original), true) => assert_eq!(
                &chat_started, original,
                "case {case:?}: expected the contact the first call started, got {chat_started:?}"
            ),
            (Some(original), false) => assert!(
                chat_started.0 != original.0
                    && chat_started.1 != original.1
                    && chat_started.2 != original.2,
                "case {case:?}: expected a second chat, got the first one back: {chat_started:?}"
            ),
        }
    }
}

#[tokio::test]
async fn a_persistent_chat_continues_the_session_it_names() {
    // (case, RehydrationType, whether the source contact is the past session's initial
    // contact). Both members of `RehydrationType`
    // (smithy_query(query="shape",shape="RehydrationType")) are covered:
    // `FROM_SEGMENT` rehydrates the contact named, `ENTIRE_PAST_SESSION` the newest
    // contact of that contact's session.
    let rows: [(&str, &str); 2] = [
        ("FROM_SEGMENT", "FROM_SEGMENT"),
        ("ENTIRE_PAST_SESSION", "ENTIRE_PAST_SESSION"),
    ];

    for (case, rehydration_type) in rows {
        let chat = Chat::new();
        let (instance_id, flow_id) = chat.instance("chat-persistent").await;

        let past = chat
            .start_chat(start_body(&instance_id, &flow_id, "Customer Pat"))
            .await;
        let (past_contact_id, _, past_participant_token) = started(&past);
        let past_connection = chat.connection_token(&past_participant_token).await;
        chat.send_message(&past_connection, "said in the past chat")
            .await;

        let mut body = start_body(&instance_id, &flow_id, "Customer Pat");
        body["PersistentChat"] = json!({
            "RehydrationType": rehydration_type,
            "SourceContactId": past_contact_id,
        });
        let answer = chat.start_chat(body).await;
        let (contact_id, _, participant_token) = started(&answer);
        assert_eq!(
            answer.body["ContinuedFromContactId"].as_str(),
            Some(past_contact_id.as_str()),
            "case {case:?}: expected the past contact reported as ContinuedFromContactId, got {}",
            answer.body
        );
        assert_ne!(
            contact_id, past_contact_id,
            "case {case:?}: expected a new contact for the rehydrated chat"
        );

        let connection_token = chat.connection_token(&participant_token).await;
        let transcript = chat.transcript(&connection_token).await;
        assert_eq!(
            transcript
                .iter()
                .map(|item| item["Content"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>(),
            vec!["said in the past chat".to_string()],
            "case {case:?}: expected the past session's transcript rehydrated into the new \
             chat, got {transcript:?}"
        );
    }
}

#[tokio::test]
async fn a_streaming_configuration_reports_the_streaming_it_recorded() {
    let chat = Chat::new();
    let (instance_id, flow_id) = chat.instance("chat-streaming").await;

    // (case, the ChatStreamingConfiguration sent, whether a StreamingId is reported)
    let rows: [(&str, Option<&str>, bool); 3] = [
        ("no streaming configuration", None, false),
        (
            "a standard SNS topic",
            Some("arn:aws:sns:us-east-1:000000000000:chat-transcripts"),
            true,
        ),
        (
            "another SNS topic",
            Some("arn:aws:sns:us-east-1:000000000000:other-transcripts"),
            true,
        ),
    ];

    let mut reported: BTreeSet<String> = BTreeSet::new();
    for (case, endpoint_arn, expects_streaming_id) in rows {
        let mut body = start_body(&instance_id, &flow_id, "Customer Pat");
        if let Some(arn) = endpoint_arn {
            body["ChatStreamingConfiguration"] = json!({"StreamingEndpointArn": arn});
        }
        let answer = chat.start_chat(body).await;
        started(&answer);
        let streaming_id = answer.body["StreamingId"].as_str().map(str::to_string);
        assert_eq!(
            streaming_id.is_some(),
            expects_streaming_id,
            "case {case:?}: expected a StreamingId present={expects_streaming_id}, got {}",
            answer.body
        );
        if let Some(streaming_id) = streaming_id {
            assert!(
                looks_like_a_uuid(&streaming_id) && reported.insert(streaming_id.clone()),
                "case {case:?}: expected a fresh minted StreamingId, got {streaming_id:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_chat_duration_is_admitted_inside_its_modelled_bounds() {
    let chat = Chat::new();
    let (instance_id, flow_id) = chat.instance("chat-duration").await;

    // `ChatDurationInMinutes` carries range 60..10080
    // (smithy_query(query="operation",operation="StartChatContact")): each published
    // bound is admitted and one past it on either side is refused.
    let rows: [(&str, Option<i64>, u16); 6] = [
        ("the member absent", None, 200),
        ("the minimum", Some(60), 200),
        ("one under the minimum", Some(59), 400),
        ("the maximum", Some(10_080), 200),
        ("one over the maximum", Some(10_081), 400),
        ("an ordinary duration", Some(1_440), 200),
    ];

    for (case, minutes, expected_status) in rows {
        let mut body = start_body(&instance_id, &flow_id, "Customer Pat");
        if let Some(minutes) = minutes {
            body["ChatDurationInMinutes"] = json!(minutes);
        }
        let answer = chat.start_chat(body).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} {}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn attributes_are_admitted_up_to_the_documented_byte_limit() {
    let chat = Chat::new();
    let (instance_id, flow_id) = chat.instance("chat-attributes").await;

    // `Attributes`: "There can be up to 32,768 UTF-8 bytes across all key-value pairs per
    // contact" (smithy_query(query="doc",shape="StartChatContactRequest",member=
    // "Attributes")). The published bound is admitted and one byte past it refused; the
    // documented key character set is spanned too.
    let key = "a-key_1";
    let filling = MAX_ATTRIBUTES_BYTES - key.len();
    let rows: [(&str, Value, u16); 5] = [
        ("no attributes", json!({}), 200),
        ("one ordinary pair", json!({"Department": "support"}), 200),
        (
            "every character class a key may carry",
            json!({"a-b_C9": "value"}),
            200,
        ),
        (
            "exactly the documented limit",
            json!({key: "v".repeat(filling)}),
            200,
        ),
        (
            "one byte past the documented limit",
            json!({key: "v".repeat(filling + 1)}),
            400,
        ),
    ];

    for (case, attributes, expected_status) in rows {
        let mut body = start_body(&instance_id, &flow_id, "Customer Pat");
        body["Attributes"] = attributes;
        let answer = chat.start_chat(body).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} {}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn an_instance_holding_its_chat_limit_refuses_another_running_chat() {
    let chat = Chat::new();
    let (instance_id, flow_id) = chat.instance("chat-quota").await;
    let store = connect_state::service_state(chat.engine()).expect("the Amazon Connect store");

    // (case, whether the seeded chats are still running, expected status, expected code).
    // The store's bound is reached by seeding it: a chat still inside its own window
    // occupies a slot, while one the clock has already ended is dropped to make room.
    let rows: [(&str, bool, u16, &str); 2] = [
        ("every seeded chat still running", true, 429, "LimitExceededException"),
        ("every seeded chat already ended", false, 200, ""),
    ];

    for (case, running, expected_status, expected_code) in rows {
        {
            let mut state = store.lock();
            let instance = state
                .instances
                .get_mut(&instance_id)
                .expect("the created instance");
            instance.chat_contacts.clear();
            for serial in 0..CHAT_CONTACTS_PER_INSTANCE_QUOTA {
                let id = format!("seeded-{serial}");
                instance.chat_contacts.insert(
                    id.clone(),
                    seeded_chat(&id, running, SystemTime::now()),
                );
            }
        }
        let answer = chat
            .start_chat(start_body(&instance_id, &flow_id, "Customer Pat"))
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code:?}, got {}",
            answer.body
        );
    }
}

/// One chat record the store is seeded with: still inside its window when `running`, or
/// already past its end when not.
fn seeded_chat(id: &str, running: bool, now: SystemTime) -> ChatContactRecord {
    let window = Duration::from_secs(60 * 60);
    ChatContactRecord {
        id: id.to_string(),
        initial_contact_id: id.to_string(),
        continued_from_contact_id: None,
        related_contact_id: None,
        participant_id: format!("{id}-participant"),
        participant_token: format!("{id}-token"),
        display_name: "Seeded Customer".to_string(),
        attributes: std::collections::BTreeMap::new(),
        segment_attributes: std::collections::BTreeMap::new(),
        customer_id: None,
        disconnect_on_customer_exit: Vec::new(),
        participant_response_mode: None,
        supported_messaging_content_types: vec!["text/plain".to_string()],
        streaming: None,
        client_token: None,
        started_at: now - window,
        ends_at: if running { now + window } else { now - window },
    }
}

#[tokio::test]
async fn a_request_that_breaks_a_rule_starts_no_chat() {
    let chat = Chat::new();
    let (instance_id, flow_id) = chat.instance("chat-refusals").await;
    let (other_instance_id, _) = chat.instance("chat-refusals-other").await;

    // (case, how the body differs, expected status, expected error code). The enum-typed
    // members (`ConnectionTypes`, `DisconnectOnCustomerExit`, `RehydrationType`) are
    // refused by the boundary validator as a `ValidationException`; the rules the model
    // states only in prose are refused by the handler as the error this operation models
    // for them.
    let rows: [(&str, Value, u16, &str); 12] = [
        (
            "an instance that does not exist",
            json!({"InstanceId": "00000000-0000-4000-8000-000000000000"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a flow that does not exist",
            json!({"ContactFlowId": "00000000-0000-4000-8000-000000000000"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a flow of another instance",
            json!({"InstanceId": other_instance_id, "ContactFlowId": flow_id}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "supported content types without text/plain",
            json!({"SupportedMessagingContentTypes": ["text/markdown"]}),
            400,
            "InvalidParameterException",
        ),
        (
            "an unsupported content type",
            json!({"SupportedMessagingContentTypes": ["text/plain", "text/html"]}),
            400,
            "InvalidParameterException",
        ),
        (
            "an initial message of an unsupported content type",
            json!({"InitialMessage": {"ContentType": "text/html", "Content": "<p>hi</p>"}}),
            400,
            "InvalidParameterException",
        ),
        (
            "a connection type the enum does not carry",
            json!({"ConnectionTypes": ["WEBSOCKETS"]}),
            400,
            "ValidationException",
        ),
        (
            "a disconnect-on-exit participant type the enum does not carry",
            json!({"DisconnectOnCustomerExit": ["CUSTOMER"]}),
            400,
            "ValidationException",
        ),
        (
            "an attribute key outside the documented character set",
            json!({"Attributes": {"not a key": "value"}}),
            400,
            "InvalidParameterException",
        ),
        (
            "a persistent chat with no rehydration type",
            json!({"PersistentChat": {"SourceContactId": "00000000-0000-4000-8000-000000000000"}}),
            400,
            "InvalidParameterException",
        ),
        (
            "a persistent chat with no source contact",
            json!({"PersistentChat": {"RehydrationType": "ENTIRE_PAST_SESSION"}}),
            400,
            "InvalidParameterException",
        ),
        (
            "both RelatedContactId and PersistentChat",
            json!({
                "RelatedContactId": "00000000-0000-4000-8000-000000000000",
                "PersistentChat": {"RehydrationType": "FROM_SEGMENT"},
            }),
            400,
            "InvalidRequestException",
        ),
    ];
    for (case, overrides, expected_status, expected_code) in rows {
        let mut body = start_body(&instance_id, &flow_id, "Customer Pat");
        for (member, value) in overrides.as_object().expect("an object of overrides") {
            body[member] = value.clone();
        }
        let answer = chat.start_chat(body.clone()).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (expected_status, expected_code),
            "case {case:?} ({body}): expected HTTP {expected_status} {expected_code}, got {}",
            answer.body
        );
        assert!(
            answer.body.get("ParticipantToken").is_none(),
            "case {case:?}: a refusal must start no chat, got {}",
            answer.body
        );
    }

    // A past chat that was never started cannot be rehydrated either, and neither can a
    // rehydration type the enum does not carry.
    let mut body = start_body(&instance_id, &flow_id, "Customer Pat");
    body["PersistentChat"] =
        json!({"RehydrationType": "PART_OF_THE_PAST", "SourceContactId": "11111111-1111-4111-8111-111111111111"});
    let answer = chat.start_chat(body).await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (400, "ValidationException"),
        "case a rehydration type the enum does not carry: got {} {}",
        answer.status,
        answer.body
    );
    let mut body = start_body(&instance_id, &flow_id, "Customer Pat");
    body["PersistentChat"] =
        json!({"RehydrationType": "FROM_SEGMENT", "SourceContactId": "11111111-1111-4111-8111-111111111111"});
    let answer = chat.start_chat(body).await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case a persistent chat from an unknown contact: got {} {}",
        answer.status,
        answer.body
    );
}

/// One chat this suite started, and what the participant service must serve for it alone.
struct OwnChat {
    case: &'static str,
    instance_id: String,
    contact_id: String,
    participant_id: String,
    connection_token: String,
    display_name: &'static str,
    message: &'static str,
}

#[tokio::test]
async fn every_started_chat_is_its_own_session() {
    let chat = Chat::new();
    let (first_instance, first_flow) = chat.instance("chat-isolation-one").await;
    let (second_instance, second_flow) = chat.instance("chat-isolation-two").await;

    // (case, the instance the chat is started on, that instance's flow, the display name,
    // the one message this chat alone carries). The rows differ along what the rule
    // generalises over: how many chats one instance holds, whether two chats sit on one
    // instance or on different ones, and whether two chats share a display name — "the
    // participantId for a chat participant is the same throughout the chat lifecycle"
    // (`StartChatContactResponse$ParticipantId`), so it belongs to ONE chat and to no
    // other.
    let rows: [(&str, &str, &str, &'static str, &'static str); 5] = [
        (
            "the first chat of an instance",
            first_instance.as_str(),
            first_flow.as_str(),
            "Customer Pat",
            "said in the first chat",
        ),
        (
            "a second chat of that instance",
            first_instance.as_str(),
            first_flow.as_str(),
            "Customer Quinn",
            "said in the second chat",
        ),
        (
            "a third chat under a display name already in use",
            first_instance.as_str(),
            first_flow.as_str(),
            "Customer Pat",
            "said in the third chat",
        ),
        (
            "the first chat of another instance",
            second_instance.as_str(),
            second_flow.as_str(),
            "Customer Pat",
            "said in the fourth chat",
        ),
        (
            "a second chat of that other instance",
            second_instance.as_str(),
            second_flow.as_str(),
            "Ана Иванова 🙂",
            "said in the fifth chat",
        ),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    let mut own_chats: Vec<OwnChat> = Vec::new();
    for (case, instance_id, flow_id, display_name, message) in rows {
        let answer = chat
            .start_chat(start_body(instance_id, flow_id, display_name))
            .await;
        let (contact_id, participant_id, participant_token) = started(&answer);
        for (member, value) in [
            ("ContactId", &contact_id),
            ("ParticipantId", &participant_id),
            ("ParticipantToken", &participant_token),
        ] {
            assert!(
                minted.insert(value.clone()),
                "case {case:?}: {member} {value:?} was already minted for another chat"
            );
        }
        let connection_token = chat.connection_token(&participant_token).await;
        let sent = chat.send_message(&connection_token, message).await;
        assert_eq!(
            sent.status, 200,
            "case {case:?}: SendMessage got {}",
            sent.body
        );
        own_chats.push(OwnChat {
            case,
            instance_id: instance_id.to_string(),
            contact_id,
            participant_id,
            connection_token,
            display_name,
            message,
        });
    }

    // Each chat's token reaches that chat's transcript and nothing of any other chat's,
    // attributed to the participant and display name THAT start minted.
    for own in &own_chats {
        let transcript = chat.transcript(&own.connection_token).await;
        let reported: Vec<(&str, &str, &str, &str)> = transcript
            .iter()
            .map(|item| {
                (
                    item["Content"].as_str().unwrap_or_default(),
                    item["ContactId"].as_str().unwrap_or_default(),
                    item["ParticipantId"].as_str().unwrap_or_default(),
                    item["DisplayName"].as_str().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            reported,
            vec![(
                own.message,
                own.contact_id.as_str(),
                own.participant_id.as_str(),
                own.display_name
            )],
            "case {:?}: expected this chat's own message alone in its transcript, got \
             {transcript:?}",
            own.case
        );
    }

    // And the control-plane half holds each chat on the instance it was started on alone.
    let store = connect_state::service_state(chat.engine()).expect("the Amazon Connect store");
    let state = store.lock();
    for own in &own_chats {
        for (instance_id, instance) in &state.instances {
            let holds = instance.chat_contacts.contains_key(&own.contact_id);
            assert_eq!(
                holds,
                instance_id == &own.instance_id,
                "case {:?}: expected the chat recorded on instance {:?} alone, instance \
                 {instance_id:?} holds it = {holds}",
                own.case,
                own.instance_id
            );
        }
    }
}
