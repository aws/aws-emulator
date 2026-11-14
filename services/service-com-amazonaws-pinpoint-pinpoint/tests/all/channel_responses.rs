// SPDX-License-Identifier: CC0-1.0

//! The bodies a channel request answers with, end to end over HTTP.
//!
//! Every case drives the real restJson1 wire through the `MultiServiceRouter` the
//! production front door uses, so routing, deserialization, the handler,
//! serialization and the HTTP status are all exercised.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * a channel response's deprecated `Id` is the segment that names the CHANNEL in
//!   its own resource URI (`/v1/apps/{ApplicationId}/channels/<segment>`), never the
//!   application id, which the same response carries as `ApplicationId`;
//! * `HasCredential` reports whether the channel holds a credential to communicate
//!   with: a configured channel does, and one whose settings a delete has removed
//!   holds none any more;
//! * deleting a channel ARCHIVES it: the delete answers with the settings as they
//!   stood, `IsArchived` set and `Enabled` still the state they were stored with;
//! * a request that names a channel an existing application has not enabled is
//!   answered with the service's channel-not-found text, which names the channel type
//!   and the application the channel has to be enabled on — as the `NotFoundException`
//!   message of a channel read or delete, and as the per-destination `StatusMessage` of
//!   a direct send.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pinpoint_pinpoint::state;

/// The region every case in this suite signs for; the suite asserts response bodies,
/// not ARNs.
const REGION: &str = "us-east-1";

/// Every `ChannelType` the API configures as a channel resource of the application,
/// paired with the segment that names it in its resource URI. Read off the
/// `/v1/apps/{ApplicationId}/channels/...` bindings the model gives those operations
/// (`smithy_query(query="operation",operation="GetAdmChannel,GetApnsChannel,GetApnsSandboxChannel,GetApnsVoipChannel,GetApnsVoipSandboxChannel,GetBaiduChannel,GetEmailChannel,GetGcmChannel,GetSmsChannel,GetVoiceChannel")`).
const CHANNEL_PATH_SEGMENTS: &[(&str, &str)] = &[
    ("ADM", "adm"),
    ("APNS", "apns"),
    ("APNS_SANDBOX", "apns_sandbox"),
    ("APNS_VOIP", "apns_voip"),
    ("APNS_VOIP_SANDBOX", "apns_voip_sandbox"),
    ("BAIDU", "baidu"),
    ("EMAIL", "email"),
    ("GCM", "gcm"),
    ("SMS", "sms"),
    ("VOICE", "voice"),
];

/// A mounted emulator holding one shared engine, exactly as the router builds it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: it asserts service behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(service_com_amazonaws_pinpoint_pinpoint::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// Dispatch one request and return the status plus the parsed JSON body
    /// (`Value::Null` for an empty body).
    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body.map_or_else(Bytes::new, |v| {
            Bytes::from(serde_json::to_vec(&v).expect("a test body always serializes"))
        });
        let request = ::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            // The routing key: Pinpoint's sigv4 signing name is `mobiletargeting`.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/mobiletargeting/aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(payload)
            .expect("a well-formed test request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create one application and return its minted id.
    async fn create_app(&self, name: &str) -> String {
        let (status, body) = self
            .call("POST", "/v1/apps", Some(json!({"Name": name})))
            .await;
        assert_eq!(status, 201, "CreateApp must answer 201, got {body:?}");
        body["Id"]
            .as_str()
            .expect("CreateApp answers with the minted Id")
            .to_string()
    }

    async fn update_baidu_channel(&self, app: &str, request: Value) -> (u16, Value) {
        self.call(
            "PUT",
            &format!("/v1/apps/{app}/channels/baidu"),
            Some(request),
        )
        .await
    }

    async fn get_baidu_channel(&self, app: &str) -> (u16, Value) {
        self.call("GET", &format!("/v1/apps/{app}/channels/baidu"), None)
            .await
    }

    async fn delete_baidu_channel(&self, app: &str) -> (u16, Value) {
        self.call("DELETE", &format!("/v1/apps/{app}/channels/baidu"), None)
            .await
    }

    async fn send_messages(&self, app: &str, request: Value) -> (u16, Value) {
        self.call("POST", &format!("/v1/apps/{app}/messages"), Some(request))
            .await
    }
}

