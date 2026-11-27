// SPDX-License-Identifier: CC0-1.0

//! The Event API's DATA plane, driven over real sockets: the HTTP endpoint a publisher
//! POSTs to, the real-time endpoint a subscriber opens a WebSocket against, the channel
//! namespace's own handler code deciding which events are published, and the
//! `AWS/AppSync` series the whole path publishes.
//!
//! The rule these cases assert, one level above the recorded case (one Event API, one
//! namespace whose `onPublish` filters on a payload member, one subscriber, four
//! publishes of one event each):
//!
//! > An Event API publishes, as its `dns` records, the addresses of the two sockets it is
//! > SERVED on. The HTTP one serves `POST /event`: it resolves the channel's channel
//! > namespace, RUNS that namespace's `onPublish` handler over the events the request
//! > carried, answers 200 with the events it accepted under `successful` and the ones it
//! > could not under `failed`, and delivers exactly the events the handler returned to
//! > every subscription whose channel covers the published one. The real-time one serves a
//! > WebSocket at `/event/realtime` for a client offering the `aws-appsync-event-ws`
//! > subprotocol and material that authorizes a connection: `connection_init` is answered
//! > `connection_ack`, `subscribe` `subscribe_success`, `unsubscribe`
//! > `unsubscribe_success`, a `publish` over the socket `publish_success`, and each
//! > delivered event arrives as a `data` frame carrying the subscription's id. Every call
//! > on that path publishes into CloudWatch's `AWS/AppSync` namespace: what happens on a
//! > channel namespace under exactly `[EventAPIId, ChannelNamespaceName]`, and what
//! > happens on a connection under exactly `[EventAPIId]`.
//!
//! The rows span what the rule generalises over: the namespace's handler (absent, a
//! filter, a transform, one that raises, one that returns nothing), the number of events
//! one publish carries, the channel a subscription names (the published channel, a
//! trailing wildcard, another channel of the same namespace), which endpoint the publish
//! was made on, and the material a call presents (a live API key, no material, an unknown
//! key).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

// The address the core binds a resource listener on by default, read from the
// create-and-read suite that asserts an API publishes its endpoints there, so the two
// cannot drift on it.
use crate::event_api::LOOPBACK_HOST;

/// The region every call in this suite is signed for.
const REGION: &str = "us-west-2";

/// How long a case waits for one frame before giving up: generous for a loaded test
/// machine, and bounded so a case that will never be answered fails instead of hanging.
const FRAME_DEADLINE: Duration = Duration::from_secs(10);

/// How long a case waits to be sure NOTHING arrives. Short, because it is spent in full on
/// every such assertion, and long enough that a delivery already queued would land in it —
/// a delivery is made by the publish that is already answered by the time it is awaited.
const QUIET_DEADLINE: Duration = Duration::from_millis(500);

/// The status a completed opening handshake carries (RFC 6455 §4.1).
const SWITCHING_PROTOCOLS: u16 = 101;

/// The subprotocol an Event API's real-time client offers; the service's own copy is
/// `service::event_api::realtime::SUBPROTOCOL`.
const SUBPROTOCOL: &str = "aws-appsync-event-ws";

/// A router with AppSync and CloudWatch mounted over ONE engine, so the series the data
/// plane publishes are read back through the service that owns metrics.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(engine),
    );
    router
}

/// One AppSync control-plane call.
pub(crate) async fn appsync(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://appsync.{REGION}.amazonaws.com{path}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/appsync/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(payload)
        .expect("a well-formed request");
    answered(router, request).await
}

/// One CloudWatch call, over the wire that service speaks (`awsJson1_0`, `POST /` with
/// `X-Amz-Target`).
pub(crate) async fn cloudwatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("GraniteServiceVersion20100801.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/monitoring/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    answered(router, request).await
}

/// The status and parsed body of one dispatched request.
async fn answered(router: &MultiServiceRouter, request: http::Request<Bytes>) -> (u16, Value) {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body)
}

/// One created Event API: its id, the two addresses it publishes, and a live API key.
pub(crate) struct EventApi {
    pub(crate) api_id: String,
    pub(crate) http: String,
    pub(crate) realtime: String,
    pub(crate) api_key: String,
}

/// An `eventConfig` whose every mode is `API_KEY`, as the recorded case creates one.
fn api_key_event_config() -> Value {
    json!({
        "authProviders": [{"authType": "API_KEY"}],
        "connectionAuthModes": [{"authType": "API_KEY"}],
        "defaultPublishAuthModes": [{"authType": "API_KEY"}],
        "defaultSubscribeAuthModes": [{"authType": "API_KEY"}],
    })
}

/// Create one Event API with an API key, and read the two endpoints it publishes.
pub(crate) async fn create_event_api(router: &MultiServiceRouter, name: &str) -> EventApi {
    create_configured_event_api(router, name, api_key_event_config()).await
}

/// The same, with the `eventConfig` the case wants — the seam for a case about WHICH
/// authorization providers an API set up.
async fn create_configured_event_api(
    router: &MultiServiceRouter,
    name: &str,
    event_config: Value,
) -> EventApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v2/apis",
        Some(json!({"name": name, "eventConfig": event_config})),
    )
    .await;
    assert_eq!(status, 200, "CreateApi must answer 200, got {created}");
    let api = &created["api"];
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
    let endpoint = |key: &str| {
        let published = api["dns"][key].as_str().unwrap_or_default().to_string();
        assert!(
            published.starts_with(&format!("{LOOPBACK_HOST}:")),
            "dns.{key} must be an address this emulator serves, got {published:?}"
        );
        published
    };
    let http = endpoint("HTTP");
    let realtime = endpoint("REALTIME");
    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    EventApi {
        api_id,
        http,
        realtime,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// Create one channel namespace, with the handler code it carries (or none).
pub(crate) async fn create_namespace(
    router: &MultiServiceRouter,
    api_id: &str,
    name: &str,
    code_handlers: Option<&str>,
) {
    let mut body = json!({"name": name});
    if let Some(code) = code_handlers {
        body["codeHandlers"] = json!(code);
    }
    create_namespace_body(router, api_id, body).await;
}

/// The same, with the whole `CreateChannelNamespace` body the case wants — the seam for a
/// namespace that OVERRIDES its API's authorization modes.
async fn create_namespace_body(router: &MultiServiceRouter, api_id: &str, body: Value) {
    let (status, created) = appsync(
        router,
        "POST",
        &format!("/v2/apis/{api_id}/channelNamespaces"),
        Some(body),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateChannelNamespace must answer 200, got {created}"
    );
}

/// The socket a client gets from a completed handshake.
pub(crate) type ClientSocket = tokio_tungstenite::WebSocketStream<TcpStream>;

/// The port of an announced endpoint, having checked the address is [`LOOPBACK_HOST`] — so
/// a dial names that literal and reaches nothing else.
pub(crate) fn loopback_port(case: &str, authority: &str) -> u16 {
    let (host, port) = authority
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("case {case:?}: expected a host:port, got {authority:?}"));
    assert_eq!(
        host, LOOPBACK_HOST,
        "case {case:?}: a case dials only this emulator's own loopback listener, got \
         {authority:?}"
    );
    port.parse()
        .unwrap_or_else(|_| panic!("case {case:?}: expected a port, got {authority:?}"))
}

