// SPDX-License-Identifier: CC0-1.0

//! Direct message sends (`SendMessages`) and the Baidu channel they are gated on,
//! end to end over HTTP.
//!
//! Every case drives the real restJson1 wire through the `MultiServiceRouter` the
//! production front door uses, so routing, deserialization, the handler,
//! serialization and the HTTP status are all exercised.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * a destination's `DeliveryStatus`/`StatusCode` is computed from the application's
//!   stored state — the channel's enablement, the endpoint's address, opt-out and
//!   status — and from whether the request carries a message the destination's channel
//!   can take, so the SAME send lands differently before and after a write that
//!   changes one of those facts;
//! * a channel the API configures as a channel of the application is delivered over
//!   only while that channel is enabled for it; a `ChannelType` with no channel
//!   resource of its own is not gated on one;
//! * a channel write is read back member for member through `GetBaiduChannel`, and is
//!   gone from it after `DeleteBaiduChannel`;
//! * a message id is minted only for an accepted delivery, is unique per delivery, and
//!   every accepted send is recorded in the application's bounded send log.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every case in this suite signs for; the suite asserts delivery
/// behaviour, not ARNs.
const REGION: &str = "us-east-1";

/// The channels the API configures as a channel resource of the application: the
/// `ChannelType` values the model gives their own
/// `/v1/apps/{ApplicationId}/channels/...` operations, as this crate's OPERATIONS.md
/// lists them (`smithy_query(query="shape",shape="ChannelType")` lists the enum).
const CONFIGURABLE_CHANNELS: &[&str] = &[
    "ADM",
    "APNS",
    "APNS_SANDBOX",
    "APNS_VOIP",
    "APNS_VOIP_SANDBOX",
    "BAIDU",
    "EMAIL",
    "GCM",
    "SMS",
    "VOICE",
];

/// The `ChannelType` values with no channel resource of their own, so there is
/// nothing to enable or disable for them: the same enum, minus the ten above.
const UNGATED_CHANNELS: &[&str] = &["PUSH", "CUSTOM", "IN_APP"];