/// The modelled error code carried by a restJson1 error body (`__type`).
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The message member of `DirectMessageConfiguration` that carries the message for
/// `channel`, and a minimal message for it. Taken from the member list of
/// `DirectMessageConfiguration`
/// (`smithy_query(query="shape",shape="DirectMessageConfiguration")`).
fn message_configuration_for(channel: &str) -> Value {
    match channel {
        "GCM" => json!({"GCMMessage": {"Body": "gcm body"}}),
        "APNS" | "APNS_SANDBOX" | "APNS_VOIP" | "APNS_VOIP_SANDBOX" => {
            json!({"APNSMessage": {"Body": "apns body"}})
        }
        "ADM" => json!({"ADMMessage": {"Body": "adm body"}}),
        "BAIDU" => json!({"BaiduMessage": {"Body": "baidu body"}}),
        "SMS" => json!({"SMSMessage": {"Body": "sms body", "MessageType": "TRANSACTIONAL"}}),
        "VOICE" => json!({"VoiceMessage": {"Body": "voice body"}}),
        "EMAIL" => {
            json!({"EmailMessage": {"FromAddress": "sender@example.com", "SimpleEmail": {
                "Subject": {"Data": "subject"}, "TextPart": {"Data": "text"}}}})
        }
        _ => json!({"DefaultMessage": {"Body": "default body"}}),
    }
}

/// The text the service answers with for a channel `channel_type` that `app` has not
/// enabled, written out here independently of the helper under test.
fn expected_channel_not_found(channel_type: &str, app: &str) -> String {
    format!(
        "{channel_type} channel not found. Please enable the {channel_type} channel for this application {app}."
    )
}

#[test]
fn a_channels_id_is_the_segment_that_names_it_in_its_resource_uri() {
    for (channel_type, segment) in CHANNEL_PATH_SEGMENTS {
        let actual = state::channel_id(channel_type);
        assert_eq!(
            actual, *segment,
            "case {channel_type:?}: expected the channel id {segment:?}, got {actual:?}"
        );
    }
}

/// One row of the channel-write table: what `UpdateBaiduChannel` is asked for, and the
/// `Enabled` state that write leaves stored.
struct ChannelWriteCase {
    case: &'static str,
    request: Value,
    enabled: bool,
}

fn channel_write_cases() -> Vec<ChannelWriteCase> {
    vec![
        ChannelWriteCase {
            case: "enabled, credentials supplied",
            request: json!({"ApiKey": "key-1", "SecretKey": "secret-1", "Enabled": true}),
            enabled: true,
        },
        ChannelWriteCase {
            case: "disabled, credentials supplied",
            request: json!({"ApiKey": "key-2", "SecretKey": "secret-2", "Enabled": false}),
            enabled: false,
        },
        ChannelWriteCase {
            case: "no Enabled member, credentials supplied",
            request: json!({"ApiKey": "key-3", "SecretKey": "secret-3"}),
            enabled: true,
        },
        ChannelWriteCase {
            case: "a long credential",
            request: json!({"ApiKey": "k".repeat(256), "SecretKey": "s".repeat(256), "Enabled": true}),
            enabled: true,
        },
    ]
}