/// Dial one announced endpoint. The target is the loopback literal: a case reaches this
/// emulator's own listener on this machine and nothing else.
pub(crate) async fn dial(case: &str, authority: &str) -> std::io::Result<TcpStream> {
    TcpStream::connect(("127.0.0.1", loopback_port(case, authority))).await
}

/// The subprotocol list a real-time client offers: this endpoint's subprotocol, and the
/// authorization object base64url-encoded in the token a WebSocket client carries its
/// request fields in.
fn offered_subprotocols(authority: &str, api_key: Option<&str>) -> String {
    match api_key {
        Some(key) => auth_subprotocols(&json!({"host": authority, "x-api-key": key})),
        None => SUBPROTOCOL.to_string(),
    }
}

/// The same, carrying whatever authorization OBJECT the case presents — the seam for a
/// client presenting material of a provider other than `API_KEY`.
fn auth_subprotocols(object: &Value) -> String {
    format!("{SUBPROTOCOL}, header-{}", encoded_auth_object(object))
}

/// One authorization object as a real-time client encodes it: base64url, which is the form
/// it travels in whether a client carries it in a subprotocol token or, as a GraphQL API's
/// client does, in the handshake's `header` query parameter.
pub(crate) fn encoded_auth_object(object: &Value) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(object.to_string())
}

/// A `Sec-WebSocket-Key`: "a base64-encoded value that, when decoded, is 16 bytes in
/// length" (RFC 6455 §4.1). A fixed one, because what the answer derives from it is the
/// protocol's business and not this suite's.
pub(crate) const CLIENT_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

/// Open an opening handshake against an announced real-time endpoint: the established
/// socket, or the status the refusal carried.
async fn handshake(
    case: &str,
    api: &EventApi,
    path: &str,
    subprotocols: &str,
) -> Result<ClientSocket, u16> {
    handshake_with_fields(case, api, path, subprotocols, &[]).await
}

/// The same, carrying whatever request FIELDS the case presents on the opening handshake —
/// the seam for a handshake made from another SITE, or addressed to another HOST.
async fn handshake_with_fields(
    case: &str,
    api: &EventApi,
    path: &str,
    subprotocols: &str,
    fields: &[(&str, &str)],
) -> Result<ClientSocket, u16> {
    let stream = dial(case, &api.realtime).await.unwrap_or_else(|error| {
        panic!(
            "case {case:?}: the announced real-time endpoint {:?} must accept a connection, \
             got {error}",
            api.realtime
        )
    });
    // The fields RFC 6455 §4.1 requires of an opening handshake, written out because the
    // request below is handed to the client as it stands.
    let mut request = http::Request::builder()
        .uri(format!("ws://{}{path}", api.realtime))
        .header("host", api.realtime.clone())
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", CLIENT_KEY)
        .body(())
        .expect("a well-formed handshake request");
    if !subprotocols.is_empty() {
        request.headers_mut().insert(
            "sec-websocket-protocol",
            http::HeaderValue::from_str(subprotocols).expect("a well-formed subprotocol list"),
        );
    }
    for (field, value) in fields {
        request.headers_mut().insert(
            http::HeaderName::from_bytes(field.as_bytes()).expect("a well-formed field name"),
            http::HeaderValue::from_str(value).expect("a well-formed field value"),
        );
    }
    match tokio_tungstenite::client_async(request, stream).await {
        Ok((socket, response)) => {
            assert_eq!(
                response.status().as_u16(),
                SWITCHING_PROTOCOLS,
                "case {case:?}: a completed handshake must answer 101"
            );
            assert_eq!(
                response
                    .headers()
                    .get("sec-websocket-protocol")
                    .and_then(|value| value.to_str().ok()),
                Some(SUBPROTOCOL),
                "case {case:?}: the answer must name the subprotocol this endpoint selected"
            );
            Ok(socket)
        }
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            Err(response.status().as_u16())
        }
        Err(error) => panic!("case {case:?}: the handshake failed before any answer: {error}"),
    }
}

/// Open a connection, ack it, and answer the socket — what every case that needs a
/// subscriber starts from.
pub(crate) async fn connected(case: &str, api: &EventApi) -> ClientSocket {
    let mut socket = handshake(
        case,
        api,
        "/event/realtime",
        &offered_subprotocols(&api.http, Some(&api.api_key)),
    )
    .await
    .unwrap_or_else(|status| panic!("case {case:?}: the handshake must complete, got {status}"));
    let ack = exchange(case, &mut socket, &json!({"type": "connection_init"})).await;
    assert_eq!(
        ack.as_ref().map(|frame| frame["type"].clone()),
        Some(json!("connection_ack")),
        "case {case:?}: connection_init must be answered connection_ack, got {ack:?}"
    );
    socket
}

/// Send one message and read the frame the endpoint answers with.
pub(crate) async fn exchange(case: &str, socket: &mut ClientSocket, message: &Value) -> Option<Value> {
    socket
        .send(Message::Text(message.to_string().into()))
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the message must be sent, got {error}"));
    next_frame(case, socket).await
}