/// A mounted emulator holding one shared engine, exactly as the router builds it.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: it asserts service behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(service_com_amazonaws_pinpoint_pinpoint::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router, engine }
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

    async fn update_endpoint(&self, app: &str, endpoint: &str, body: Value) -> (u16, Value) {
        self.call(
            "PUT",
            &format!("/v1/apps/{app}/endpoints/{endpoint}"),
            Some(body),
        )
        .await
    }

    async fn send_messages(&self, app: &str, request: Value) -> (u16, Value) {
        self.call("POST", &format!("/v1/apps/{app}/messages"), Some(request))
            .await
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

    /// Enable the Baidu channel of `app`, so a BAIDU destination is deliverable.
    async fn enable_baidu(&self, app: &str) {
        let (status, body) = self
            .update_baidu_channel(
                app,
                json!({"ApiKey": "baidu-api-key", "SecretKey": "baidu-secret-key", "Enabled": true}),
            )
            .await;
        assert_eq!(
            status, 200,
            "UpdateBaiduChannel must answer 200, got {body:?}"
        );
    }

    /// The sends recorded against `app`, as `(request_id, message_ids)` pairs read
    /// straight out of this service's store.
    fn recorded_sends(&self, app: &str) -> Vec<(String, Vec<String>)> {
        let store = service_com_amazonaws_pinpoint_pinpoint::state::service_state(&*self.engine)
            .expect("the shared engine holds this service's state");
        let guard = store.lock();
        let stored = guard
            .apps
            .get(app)
            .expect("the application under test is stored");
        stored
            .sends
            .iter()
            .map(|send| {
                (
                    send.request_id.clone(),
                    send.deliveries
                        .iter()
                        .filter_map(|delivery| delivery.message_id.clone())
                        .collect(),
                )
            })
            .collect()
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
/// (`smithy_query(query="shape",shape="DirectMessageConfiguration")`); a channel with
/// no member of its own is sent the default message for all channels.
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

/// One address destination's result out of a `MessageResponse`.
fn result_for(body: &Value, address: &str) -> Value {
    body["Result"][address].clone()
}

/// One endpoint destination's result out of a `MessageResponse`.
fn endpoint_result_for(body: &Value, endpoint_id: &str) -> Value {
    body["EndpointResult"][endpoint_id].clone()
}

/// Assert one destination's result carries `delivery_status` and `status_code`, and
/// that a message id was minted for it only if the message was accepted.
fn assert_outcome(case: &str, outcome: &Value, delivery_status: &str, status_code: i64) {
    assert_eq!(
        outcome["DeliveryStatus"].as_str(),
        Some(delivery_status),
        "case {case:?}: expected DeliveryStatus {delivery_status:?}, got {outcome:?}"
    );
    assert_eq!(
        outcome["StatusCode"].as_i64(),
        Some(status_code),
        "case {case:?}: expected StatusCode {status_code}, got {outcome:?}"
    );
    let message_id = outcome["MessageId"].as_str();
    if delivery_status == "SUCCESSFUL" {
        let id = message_id.unwrap_or_default();
        assert!(
            id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()),
            "case {case:?}: an accepted delivery carries a minted MessageId, got {outcome:?}"
        );
    } else {
        assert!(
            message_id.is_none(),
            "case {case:?}: a refused delivery mints no MessageId, got {outcome:?}"
        );
    }
    assert!(
        outcome["StatusMessage"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "case {case:?}: every result says why, got {outcome:?}"
    );
}

#[tokio::test]
async fn a_send_is_gated_on_the_channel_being_enabled_for_the_application() {
    let emulator = Emulator::new();
    let app = emulator.create_app("channel-gate").await;

    // Every channel the API configures as a channel of the application: an
    // application that never configured it cannot be delivered over it, however
    // well-formed the message is.
    for channel in CONFIGURABLE_CHANNELS {
        let address = format!("address-for-{channel}");
        let (status, body) = emulator
            .send_messages(
                &app,
                json!({
                    "Addresses": {address.clone(): {"ChannelType": channel}},
                    "MessageConfiguration": message_configuration_for(channel),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {channel:?}: a send whose destinations all fail is still a 200, got {body:?}"
        );
        assert_outcome(
            channel,
            &result_for(&body, &address),
            "PERMANENT_FAILURE",
            404,
        );
    }

    // A `ChannelType` the API has no channel resource for is not gated on one: there
    // is nothing a caller could enable, so the default message is accepted.
    for channel in UNGATED_CHANNELS {
        let address = format!("address-for-{channel}");
        let (status, body) = emulator
            .send_messages(
                &app,
                json!({
                    "Addresses": {address.clone(): {"ChannelType": channel}},
                    "MessageConfiguration": message_configuration_for(channel),
                }),
            )
            .await;
        assert_eq!(status, 200, "case {channel:?}: expected 200, got {body:?}");
        assert_outcome(channel, &result_for(&body, &address), "SUCCESSFUL", 200);
    }
}

#[tokio::test]
async fn the_baidu_channels_state_transitions_change_the_same_sends_outcome() {
    let emulator = Emulator::new();
    let app = emulator.create_app("baidu-transitions").await;
    let address = "baidu-push-token";
    let send = json!({
        "Addresses": {address: {"ChannelType": "BAIDU"}},
        "MessageConfiguration": {"BaiduMessage": {"Body": "baidu body", "Title": "hello"}},
    });

    // Before the channel is configured at all.
    let (_, body) = emulator.send_messages(&app, send.clone()).await;
    assert_outcome(
        "before configuring",
        &result_for(&body, address),
        "PERMANENT_FAILURE",
        404,
    );

    // Configured and enabled: the same send is accepted, and the channel is readable
    // back member for member.
    let (status, channel) = emulator
        .update_baidu_channel(
            &app,
            json!({"ApiKey": "baidu-api-key", "SecretKey": "baidu-secret-key", "Enabled": true}),
        )
        .await;
    assert_eq!(status, 200, "UpdateBaiduChannel: expected 200, got {channel:?}");
    let (status, read_back) = emulator.get_baidu_channel(&app).await;
    assert_eq!(status, 200, "GetBaiduChannel: expected 200, got {read_back:?}");
    assert_eq!(
        read_back, channel,
        "GetBaiduChannel must answer with what UpdateBaiduChannel stored"
    );
    assert_eq!(
        (
            read_back["Platform"].as_str(),
            read_back["Enabled"].as_bool(),
            read_back["Credential"].as_str(),
            read_back["HasCredential"].as_bool(),
            read_back["ApplicationId"].as_str(),
            read_back["Version"].as_i64(),
        ),
        (
            Some("BAIDU"),
            Some(true),
            Some("baidu-api-key"),
            Some(true),
            Some(app.as_str()),
            Some(1),
        ),
        "the first channel write is version 1 and reports the stored settings, got {read_back:?}"
    );
    assert!(
        read_back["CreationDate"].as_str().is_some_and(|d| !d.is_empty())
            && read_back["LastModifiedDate"]
                .as_str()
                .is_some_and(|d| !d.is_empty()),
        "a configured channel carries both dates, got {read_back:?}"
    );

    let (_, body) = emulator.send_messages(&app, send.clone()).await;
    assert_outcome(
        "enabled",
        &result_for(&body, address),
        "SUCCESSFUL",
        200,
    );

    // Disabled again, with the credentials still stored: not deliverable over.
    let (status, channel) = emulator
        .update_baidu_channel(
            &app,
            json!({"ApiKey": "baidu-api-key", "SecretKey": "baidu-secret-key", "Enabled": false}),
        )
        .await;
    assert_eq!(status, 200, "UpdateBaiduChannel: expected 200, got {channel:?}");
    assert_eq!(
        (channel["Enabled"].as_bool(), channel["Version"].as_i64()),
        (Some(false), Some(2)),
        "a second write disables the channel and bumps its version, got {channel:?}"
    );
    let (_, body) = emulator.send_messages(&app, send.clone()).await;
    assert_outcome(
        "disabled",
        &result_for(&body, address),
        "PERMANENT_FAILURE",
        404,
    );

    // Enabled once more: deliverable again, and the creation date is still the first
    // write's.
    let (_, re_enabled) = emulator
        .update_baidu_channel(
            &app,
            json!({"ApiKey": "baidu-api-key-2", "SecretKey": "baidu-secret-key", "Enabled": true}),
        )
        .await;
    assert_eq!(
        (
            re_enabled["Enabled"].as_bool(),
            re_enabled["Version"].as_i64(),
            re_enabled["Credential"].as_str(),
            re_enabled["CreationDate"].as_str(),
        ),
        (
            Some(true),
            Some(3),
            Some("baidu-api-key-2"),
            read_back["CreationDate"].as_str(),
        ),
        "a later write keeps the creation date and replaces the credential, got {re_enabled:?}"
    );
    let (_, body) = emulator.send_messages(&app, send.clone()).await;
    assert_outcome("re-enabled", &result_for(&body, address), "SUCCESSFUL", 200);

    // Deleted: the settings are gone, so the channel reads as absent and the send is
    // refused again.
    let (status, deleted) = emulator.delete_baidu_channel(&app).await;
    assert_eq!(status, 200, "DeleteBaiduChannel: expected 200, got {deleted:?}");
    assert_eq!(
        (deleted["IsArchived"].as_bool(), deleted["Enabled"].as_bool()),
        (Some(true), Some(true)),
        "a deleted channel is reported archived, carrying the Enabled state it was stored with, got {deleted:?}"
    );
    let (status, missing) = emulator.get_baidu_channel(&app).await;
    assert_eq!(
        (status, error_code(&missing).as_str()),
        (404, "NotFoundException"),
        "GetBaiduChannel after the delete must be a 404 NotFoundException, got {missing:?}"
    );
    let (_, body) = emulator.send_messages(&app, send.clone()).await;
    assert_outcome(
        "deleted",
        &result_for(&body, address),
        "PERMANENT_FAILURE",
        404,
    );

    // And deleting what is no longer there is a 404 as well.
    let (status, again) = emulator.delete_baidu_channel(&app).await;
    assert_eq!(
        (status, error_code(&again).as_str()),
        (404, "NotFoundException"),
        "a second DeleteBaiduChannel must be a 404 NotFoundException, got {again:?}"
    );
}

/// One row of the endpoint-state table: the endpoint write that sets the state, and
/// the outcome a send to that endpoint lands on.
struct EndpointCase {
    case: &'static str,
    endpoint_id: &'static str,
    /// The `EndpointRequest` to write, or `None` for an endpoint that was never
    /// written at all.
    write: Option<Value>,
    delivery_status: &'static str,
    status_code: i64,
}

#[tokio::test]
async fn an_endpoints_stored_state_decides_what_a_send_to_it_lands_on() {
    let emulator = Emulator::new();
    let app = emulator.create_app("endpoint-state").await;
    emulator.enable_baidu(&app).await;

    let cases = vec![
        EndpointCase {
            case: "active endpoint with an address",
            endpoint_id: "deliverable",
            write: Some(json!({"Address": "token-deliverable", "ChannelType": "BAIDU"})),
            delivery_status: "SUCCESSFUL",
            status_code: 200,
        },
        EndpointCase {
            case: "the user opted out of everything",
            endpoint_id: "opted-out",
            write: Some(
                json!({"Address": "token-opted-out", "ChannelType": "BAIDU", "OptOut": "ALL"}),
            ),
            delivery_status: "OPT_OUT",
            status_code: 403,
        },
        EndpointCase {
            case: "the user opted out of nothing",
            endpoint_id: "opted-in",
            write: Some(
                json!({"Address": "token-opted-in", "ChannelType": "BAIDU", "OptOut": "NONE"}),
            ),
            delivery_status: "SUCCESSFUL",
            status_code: 200,
        },
        EndpointCase {
            case: "the endpoint is inactive",
            endpoint_id: "inactive",
            write: Some(json!({
                "Address": "token-inactive",
                "ChannelType": "BAIDU",
                "EndpointStatus": "INACTIVE"
            })),
            delivery_status: "PERMANENT_FAILURE",
            status_code: 400,
        },
        EndpointCase {
            case: "the endpoint has no address",
            endpoint_id: "addressless",
            write: Some(json!({"ChannelType": "BAIDU"})),
            delivery_status: "PERMANENT_FAILURE",
            status_code: 400,
        },
        EndpointCase {
            case: "the endpoint carries no channel",
            endpoint_id: "channelless",
            write: Some(json!({"Address": "token-channelless"})),
            delivery_status: "PERMANENT_FAILURE",
            status_code: 400,
        },
        EndpointCase {
            case: "the endpoint was never written",
            endpoint_id: "never-written",
            write: None,
            delivery_status: "PERMANENT_FAILURE",
            status_code: 404,
        },
    ];

    for case in &cases {
        if let Some(write) = case.write.clone() {
            let (status, body) = emulator
                .update_endpoint(&app, case.endpoint_id, write)
                .await;
            assert_eq!(
                status, 202,
                "case {:?}: UpdateEndpoint must answer 202, got {body:?}",
                case.case
            );
        }
    }

    // One send naming every endpoint at once: each destination is answered from its
    // own stored state, under its own endpoint id.
    let endpoints: serde_json::Map<String, Value> = cases
        .iter()
        .map(|case| (case.endpoint_id.to_string(), json!({})))
        .collect();
    let (status, body) = emulator
        .send_messages(
            &app,
            json!({
                "Endpoints": Value::Object(endpoints),
                "MessageConfiguration": {"BaiduMessage": {"Body": "baidu body"}},
            }),
        )
        .await;
    assert_eq!(status, 200, "SendMessages: expected 200, got {body:?}");

    for case in &cases {
        let outcome = endpoint_result_for(&body, case.endpoint_id);
        assert_outcome(
            case.case,
            &outcome,
            case.delivery_status,
            case.status_code,
        );
    }
    assert!(
        body["Result"].is_null(),
        "a send that names no address reports no address results, got {body:?}"
    );
    // The stored address is reported back for an endpoint destination that has one.
    assert_eq!(
        endpoint_result_for(&body, "deliverable")["Address"].as_str(),
        Some("token-deliverable"),
        "an endpoint result carries the stored address, got {body:?}"
    );
}

#[tokio::test]
async fn a_destinations_channel_must_have_a_message_in_the_request() {
    let emulator = Emulator::new();
    let app = emulator.create_app("message-coverage").await;
    emulator.enable_baidu(&app).await;
    let (status, _) = emulator
        .update_endpoint(
            &app,
            "baidu-endpoint",
            json!({"Address": "token-coverage", "ChannelType": "BAIDU"}),
        )
        .await;
    assert_eq!(status, 202, "UpdateEndpoint must answer 202");

    // A BAIDU destination takes the Baidu message, the default push notification
    // message for all push notification channels, or the default message for all
    // channels; a message for some other channel reaches it not at all.
    let cases: Vec<(&str, Value, &str, i64)> = vec![
        (
            "the channel's own message",
            json!({"BaiduMessage": {"Body": "baidu body"}}),
            "SUCCESSFUL",
            200,
        ),
        (
            "the default push notification message",
            json!({"DefaultPushNotificationMessage": {"Body": "push body"}}),
            "SUCCESSFUL",
            200,
        ),
        (
            "the default message for all channels",
            json!({"DefaultMessage": {"Body": "default body"}}),
            "SUCCESSFUL",
            200,
        ),
        (
            "another channel's message only",
            json!({"SMSMessage": {"Body": "sms body"}}),
            "PERMANENT_FAILURE",
            400,
        ),
        (
            "a third channel's message only",
            json!({"EmailMessage": {"FromAddress": "sender@example.com"}}),
            "PERMANENT_FAILURE",
            400,
        ),
        (
            "another channel's message beside the default",
            json!({"SMSMessage": {"Body": "sms body"}, "DefaultMessage": {"Body": "default"}}),
            "SUCCESSFUL",
            200,
        ),
    ];

    for (case, configuration, delivery_status, status_code) in &cases {
        let (status, body) = emulator
            .send_messages(
                &app,
                json!({
                    "Endpoints": {"baidu-endpoint": {}},
                    "MessageConfiguration": configuration.clone(),
                }),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {body:?}");
        assert_outcome(
            case,
            &endpoint_result_for(&body, "baidu-endpoint"),
            delivery_status,
            *status_code,
        );
    }
}

#[tokio::test]
async fn one_address_takes_delivery_once_in_one_send() {
    let emulator = Emulator::new();
    let app = emulator.create_app("duplicate-address").await;
    emulator.enable_baidu(&app).await;
    let address = "shared-token";
    let (status, _) = emulator
        .update_endpoint(
            &app,
            "owner",
            json!({"Address": address, "ChannelType": "BAIDU"}),
        )
        .await;
    assert_eq!(status, 202, "UpdateEndpoint must answer 202");

    // The same address named twice in one send — once by its endpoint, once as a raw
    // address: the second naming is a duplicate of the delivery the first took.
    let (status, body) = emulator
        .send_messages(
            &app,
            json!({
                "Endpoints": {"owner": {}},
                "Addresses": {address: {"ChannelType": "BAIDU"}},
                "MessageConfiguration": {"BaiduMessage": {"Body": "baidu body"}},
            }),
        )
        .await;
    assert_eq!(status, 200, "SendMessages: expected 200, got {body:?}");
    assert_outcome(
        "the endpoint that holds the address",
        &endpoint_result_for(&body, "owner"),
        "SUCCESSFUL",
        200,
    );
    assert_outcome(
        "the same address again",
        &result_for(&body, address),
        "DUPLICATE",
        400,
    );
}

#[tokio::test]
async fn a_send_that_names_no_destination_or_no_application_is_refused() {
    let emulator = Emulator::new();
    let app = emulator.create_app("refusals").await;
    let configuration = json!({"DefaultMessage": {"Body": "default body"}});

    for (case, request) in [
        (
            "no Addresses and no Endpoints",
            json!({"MessageConfiguration": configuration.clone()}),
        ),
        (
            "both maps present but empty",
            json!({
                "Addresses": {},
                "Endpoints": {},
                "MessageConfiguration": configuration.clone(),
            }),
        ),
    ] {
        let (status, body) = emulator.send_messages(&app, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "BadRequestException"),
            "case {case:?}: a send with nothing to send to is a 400 BadRequestException, got {body:?}"
        );
    }

    for case in ["00000000000000000000000000000000", "not-an-application-id"] {
        let (status, body) = emulator
            .send_messages(
                case,
                json!({
                    "Addresses": {"token": {"ChannelType": "CUSTOM"}},
                    "MessageConfiguration": configuration.clone(),
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "NotFoundException"),
            "case {case:?}: a send to an unknown application is a 404 NotFoundException, got {body:?}"
        );
    }

    // The service's own ceiling on how many destinations one send may name: the last
    // accepted size is served, one more is refused and nothing is recorded for it.
    let limit = service_com_amazonaws_pinpoint_pinpoint::state::MAX_DESTINATIONS_PER_SEND;
    for (case, count, expected_status) in [("at the ceiling", limit, 200), ("past it", limit + 1, 400)]
    {
        let addresses: serde_json::Map<String, Value> = (0..count)
            .map(|index| (format!("token-{index}"), json!({"ChannelType": "CUSTOM"})))
            .collect();
        let (status, body) = emulator
            .send_messages(
                &app,
                json!({
                    "Addresses": Value::Object(addresses),
                    "MessageConfiguration": configuration.clone(),
                }),
            )
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: a send naming {count} destinations must answer {expected_status}, got {body:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case {case:?}: the refusal is a BadRequestException, got {body:?}"
            );
        }
    }
    assert_eq!(
        emulator.recorded_sends(&app).len(),
        1,
        "only the accepted send is recorded"
    );
}

#[tokio::test]
async fn every_accepted_send_is_recorded_and_its_message_ids_are_unique() {
    let emulator = Emulator::new();
    let app = emulator.create_app("send-log").await;
    emulator.enable_baidu(&app).await;

    let mut request_ids: Vec<String> = Vec::new();
    let mut message_ids: BTreeSet<String> = BTreeSet::new();
    // Two destinations per send, one of which is refused: a message id is minted for
    // the accepted one only.
    for round in 0..3 {
        let address = format!("token-{round}");
        let (status, body) = emulator
            .send_messages(
                &app,
                json!({
                    "Addresses": {
                        address.clone(): {"ChannelType": "BAIDU"},
                        format!("gcm-{round}"): {"ChannelType": "GCM"},
                    },
                    "MessageConfiguration": {"BaiduMessage": {"Body": "baidu body"},
                                             "GCMMessage": {"Body": "gcm body"}},
                    "TraceId": format!("trace-{round}"),
                }),
            )
            .await;
        assert_eq!(status, 200, "round {round}: expected 200, got {body:?}");
        assert_outcome(&address, &result_for(&body, &address), "SUCCESSFUL", 200);
        let request_id = body["RequestId"]
            .as_str()
            .expect("every send answers with a RequestId")
            .to_string();
        assert!(
            request_ids.iter().all(|seen| seen != &request_id),
            "round {round}: each send mints its own RequestId, got {request_id}"
        );
        request_ids.push(request_id);
        let message_id = result_for(&body, &address)["MessageId"]
            .as_str()
            .expect("an accepted delivery carries a MessageId")
            .to_string();
        assert!(
            message_ids.insert(message_id.clone()),
            "round {round}: each accepted delivery mints its own MessageId, got {message_id}"
        );
    }

    let recorded = emulator.recorded_sends(&app);
    assert_eq!(
        recorded.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
        request_ids,
        "the store records every accepted send, in order, under the RequestId it answered with"
    );
    for (request_id, minted) in &recorded {
        assert_eq!(
            minted.len(),
            1,
            "send {request_id}: exactly the accepted destination has a recorded message id, got {minted:?}"
        );
        assert!(
            message_ids.contains(&minted[0]),
            "send {request_id}: the recorded message id is the one the response carried, got {minted:?}"
        );
    }
}

#[tokio::test]
async fn the_send_log_is_bounded() {
    let emulator = Emulator::new();
    let app = emulator.create_app("send-log-bound").await;
    let limit = service_com_amazonaws_pinpoint_pinpoint::state::MAX_SENDS_PER_APP;

    // Past the ceiling: the log keeps the most recent sends and drops the oldest, so
    // repeated sending cannot grow the store without limit.
    for round in 0..(limit + 5) {
        let (status, body) = emulator
            .send_messages(
                &app,
                json!({
                    "Addresses": {format!("token-{round}"): {"ChannelType": "CUSTOM"}},
                    "MessageConfiguration": {"DefaultMessage": {"Body": "default body"}},
                }),
            )
            .await;
        assert_eq!(status, 200, "round {round}: expected 200, got {body:?}");
        let recorded = emulator.recorded_sends(&app);
        let expected = (round + 1).min(limit);
        assert_eq!(
            recorded.len(),
            expected,
            "round {round}: expected {expected} recorded sends, got {}",
            recorded.len()
        );
    }
}