#[tokio::test]
async fn every_baidu_channel_body_names_the_channel_and_not_the_application_in_id() {
    let emulator = Emulator::new();

    for ChannelWriteCase {
        case,
        request,
        enabled,
    } in channel_write_cases()
    {
        // A fresh application per row, so the application id varies from row to row
        // and an Id taken from the request path would differ every time.
        let app = emulator.create_app(case).await;

        let (status, written) = emulator.update_baidu_channel(&app, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateBaiduChannel must answer 200, got {written:?}"
        );
        let (status, read_back) = emulator.get_baidu_channel(&app).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetBaiduChannel must answer 200, got {read_back:?}"
        );
        let (status, deleted) = emulator.delete_baidu_channel(&app).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteBaiduChannel must answer 200, got {deleted:?}"
        );

        for (operation, body) in [
            ("UpdateBaiduChannel", &written),
            ("GetBaiduChannel", &read_back),
            ("DeleteBaiduChannel", &deleted),
        ] {
            assert_eq!(
                body["Id"].as_str(),
                Some("baidu"),
                "case {case:?}/{operation}: expected Id \"baidu\", got {body:?}"
            );
            assert_ne!(
                body["Id"].as_str(),
                Some(app.as_str()),
                "case {case:?}/{operation}: Id must not be the application id, got {body:?}"
            );
            assert_eq!(
                body["ApplicationId"].as_str(),
                Some(app.as_str()),
                "case {case:?}/{operation}: ApplicationId carries the application, got {body:?}"
            );
            assert_eq!(
                body["Platform"].as_str(),
                Some("BAIDU"),
                "case {case:?}/{operation}: expected Platform \"BAIDU\", got {body:?}"
            );
        }

        assert_eq!(
            (
                written["HasCredential"].as_bool(),
                read_back["HasCredential"].as_bool(),
                deleted["HasCredential"].as_bool(),
            ),
            (Some(true), Some(true), Some(false)),
            "case {case:?}: a configured channel holds the credential it was given and a deleted one holds none, got {read_back:?} / {deleted:?}"
        );

        assert_eq!(
            (
                written["Enabled"].as_bool(),
                read_back["Enabled"].as_bool(),
                written["IsArchived"].as_bool(),
                read_back["IsArchived"].as_bool(),
            ),
            (Some(enabled), Some(enabled), Some(false), Some(false)),
            "case {case:?}: a live channel reports the stored Enabled state and is not archived, got {read_back:?}"
        );
    }
}

#[tokio::test]
async fn deleting_a_channel_archives_it_and_keeps_the_stored_enabled_state() {
    let emulator = Emulator::new();

    for ChannelWriteCase {
        case,
        request,
        enabled,
    } in channel_write_cases()
    {
        let app = emulator.create_app(case).await;
        let (_, written) = emulator.update_baidu_channel(&app, request).await;
        let version = written["Version"].as_i64();

        let (status, deleted) = emulator.delete_baidu_channel(&app).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteBaiduChannel must answer 200, got {deleted:?}"
        );
        assert_eq!(
            (
                deleted["IsArchived"].as_bool(),
                deleted["Enabled"].as_bool(),
                deleted["HasCredential"].as_bool(),
                deleted["Version"].as_i64(),
                deleted["Credential"].as_str(),
                deleted["CreationDate"].as_str(),
            ),
            (
                Some(true),
                Some(enabled),
                Some(false),
                version,
                written["Credential"].as_str(),
                written["CreationDate"].as_str(),
            ),
            "case {case:?}: the delete answers with the archived settings as they stood, got {deleted:?}"
        );

        // And the settings really are gone: the channel reads as absent afterwards.
        let (status, missing) = emulator.get_baidu_channel(&app).await;
        assert_eq!(
            (status, error_code(&missing).as_str()),
            (404, "NotFoundException"),
            "case {case:?}: the deleted channel must read back as a 404 NotFoundException, got {missing:?}"
        );
    }
}

#[tokio::test]
async fn a_channel_missing_on_a_live_application_is_the_channel_not_found_message() {
    let emulator = Emulator::new();

    // The application id is a per-run minted value, so each row proves the message
    // carries the application of THAT request: a never-configured channel, and one
    // whose settings a delete has already removed.
    let never_configured = emulator.create_app("never-configured").await;
    let deleted_again = emulator.create_app("deleted-channel").await;
    let (status, _) = emulator
        .update_baidu_channel(
            &deleted_again,
            json!({"ApiKey": "key", "SecretKey": "secret", "Enabled": true}),
        )
        .await;
    assert_eq!(status, 200, "UpdateBaiduChannel must answer 200");
    let (status, _) = emulator.delete_baidu_channel(&deleted_again).await;
    assert_eq!(status, 200, "DeleteBaiduChannel must answer 200");

    for (case, app) in [
        ("never configured", &never_configured),
        ("configured, then deleted", &deleted_again),
    ] {
        let expected = expected_channel_not_found("BAIDU", app);
        for (operation, (status, body)) in [
            ("GetBaiduChannel", emulator.get_baidu_channel(app).await),
            (
                "DeleteBaiduChannel",
                emulator.delete_baidu_channel(app).await,
            ),
        ] {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "NotFoundException"),
                "case {case:?}/{operation}: expected a 404 NotFoundException, got {body:?}"
            );
            assert_eq!(
                body["Message"].as_str(),
                Some(expected.as_str()),
                "case {case:?}/{operation}: expected Message {expected:?}, got {body:?}"
            );
        }
    }

    // The application-level case is a different class and keeps its own text: it is
    // the APPLICATION that is missing, so there is no channel to ask anyone to enable.
    let (status, body) = emulator
        .get_baidu_channel("00000000000000000000000000000000")
        .await;
    assert_eq!(
        (status, error_code(&body).as_str(), body["Message"].as_str()),
        (404, "NotFoundException", Some("Application not found")),
        "an application that does not exist is the application-level text, got {body:?}"
    );
}