/// Read the next frame, or `None` when none arrives within [`FRAME_DEADLINE`].
async fn next_frame(case: &str, socket: &mut ClientSocket) -> Option<Value> {
    next_frame_within(case, socket, FRAME_DEADLINE).await
}

/// Read the next frame, or `None` when none arrives within `deadline`.
async fn next_frame_within(
    case: &str,
    socket: &mut ClientSocket,
    deadline: Duration,
) -> Option<Value> {
    match tokio::time::timeout(deadline, socket.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => Some(
            serde_json::from_str(text.as_str())
                .unwrap_or_else(|error| panic!("case {case:?}: a frame must be a document: {error}")),
        ),
        Ok(Some(Ok(Message::Close(_)))) | Ok(None) => None,
        Ok(Some(Ok(other))) => panic!("case {case:?}: unexpected frame {other:?}"),
        Ok(Some(Err(error))) => panic!("case {case:?}: the socket failed: {error}"),
        Err(_elapsed) => None,
    }
}

/// POST one document to an announced HTTP endpoint, as a publisher does: the status and
/// the parsed body.
async fn post(
    case: &str,
    api: &EventApi,
    path: &str,
    method: &str,
    body: &Value,
    api_key: Option<&str>,
) -> (u16, Value) {
    let fields: Vec<(&str, &str)> = api_key
        .map(|key| vec![("x-api-key", key)])
        .unwrap_or_default();
    post_with_fields(case, api, path, method, body, &fields).await
}

/// The same, carrying whatever request FIELDS the case presents — the seam for a publisher
/// presenting material of a provider other than `API_KEY`.
async fn post_with_fields(
    case: &str,
    api: &EventApi,
    path: &str,
    method: &str,
    body: &Value,
    fields: &[(&str, &str)],
) -> (u16, Value) {
    let mut stream = dial(case, &api.http).await.unwrap_or_else(|error| {
        panic!(
            "case {case:?}: the announced HTTP endpoint {:?} must accept a connection, got {error}",
            api.http
        )
    });
    let payload = body.to_string();
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nhost: {}\r\ncontent-type: application/json\r\n\
         content-length: {}\r\nconnection: close\r\n",
        api.http,
        payload.len()
    );
    for (field, value) in fields {
        request.push_str(&format!("{field}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&payload);
    stream
        .write_all(request.as_bytes())
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the request must be written: {error}"));
    let mut answered = Vec::new();
    tokio::time::timeout(FRAME_DEADLINE, stream.read_to_end(&mut answered))
        .await
        .unwrap_or_else(|_| panic!("case {case:?}: the endpoint must answer within the deadline"))
        .unwrap_or_else(|error| panic!("case {case:?}: the answer must be read: {error}"));
    let text = String::from_utf8_lossy(&answered).to_string();
    let (head, payload) = text
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("case {case:?}: expected an HTTP response, got {text:?}"));
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("case {case:?}: expected a status line, got {head:?}"));
    let document = serde_json::from_str(payload).unwrap_or(Value::Null);
    (status, document)
}

/// Publish `events` to `channel` over the announced HTTP endpoint.
///
/// Shared with the suite that asserts how a publish PARTITIONS its batch
/// ([`crate::event_api_publish_handler_errors`]), so a publisher is written once.
pub(crate) async fn publish(
    case: &str,
    api: &EventApi,
    channel: &str,
    events: &[Value],
    api_key: Option<&str>,
) -> (u16, Value) {
    let events: Vec<Value> = events.iter().map(|event| json!(event.to_string())).collect();
    post(
        case,
        api,
        "/event",
        "POST",
        &json!({"channel": channel, "events": events}),
        api_key,
    )
    .await
}

/// The `payload` of the next `expected` `data` frames, in order, stopping at the first
/// deadline that passes with nothing on the socket.
pub(crate) async fn delivered(case: &str, socket: &mut ClientSocket, expected: usize) -> Vec<Value> {
    let mut events = Vec::new();
    for _ in 0..expected {
        let Some(frame) = next_frame(case, socket).await else {
            break;
        };
        assert_eq!(
            frame["type"], json!("data"),
            "case {case:?}: expected a data frame, got {frame}"
        );
        let event = frame["event"].as_str().unwrap_or_default().to_string();
        events.push(
            serde_json::from_str(&event)
                .unwrap_or_else(|error| panic!("case {case:?}: an event must be a document: {error}")),
        );
    }
    events
}

/// Assert that NOTHING more arrives on the socket: the events a publish did not publish
/// are the ones that reach no subscriber.
async fn quiet(case: &str, what: &str, socket: &mut ClientSocket) {
    let extra = next_frame_within(case, socket, QUIET_DEADLINE).await;
    assert!(
        extra.is_none(),
        "case {case:?}: expected {what}, got {extra:?}"
    );
}

/// One row of the publish table: the namespace's handler, what is published to it, and
/// what the subscriber must receive.
struct PublishRow {
    case: &'static str,
    namespace: &'static str,
    /// The namespace's `codeHandlers`, or none.
    code_handlers: Option<&'static str>,
    /// The channel the subscription names, which is not always the published one.
    subscribed: &'static str,
    /// The channel the events are published to.
    channel: &'static str,
    /// The events the publish carries.
    events: Vec<Value>,
    /// Whether the publish is made over the real-time socket rather than over HTTP.
    over_socket: bool,
    /// The payloads the subscription must receive, in order.
    expected: Vec<Value>,
    /// How many of the events the publish must report under `successful` and `failed`.
    expected_successful: usize,
    expected_failed: usize,
}

/// `onPublish` handlers the rows use, each a module as AppSync's own handlers are written.
///
/// The filtering one is shared with the suite that drives a namespace from the addresses the
/// inventory published ([`crate::api_published_endpoint_reachability`]), so a handler that
/// decides what is published is written once.
pub(crate) const FILTER_HANDLER: &str =
    "export function onPublish(ctx) { return ctx.events.filter((e) => e.payload.keep === true) }";
const TRANSFORM_HANDLER: &str = "export function onPublish(ctx) {\n  \
     return ctx.events.map((e) => ({id: e.id, payload: {seen: e.payload.m, \
     channel: ctx.info.channel.path}}))\n}";
const RAISING_HANDLER: &str = "import { util } from '@aws-appsync/utils';\n\
     export function onPublish(ctx) { util.error('this namespace refuses', 'Refused') }";
const SILENT_HANDLER: &str = "export function onPublish(ctx) { }";

fn publish_rows() -> Vec<PublishRow> {
    vec![
        PublishRow {
            case: "the recorded case: a filter on a payload member, one event at a time",
            namespace: "filtered",
            code_handlers: Some(FILTER_HANDLER),
            subscribed: "/filtered/room1",
            channel: "/filtered/room1",
            events: vec![json!({"m": "a", "keep": true})],
            over_socket: false,
            expected: vec![json!({"m": "a", "keep": true})],
            expected_successful: 1,
            expected_failed: 0,
        },
        PublishRow {
            case: "the same filter over a publish carrying three events",
            namespace: "filtered-batch",
            code_handlers: Some(FILTER_HANDLER),
            subscribed: "/filtered-batch/room1",
            channel: "/filtered-batch/room1",
            events: vec![
                json!({"m": "a", "keep": true}),
                json!({"m": "b", "keep": false}),
                json!({"m": "c", "keep": true}),
            ],
            over_socket: false,
            expected: vec![
                json!({"m": "a", "keep": true}),
                json!({"m": "c", "keep": true}),
            ],
            expected_successful: 3,
            expected_failed: 0,
        },
        PublishRow {
            case: "a namespace carrying no handler publishes what it was given",
            namespace: "open",
            code_handlers: None,
            subscribed: "/open/room1",
            channel: "/open/room1",
            events: vec![json!({"m": "a"}), json!({"m": "b"})],
            over_socket: false,
            expected: vec![json!({"m": "a"}), json!({"m": "b"})],
            expected_successful: 2,
            expected_failed: 0,
        },
        PublishRow {
            case: "a handler that transforms every event, under a wildcard subscription",
            namespace: "mapped",
            code_handlers: Some(TRANSFORM_HANDLER),
            subscribed: "/mapped/*",
            channel: "/mapped/room7",
            events: vec![json!({"m": "a"})],
            over_socket: false,
            expected: vec![json!({"seen": "a", "channel": "/mapped/room7"})],
            expected_successful: 1,
            expected_failed: 0,
        },
        PublishRow {
            case: "a handler that raises accepts nothing and delivers nothing",
            namespace: "refusing",
            code_handlers: Some(RAISING_HANDLER),
            subscribed: "/refusing/room1",
            channel: "/refusing/room1",
            events: vec![json!({"m": "a"}), json!({"m": "b"})],
            over_socket: false,
            expected: Vec::new(),
            expected_successful: 0,
            expected_failed: 2,
        },
        PublishRow {
            case: "a handler that returns nothing publishes nothing, and the publish succeeds",
            namespace: "silent",
            code_handlers: Some(SILENT_HANDLER),
            subscribed: "/silent/room1",
            channel: "/silent/room1",
            events: vec![json!({"m": "a"})],
            over_socket: false,
            expected: Vec::new(),
            expected_successful: 1,
            expected_failed: 0,
        },
        PublishRow {
            case: "a subscription on another channel of the namespace receives nothing",
            namespace: "elsewhere",
            code_handlers: None,
            subscribed: "/elsewhere/room1",
            channel: "/elsewhere/room2",
            events: vec![json!({"m": "a"})],
            over_socket: false,
            expected: Vec::new(),
            expected_successful: 1,
            expected_failed: 0,
        },
        PublishRow {
            case: "a publish made over the subscriber's own socket",
            namespace: "socket",
            code_handlers: Some(FILTER_HANDLER),
            subscribed: "/socket/room1",
            channel: "/socket/room1",
            events: vec![
                json!({"m": "a", "keep": true}),
                json!({"m": "b", "keep": false}),
            ],
            over_socket: true,
            expected: vec![json!({"m": "a", "keep": true})],
            expected_successful: 2,
            expected_failed: 0,
        },
    ]
}