#[tokio::test]
async fn a_send_over_a_channel_the_application_has_not_enabled_reports_that_message() {
    let emulator = Emulator::new();

    // Every channel the API configures as a channel of the application: a fresh
    // application per row, so the application id in the message is that row's.
    for (channel_type, _) in CHANNEL_PATH_SEGMENTS {
        let app = emulator.create_app(channel_type).await;
        let address = format!("{}-address", channel_type.to_ascii_lowercase());
        let (status, body) = emulator
            .send_messages(
                &app,
                json!({
                    "Addresses": {address.clone(): {"ChannelType": channel_type}},
                    "MessageConfiguration": message_configuration_for(channel_type),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {channel_type:?}: SendMessages answers 200 overall, got {body:?}"
        );
        let outcome = body["Result"][&address].clone();
        let expected = expected_channel_not_found(channel_type, &app);
        assert_eq!(
            (
                outcome["DeliveryStatus"].as_str(),
                outcome["StatusCode"].as_i64(),
                outcome["StatusMessage"].as_str(),
            ),
            (Some("PERMANENT_FAILURE"), Some(404), Some(expected.as_str())),
            "case {channel_type:?}: expected StatusMessage {expected:?}, got {outcome:?}"
        );
    }

    // A channel that IS configured but disabled is the same class: the application has
    // not enabled it.
    let app = emulator.create_app("configured-but-disabled").await;
    let (status, _) = emulator
        .update_baidu_channel(
            &app,
            json!({"ApiKey": "key", "SecretKey": "secret", "Enabled": false}),
        )
        .await;
    assert_eq!(status, 200, "UpdateBaiduChannel must answer 200");
    let address = "baidu-token";
    let (_, body) = emulator
        .send_messages(
            &app,
            json!({
                "Addresses": {address: {"ChannelType": "BAIDU"}},
                "MessageConfiguration": message_configuration_for("BAIDU"),
            }),
        )
        .await;
    let outcome = body["Result"][address].clone();
    let expected = expected_channel_not_found("BAIDU", &app);
    assert_eq!(
        outcome["StatusMessage"].as_str(),
        Some(expected.as_str()),
        "case \"configured but disabled\": expected StatusMessage {expected:?}, got {outcome:?}"
    );

    // And a send over an enabled channel is not refused with that text at all.
    let (status, _) = emulator
        .update_baidu_channel(
            &app,
            json!({"ApiKey": "key", "SecretKey": "secret", "Enabled": true}),
        )
        .await;
    assert_eq!(status, 200, "UpdateBaiduChannel must answer 200");
    let (_, body) = emulator
        .send_messages(
            &app,
            json!({
                "Addresses": {address: {"ChannelType": "BAIDU"}},
                "MessageConfiguration": message_configuration_for("BAIDU"),
            }),
        )
        .await;
    let outcome = body["Result"][address].clone();
    assert_eq!(
        (
            outcome["DeliveryStatus"].as_str(),
            outcome["StatusCode"].as_i64(),
        ),
        (Some("SUCCESSFUL"), Some(200)),
        "case \"enabled\": the same send is accepted once the channel is enabled, got {outcome:?}"
    );
}