#[tokio::test]
async fn an_event_api_delivers_exactly_the_events_its_namespaces_handler_returns() {
    let router = emulator();
    let api = create_event_api(&router, "Events").await;

    for row in publish_rows() {
        let case = row.case;
        create_namespace(&router, &api.api_id, row.namespace, row.code_handlers).await;
        let mut socket = connected(case, &api).await;
        let subscription = "sub-1";
        let subscribed = exchange(
            case,
            &mut socket,
            &json!({"type": "subscribe", "id": subscription, "channel": row.subscribed}),
        )
        .await;
        assert_eq!(
            subscribed.as_ref().map(|frame| frame["type"].clone()),
            Some(json!("subscribe_success")),
            "case {case:?}: subscribe must be answered subscribe_success, got {subscribed:?}"
        );

        let answer = if row.over_socket {
            let events: Vec<Value> = row
                .events
                .iter()
                .map(|event| json!(event.to_string()))
                .collect();
            let frame = exchange(
                case,
                &mut socket,
                &json!({
                    "type": "publish",
                    "id": "pub-1",
                    "channel": row.channel,
                    "events": events,
                }),
            )
            .await
            .unwrap_or_else(|| panic!("case {case:?}: a publish must be answered"));
            assert_eq!(
                frame["type"],
                json!("publish_success"),
                "case {case:?}: expected publish_success, got {frame}"
            );
            frame
        } else {
            let (status, body) = publish(
                case,
                &api,
                row.channel,
                &row.events,
                Some(&api.api_key),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a publish must be answered 200, got {status} {body}"
            );
            body
        };
        assert_eq!(
            answer["successful"].as_array().map(Vec::len),
            Some(row.expected_successful),
            "case {case:?}: expected {} accepted events, got {answer}",
            row.expected_successful
        );
        assert_eq!(
            answer["failed"].as_array().map(Vec::len),
            Some(row.expected_failed),
            "case {case:?}: expected {} failed events, got {answer}",
            row.expected_failed
        );
        for (index, accepted) in answer["successful"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .enumerate()
        {
            assert_eq!(
                accepted["index"].as_u64(),
                Some(index as u64),
                "case {case:?}: an accepted event names the index it was published at, got \
                 {answer}"
            );
            assert!(
                accepted["identifier"]
                    .as_str()
                    .is_some_and(|identifier| identifier.len() == 36),
                "case {case:?}: an accepted event carries its own identifier, got {answer}"
            );
        }

        // Exactly the events the handler returned arrive, in order, and nothing else does.
        let received = delivered(case, &mut socket, row.expected.len()).await;
        assert_eq!(
            received, row.expected,
            "case {case:?}: expected {:?} delivered, got {received:?}",
            row.expected
        );
        quiet(case, "no event the handler did not return", &mut socket).await;

        let unsubscribed = exchange(
            case,
            &mut socket,
            &json!({"type": "unsubscribe", "id": subscription}),
        )
        .await;
        assert_eq!(
            unsubscribed.as_ref().map(|frame| frame["type"].clone()),
            Some(json!("unsubscribe_success")),
            "case {case:?}: unsubscribe must be answered unsubscribe_success, got \
             {unsubscribed:?}"
        );
        // An event published after the unsubscribe reaches this socket no longer.
        let (status, _) = publish(
            case,
            &api,
            row.channel,
            &[json!({"m": "after", "keep": true})],
            Some(&api.api_key),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a publish after the unsubscribe is still served"
        );
        quiet(case, "nothing on an unsubscribed socket", &mut socket).await;
        let _ = socket.close(None).await;
    }
}

/// One row of the refusal table: a call this plane does not serve, and the status it is
/// answered with.
struct RefusalRow {
    case: &'static str,
    /// The call: a handshake (path, subprotocols) or a request on the HTTP endpoint.
    call: Call,
    expected: u16,
}

enum Call {
    /// An opening handshake on the real-time endpoint.
    Handshake {
        path: &'static str,
        /// `None` presents no authorization token at all.
        api_key: Option<&'static str>,
        /// Whether the client offers this endpoint's subprotocol.
        offers_subprotocol: bool,
        /// Request fields the handshake carries beyond the ones RFC 6455 requires — where
        /// a row is about the transport rather than about the API.
        fields: Vec<(&'static str, &'static str)>,
    },
    /// A request on the HTTP endpoint.
    Http {
        method: &'static str,
        path: &'static str,
        body: Value,
        /// `None` presents no API key at all; `Some` an id, which may name no key.
        api_key: Option<&'static str>,
        /// The same seam as above, for the publish route.
        fields: Vec<(&'static str, &'static str)>,
    },
}

#[tokio::test]
async fn the_endpoints_refuse_what_they_do_not_serve() {
    let router = emulator();
    let api = create_event_api(&router, "Refusals").await;
    create_namespace(&router, &api.api_id, "default", None).await;
    let events = vec![json!(json!({"m": "a"}).to_string())];

    let rows = vec![
        RefusalRow {
            case: "a handshake a browser made from another site",
            call: Call::Handshake {
                path: "/event/realtime",
                api_key: Some("live"),
                offers_subprotocol: true,
                fields: vec![
                    ("origin", "https://unrelated.example"),
                    ("sec-fetch-site", "cross-site"),
                ],
            },
            expected: 403,
        },
        RefusalRow {
            case: "a handshake addressed to a host this endpoint does not serve",
            call: Call::Handshake {
                path: "/event/realtime",
                api_key: Some("live"),
                offers_subprotocol: true,
                fields: vec![("host", "rebound.example")],
            },
            expected: 421,
        },
        RefusalRow {
            case: "a publish a browser made from another site",
            call: Call::Http {
                method: "POST",
                path: "/event",
                body: json!({"channel": "/default/room1", "events": events.clone()}),
                api_key: Some("live"),
                fields: vec![
                    ("origin", "https://unrelated.example"),
                    ("sec-fetch-site", "cross-site"),
                ],
            },
            expected: 403,
        },
        RefusalRow {
            case: "a handshake on a path the real-time endpoint does not serve",
            call: Call::Handshake {
                path: "/event",
                api_key: Some("live"),
                offers_subprotocol: true,
                fields: Vec::new(),
            },
            expected: 404,
        },
        RefusalRow {
            case: "a handshake offering no subprotocol of this endpoint",
            call: Call::Handshake {
                path: "/event/realtime",
                api_key: None,
                offers_subprotocol: false,
                fields: Vec::new(),
            },
            expected: 400,
        },
        RefusalRow {
            case: "a handshake presenting no authorization material",
            call: Call::Handshake {
                path: "/event/realtime",
                api_key: None,
                offers_subprotocol: true,
                fields: Vec::new(),
            },
            expected: 401,
        },
        RefusalRow {
            case: "a handshake presenting a key this API never minted",
            call: Call::Handshake {
                path: "/event/realtime",
                api_key: Some("da2-nosuchkeyatallnosuchkey"),
                offers_subprotocol: true,
                fields: Vec::new(),
            },
            expected: 401,
        },
        RefusalRow {
            case: "a publish presenting no API key",
            call: Call::Http {
                method: "POST",
                path: "/event",
                body: json!({"channel": "/default/room1", "events": events.clone()}),
                api_key: None,
                fields: Vec::new(),
            },
            expected: 401,
        },
        RefusalRow {
            case: "a publish presenting a key this API never minted",
            call: Call::Http {
                method: "POST",
                path: "/event",
                body: json!({"channel": "/default/room1", "events": events.clone()}),
                api_key: Some("da2-nosuchkeyatallnosuchkey"),
                fields: Vec::new(),
            },
            expected: 401,
        },
        RefusalRow {
            case: "a publish to a channel namespace this API does not hold",
            call: Call::Http {
                method: "POST",
                path: "/event",
                body: json!({"channel": "/missing/room1", "events": events.clone()}),
                api_key: Some("live"),
                fields: Vec::new(),
            },
            expected: 404,
        },
        RefusalRow {
            case: "a publish naming no channel at all",
            call: Call::Http {
                method: "POST",
                path: "/event",
                body: json!({"events": events.clone()}),
                api_key: Some("live"),
                fields: Vec::new(),
            },
            expected: 400,
        },
        RefusalRow {
            case: "a publish carrying more events than one publish may",
            call: Call::Http {
                method: "POST",
                path: "/event",
                body: json!({
                    "channel": "/default/room1",
                    "events": vec![events[0].clone(); 6],
                }),
                api_key: Some("live"),
                fields: Vec::new(),
            },
            expected: 400,
        },
        RefusalRow {
            case: "a GET on the publish route",
            call: Call::Http {
                method: "GET",
                path: "/event",
                body: json!({}),
                api_key: Some("live"),
                fields: Vec::new(),
            },
            expected: 405,
        },
        RefusalRow {
            case: "a request on a path the HTTP endpoint does not serve",
            call: Call::Http {
                method: "POST",
                path: "/graphql",
                body: json!({}),
                api_key: Some("live"),
                fields: Vec::new(),
            },
            expected: 404,
        },
    ];

    for row in rows {
        let case = row.case;
        // `"live"` stands for the key this API really minted, so a row names a live key
        // without the suite knowing the minted id up front.
        let live = |named: Option<&str>| match named {
            Some("live") => Some(api.api_key.clone()),
            Some(other) => Some(other.to_string()),
            None => None,
        };
        let answered = match &row.call {
            Call::Handshake {
                path,
                api_key,
                offers_subprotocol,
                fields,
            } => {
                let key = live(*api_key);
                let subprotocols = if *offers_subprotocol {
                    offered_subprotocols(&api.http, key.as_deref())
                } else {
                    String::new()
                };
                handshake_with_fields(case, &api, path, &subprotocols, fields)
                    .await
                    .err()
            }
            Call::Http {
                method,
                path,
                body,
                api_key,
                fields,
            } => {
                let key = live(*api_key);
                let mut presented: Vec<(&str, &str)> = fields.clone();
                if let Some(key) = key.as_deref() {
                    presented.push(("x-api-key", key));
                }
                let (status, _body) =
                    post_with_fields(case, &api, path, method, body, &presented).await;
                Some(status)
            }
        };
        assert_eq!(
            answered,
            Some(row.expected),
            "case {case:?}: expected {}, got {answered:?}",
            row.expected
        );
    }
}

/// Every datapoint of one series, by dimension set, as `GetMetricStatistics` reports them.
///
/// Shared with the suites that read an `AWS/AppSync` series of their own
/// ([`crate::graphql_api_request_metrics`]), so there is one reading of how a client asks for
/// one rather than one per suite.
pub(crate) async fn statistics(
    router: &MultiServiceRouter,
    metric: &str,
    dimensions: &[(&str, &str)],
) -> Vec<Value> {
    statistics_in_unit(router, metric, dimensions, None).await
}

/// The same reading, for a client that NAMES the unit it wants — "if you specify a unit, the
/// operation returns only data that was collected with that unit specified"
/// (`GetMetricStatistics$Unit`) — or names none, which is what [`statistics`] asks for and
/// sees every unit a datapoint was published with.
pub(crate) async fn statistics_in_unit(
    router: &MultiServiceRouter,
    metric: &str,
    dimensions: &[(&str, &str)],
    unit: Option<&str>,
) -> Vec<Value> {
    let dimensions: Vec<Value> = dimensions
        .iter()
        .map(|(name, value)| json!({"Name": name, "Value": value}))
        .collect();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    let mut request = json!({
        "Namespace": "AWS/AppSync",
        "MetricName": metric,
        "Dimensions": dimensions,
        "StartTime": now - 600.0,
        "EndTime": now + 60.0,
        "Period": 60,
        "Statistics": ["SampleCount", "Sum", "Minimum", "Maximum"],
    });
    if let Some(unit) = unit {
        request["Unit"] = json!(unit);
    }
    let (status, body) = cloudwatch(router, "GetMetricStatistics", &request).await;
    assert_eq!(
        status, 200,
        "GetMetricStatistics for {metric} must answer 200, got {body}"
    );
    body["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `Sum` of every datapoint of one series, or `None` when the series holds none.
pub(crate) async fn metric_sum(
    router: &MultiServiceRouter,
    metric: &str,
    dimensions: &[(&str, &str)],
) -> Option<f64> {
    let datapoints = statistics(router, metric, dimensions).await;
    if datapoints.is_empty() {
        return None;
    }
    Some(
        datapoints
            .iter()
            .filter_map(|point| point["Sum"].as_f64())
            .sum(),
    )
}

/// Every metric of the `AWS/AppSync` namespace, as the dimension NAMES each is published
/// under.
async fn catalogued(router: &MultiServiceRouter) -> Vec<(String, Vec<String>)> {
    let (status, body) = cloudwatch(
        router,
        "ListMetrics",
        &json!({"Namespace": "AWS/AppSync"}),
    )
    .await;
    assert_eq!(status, 200, "ListMetrics must answer 200, got {body}");
    body["Metrics"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|metric| {
            let names: Vec<String> = metric["Dimensions"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|dimension| dimension["Name"].as_str().map(str::to_string))
                .collect();
            (
                metric["MetricName"].as_str().unwrap_or_default().to_string(),
                names,
            )
        })
        .collect()
}

#[tokio::test]
async fn the_data_plane_publishes_each_series_under_exactly_its_own_dimension_set() {
    let router = emulator();
    let api = create_event_api(&router, "Metrics").await;
    let namespace = "counted";
    create_namespace(&router, &api.api_id, namespace, Some(FILTER_HANDLER)).await;

    // The recorded lifecycle: one connection, one subscription, four publishes of one
    // event each — one of which the handler filters out — then an unsubscribe.
    let case = "the recorded namespace lifecycle";
    let mut socket = connected(case, &api).await;
    let subscribed = exchange(
        case,
        &mut socket,
        &json!({"type": "subscribe", "id": "sub-1", "channel": format!("/{namespace}/room1")}),
    )
    .await;
    assert_eq!(
        subscribed.as_ref().map(|frame| frame["type"].clone()),
        Some(json!("subscribe_success")),
        "case {case:?}: subscribe must be answered subscribe_success, got {subscribed:?}"
    );
    for event in [
        json!({"m": "a", "keep": true}),
        json!({"m": "b", "keep": true}),
        json!({"m": "c", "keep": false}),
        json!({"m": "d", "keep": true}),
    ] {
        let (status, body) = publish(
            case,
            &api,
            &format!("/{namespace}/room1"),
            &[event],
            Some(&api.api_key),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a publish must be answered 200, got {status} {body}"
        );
    }
    assert_eq!(
        delivered(case, &mut socket, 3).await.len(),
        3,
        "case {case:?}: exactly the three events the handler returned are delivered"
    );
    quiet(case, "no fourth event, the one the handler filtered out", &mut socket).await;
    let unsubscribed = exchange(case, &mut socket, &json!({"type": "unsubscribe", "id": "sub-1"})).await;
    assert_eq!(
        unsubscribed.as_ref().map(|frame| frame["type"].clone()),
        Some(json!("unsubscribe_success")),
        "case {case:?}: unsubscribe must be answered unsubscribe_success, got {unsubscribed:?}"
    );

    // The namespace-scoped series, each under exactly [EventAPIId, ChannelNamespaceName].
    let namespace_dimensions = [
        ("EventAPIId", api.api_id.as_str()),
        ("ChannelNamespaceName", namespace),
    ];
    let expected_sums: &[(&str, f64)] = &[
        ("SubscribeSuccess", 1.0),
        ("UnsubscribeSuccess", 1.0),
        ("PublishHandlerInvocations", 4.0),
        ("SuccessfulEvents", 4.0),
        ("DroppedEvents", 1.0),
        ("FailedEvents", 0.0),
    ];
    for (metric, expected) in expected_sums {
        let summed = metric_sum(&router, metric, &namespace_dimensions).await;
        assert_eq!(
            summed,
            Some(*expected),
            "case {case:?}: expected {metric} to sum to {expected} under \
             [EventAPIId, ChannelNamespaceName], got {summed:?}"
        );
    }

    // The connection-scoped series, each under exactly [EventAPIId]: the connection is
    // still open, so the gauges read one connection and no subscription.
    let api_dimensions = [("EventAPIId", api.api_id.as_str())];
    let connected_sum = metric_sum(&router, "ConnectSuccess", &api_dimensions).await;
    assert_eq!(
        connected_sum,
        Some(1.0),
        "case {case:?}: expected one ConnectSuccess under [EventAPIId], got {connected_sum:?}"
    );
    let subscriptions = statistics(&router, "ActiveSubscriptions", &api_dimensions).await;
    assert!(
        subscriptions
            .iter()
            .any(|point| point["Minimum"].as_f64() == Some(0.0)),
        "case {case:?}: expected an ActiveSubscriptions reading of 0 after the unsubscribe, \
         got {subscriptions:?}"
    );
    let connections = statistics(&router, "ActiveConnections", &api_dimensions).await;
    assert!(
        connections
            .iter()
            .any(|point| point["Maximum"].as_f64().is_some_and(|value| value >= 1.0)),
        "case {case:?}: expected an ActiveConnections reading of at least 1 while the \
         connection is open, got {connections:?}"
    );

    // Which dimension set each metric lands under is itself the contract: a connection
    // belongs to no namespace, and what happens on a namespace is not a property of the
    // API at large.
    let namespace_scoped: Vec<&str> = expected_sums.iter().map(|(metric, _)| *metric).collect();
    let connection_scoped = [
        "ConnectSuccess",
        "DisconnectSuccess",
        "ActiveConnections",
        "ActiveSubscriptions",
    ];
    for (metric, dimensions) in catalogued(&router).await {
        if namespace_scoped.contains(&metric.as_str()) {
            assert_eq!(
                dimensions,
                vec![
                    "ChannelNamespaceName".to_string(),
                    "EventAPIId".to_string()
                ],
                "case {case:?}: {metric} belongs to one channel namespace of one API"
            );
        } else if connection_scoped.contains(&metric.as_str()) {
            assert_eq!(
                dimensions,
                vec!["EventAPIId".to_string()],
                "case {case:?}: {metric} belongs to the API and to no channel namespace"
            );
        } else {
            panic!("case {case:?}: AWS/AppSync carries a series this plane does not publish: {metric}");
        }
    }

    // Closing the connection is what owes a DisconnectSuccess: it is published when the
    // connection ends, not when it was opened.
    let _ = socket.close(None).await;
    let mut disconnected = None;
    for _ in 0..40 {
        disconnected = metric_sum(&router, "DisconnectSuccess", &api_dimensions).await;
        if disconnected.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        disconnected,
        Some(1.0),
        "case {case:?}: expected one DisconnectSuccess under [EventAPIId] once the \
         connection closed, got {disconnected:?}"
    );
}

#[tokio::test]
async fn a_deleted_api_stops_answering_at_the_addresses_it_published() {
    let router = emulator();
    let api = create_event_api(&router, "Deleted").await;
    create_namespace(&router, &api.api_id, "default", None).await;
    let case = "the API's own endpoints";

    // The reader reports the same addresses the create published, and both answer there.
    let (status, fetched) = appsync(
        &router,
        "GET",
        &format!("/v2/apis/{}", api.api_id),
        None,
    )
    .await;
    assert_eq!(status, 200, "GetApi must answer 200, got {fetched}");
    assert_eq!(
        fetched["api"]["dns"]["HTTP"].as_str(),
        Some(api.http.as_str()),
        "case {case:?}: GetApi must report the stored HTTP endpoint, got {fetched}"
    );
    assert_eq!(
        fetched["api"]["dns"]["REALTIME"].as_str(),
        Some(api.realtime.as_str()),
        "case {case:?}: GetApi must report the stored real-time endpoint, got {fetched}"
    );
    let (status, body) = publish(
        case,
        &api,
        "/default/room1",
        &[json!({"m": "a"})],
        Some(&api.api_key),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the published HTTP endpoint serves a publish, got {status} {body}"
    );

    let (status, deleted) = appsync(
        &router,
        "DELETE",
        &format!("/v2/apis/{}", api.api_id),
        None,
    )
    .await;
    assert_eq!(status, 200, "DeleteApi must answer 200, got {deleted}");

    // The sockets are gone with the record: nothing answers at either address.
    for authority in [&api.http, &api.realtime] {
        let dialled = tokio::time::timeout(
            FRAME_DEADLINE,
            TcpStream::connect(("127.0.0.1", loopback_port(case, authority))),
        )
        .await;
        let unreachable = match dialled {
            // A port nothing listens on refuses the connection, and a connection the
            // core severed reads EOF: either way, no request is served.
            Err(_elapsed) => true,
            Ok(Err(_refused)) => true,
            Ok(Ok(mut stream)) => {
                let _ = stream.write_all(b"GET / HTTP/1.1\r\nhost: x\r\n\r\n").await;
                let mut read = Vec::new();
                tokio::time::timeout(FRAME_DEADLINE, stream.read_to_end(&mut read))
                    .await
                    .is_ok_and(|_| read.is_empty())
            }
        };
        assert!(
            unreachable,
            "case {case:?}: a deleted API answers nothing at {authority:?}"
        );
    }
}

#[tokio::test]
async fn each_endpoint_authorizes_only_the_providers_its_api_configured_for_the_call() {
    // The rule: material authorizes a call only where the API set that provider up FOR
    // THAT CALL — `EventConfig`'s `connectionAuthModes`, `defaultPublishAuthModes` and
    // `defaultSubscribeAuthModes`, with a channel namespace's own `publishAuthModes` /
    // `subscribeAuthModes` overriding the defaults for its channels.
    let router = emulator();

    // One API configured for `API_KEY` alone, and one for `AWS_IAM` alone, both with both
    // providers set up (an auth mode must name a provider the API configured).
    let providers = json!([{"authType": "API_KEY"}, {"authType": "AWS_IAM"}]);
    let only_key = create_configured_event_api(
        &router,
        "KeyOnly",
        json!({
            "authProviders": providers,
            "connectionAuthModes": [{"authType": "API_KEY"}],
            "defaultPublishAuthModes": [{"authType": "API_KEY"}],
            "defaultSubscribeAuthModes": [{"authType": "API_KEY"}],
        }),
    )
    .await;
    let only_iam = create_configured_event_api(
        &router,
        "IamOnly",
        json!({
            "authProviders": providers,
            "connectionAuthModes": [{"authType": "AWS_IAM"}],
            "defaultPublishAuthModes": [{"authType": "AWS_IAM"}],
            "defaultSubscribeAuthModes": [{"authType": "AWS_IAM"}],
        }),
    )
    .await;
    create_namespace(&router, &only_key.api_id, "default", None).await;
    create_namespace(&router, &only_iam.api_id, "default", None).await;
    // A namespace of the IAM-only API that overrides the publish modes with `API_KEY`.
    create_namespace_body(
        &router,
        &only_iam.api_id,
        json!({"name": "keyed", "publishAuthModes": [{"authType": "API_KEY"}]}),
    )
    .await;

    let signature = "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/us-west-2/appsync/\
                     aws4_request, SignedHeaders=host, Signature=0000";

    // The handshake: each API admits the material its own connection modes name, and
    // refuses the other.
    let case = "a connection presenting a key on the API_KEY-only API";
    assert!(
        handshake(
            case,
            &only_key,
            "/event/realtime",
            &offered_subprotocols(&only_key.http, Some(&only_key.api_key)),
        )
        .await
        .is_ok(),
        "case {case:?}: the configured provider's material must be admitted"
    );
    let case = "a connection presenting a signature on the API_KEY-only API";
    assert_eq!(
        handshake(
            case,
            &only_key,
            "/event/realtime",
            &auth_subprotocols(&json!({"host": only_key.http, "Authorization": signature})),
        )
        .await
        .err(),
        Some(401),
        "case {case:?}: a provider the API did not configure for connecting authorizes nothing"
    );
    let case = "a connection presenting a signature on the AWS_IAM-only API";
    assert!(
        handshake(
            case,
            &only_iam,
            "/event/realtime",
            &auth_subprotocols(&json!({"host": only_iam.http, "Authorization": signature})),
        )
        .await
        .is_ok(),
        "case {case:?}: the configured provider's material must be admitted"
    );
    let case = "a connection presenting that API's own key on the AWS_IAM-only API";
    assert_eq!(
        handshake(
            case,
            &only_iam,
            "/event/realtime",
            &offered_subprotocols(&only_iam.http, Some(&only_iam.api_key)),
        )
        .await
        .err(),
        Some(401),
        "case {case:?}: a minted key authorizes nothing on an API that configured no \
         API_KEY mode for connecting"
    );

    // The publish: the same rule, over the publish modes — and a namespace's own override
    // replaces the API's default for its channels.
    let events = vec![json!(json!({"m": "a"}).to_string())];
    /// One publish row: which API and channel it addresses, the fields it presents, and
    /// the status it must be answered with.
    struct PublishAuthRow<'a> {
        case: &'a str,
        api: &'a EventApi,
        channel: &'a str,
        fields: Vec<(&'a str, &'a str)>,
        expected: u16,
    }
    let rows = vec![
        PublishAuthRow {
            case: "a publish presenting a key where the namespace takes the API_KEY default",
            api: &only_key,
            channel: "/default/room1",
            fields: vec![("x-api-key", only_key.api_key.as_str())],
            expected: 200,
        },
        PublishAuthRow {
            case: "a publish presenting a signature where only API_KEY is configured",
            api: &only_key,
            channel: "/default/room1",
            fields: vec![("authorization", signature)],
            expected: 401,
        },
        PublishAuthRow {
            case: "a publish presenting a signature where only AWS_IAM is configured",
            api: &only_iam,
            channel: "/default/room1",
            fields: vec![("authorization", signature)],
            expected: 200,
        },
        PublishAuthRow {
            case: "a publish presenting a key where only AWS_IAM is configured",
            api: &only_iam,
            channel: "/default/room1",
            fields: vec![("x-api-key", only_iam.api_key.as_str())],
            expected: 401,
        },
        PublishAuthRow {
            case: "a publish presenting a key to the namespace that overrides with API_KEY",
            api: &only_iam,
            channel: "/keyed/room1",
            fields: vec![("x-api-key", only_iam.api_key.as_str())],
            expected: 200,
        },
        PublishAuthRow {
            case: "a publish presenting a signature to that same overriding namespace",
            api: &only_iam,
            channel: "/keyed/room1",
            fields: vec![("authorization", signature)],
            expected: 401,
        },
    ];
    for row in rows {
        let case = row.case;
        let (status, body) = post_with_fields(
            case,
            row.api,
            "/event",
            "POST",
            &json!({"channel": row.channel, "events": events.clone()}),
            &row.fields,
        )
        .await;
        assert_eq!(
            status, row.expected,
            "case {case:?}: expected {}, got {status} {body}",
            row.expected
        );
    }
}
