// SPDX-License-Identifier: CC0-1.0

//! The SOCKET a WEBSOCKET api is served on: the core-owned listener its `ApiEndpoint`
//! announces, the RFC 6455 opening handshake it completes, and the frames it then carries.
//!
//! A WebSocket api cannot be served through the emulator's front door: that door serves ONE
//! buffered request per call, and the seams a service is given (`Service::can_handle`,
//! `Service::override_operation`, `Service::override_routing_error`) take an already-buffered
//! request and return a buffered response, so nothing there can switch a connection to
//! another protocol and carry frames over it. What a WebSocket api needs is a socket, which
//! is exactly what the core lends a service for a resource with a native protocol:
//! `engine.listeners().tcp_plaintext(handler)` binds it on the operator-chosen interface
//! (loopback by default), assigns the port, caps the connections and severs them when the
//! handle is dropped, and hands this module each accepted connection's byte stream. The
//! address it reports is what the api announces as its `ApiEndpoint`, so the URI a caller
//! hands a WebSocket client library is one that client can reach
//! ([`crate::state::websocket_transport`]).
//!
//! # What this module does with a connection, in order
//!
//! 1. ENCRYPTION, negotiated in band. The socket is handed over plaintext and the first byte
//!    says what the client opened with: a TLS record is handed to the core's own TLS server
//!    and everything after it is read decrypted, while an HTTP method is read as it stands
//!    (the tree's one reading of that, [`::service_common::in_band_tls`]). So ONE announced
//!    address serves both the `wss://` a client of AWS dials and the `ws://` a client that
//!    does not verify a self-signed certificate dials, and the service never touches a key
//!    or a certificate.
//! 2. The HANDSHAKE's own bytes, read here before the WebSocket implementation sees them:
//!    the decision this connection needs is AWAITED — the `$connect` route's integration
//!    and the authorizer in front of it are each a Lambda invocation — while that
//!    implementation's handshake callback is synchronous, so the request head is read and
//!    parsed first ([`Offer`], by the maintained `httparse`), the decision is made, and
//!    every byte read is then REPLAYED to the implementation, which derives
//!    `Sec-WebSocket-Accept` and writes either `101 Switching Protocols` or the refusal that
//!    decision carries ([`admission`]). Nothing about the protocol is hand-written here.
//!    A request that is NOT an opening handshake is served as the ordinary HTTP it is
//!    ([`super::endpoint_http`]): this socket is the api's whole `ApiEndpoint`, and the
//!    `@connections` management API is published at that same endpoint.
//! 3. The FRAMES, each dispatched to the route the api's `RouteSelectionExpression` selects
//!    and answered by that route's integration PERFORMED ([`super::integration`]), and each
//!    frame pushed TO the connection written back over the same socket. A message this plane
//!    does NOT serve is classified before any route is selected, and refused with the close
//!    its refusal carries rather than reaching a backend ([`refused`]): a client data frame
//!    that is not text, and a message past the frame quota ([`MAX_FRAME_BYTES`]) or the
//!    per-message ceiling ([`MAX_MESSAGE_BYTES`]).
//! 4. The CLOSE, which invokes the `$disconnect` route's integration and ends the
//!    connection's record: from then on its `ConnectionId` addresses a connection that no
//!    longer exists.
//!
//! Every answer is computed from this service's own records — the api, its stage, the
//! configuration that stage's deployment froze — read under the one shared store lock, and
//! never while an integration is awaited; the authorization the connection request's route
//! carries is applied by the SAME rule the service endpoint applies it by
//! ([`super::connect_authorization`]).

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use ::emulator_server::{ListenerConn, NativeHandler, TlsUpgrader};
use ::futures_util::{SinkExt, StreamExt};
use ::service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi::state as connections;
use ::service_common::in_band_tls::{negotiated, Prefixed};
use ::smithy_server_runtime::EmbeddedEmulator;
use ::tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use ::tokio::sync::mpsc::{channel, Receiver, Sender};
use ::tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as HandshakeRequest, Response as HandshakeResponse,
};
use ::tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use ::tokio_tungstenite::tungstenite::protocol::CloseFrame;
use ::tokio_tungstenite::tungstenite::{Error as SocketError, Message};

use super::event::{Call, Connection};
use super::{access_logs, dispatch, event, integration};
// The `AWS/ApiGateway` series this stage publishes about the traffic it serves: a frame
// carried over this socket leaves no response a later caller can read, so the connections,
// the messages, the backend latency and the failures are reported there as they happen.
use crate::service::metrics::{self, CallOutcome, CloudWatchState, StageMetrics};
use crate::state::ComAmazonawsApigatewayv2Apigatewayv2State as ServiceStore;

/// The member a gateway frame carries its reason under, and the two members it names the
/// call by: API Gateway's own frames for a message it cannot route or serve are
/// `{"message": …, "connectionId": …, "requestId": …}`.
const FRAME_MESSAGE_MEMBER: &str = "message";
const FRAME_CONNECTION_ID_MEMBER: &str = "connectionId";
const FRAME_REQUEST_ID_MEMBER: &str = "requestId";

/// Open the socket `api_id`'s WebSocket data plane is served on and remember it, answering
/// the `host:port` it landed on — the authority that api's `ApiEndpoint` announces.
///
/// The core owns the socket, the interface, the port and the connection caps; this hands it
/// the handler and stores the returned handle in the service's own state, which is what keeps
/// the port open for as long as the api exists. `Err` when this service was mounted on an
/// engine with no listener surface, or when the core could not bind: the caller then answers
/// that rather than announcing an address nothing serves.
///
/// `callback` is the in-process transport this api's routes are performed over — the handle
/// the operation that mints the api is given (`OperationContext::callback`), kept for the
/// socket's whole life because a frame arriving on it has no request and no context of its
/// own, and a route's integration is a call into the service that owns the backend.
pub async fn open_for_api<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: EmbeddedEmulator,
    api_id: &str,
) -> Result<String, String> {
    let shared = (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .ok_or_else(|| "this emulator's engine has no listener surface".to_string())?;
    let state = crate::state::service_state(engine)
        .ok_or_else(|| "this emulator's engine does not carry this service's store".to_string())?;
    let listeners = shared.listeners();
    let registry = connections::registry(engine).ok_or_else(|| {
        "this emulator's engine does not carry the WebSocket connection registry".to_string()
    })?;
    let handler = StageSocket {
        state: state.clone(),
        registry: registry.clone(),
        api_id: api_id.to_string(),
        callback,
        // The CloudWatch store this api's stages publish their own `AWS/ApiGateway` series
        // to. `None` on an engine carrying no CloudWatch: the data plane then serves every
        // call as it does otherwise and nothing is published.
        metrics: metrics::cloudwatch_state(engine),
        // The core's own TLS server, for a client that dialled `wss://`. A core that cannot
        // prepare it leaves this `None`, and such a client is then refused at the first byte
        // rather than being answered in the clear.
        upgrader: listeners.tls_upgrader().ok(),
    };
    let listener = listeners
        .tcp_plaintext(handler)
        .await
        .map_err(|error| format!("the api's WebSocket socket could not be opened: {error}"))?;
    let authority = state
        .lock()
        .websocket_transport
        .remember_listener(api_id, Arc::new(listener));
    // The address this socket landed on is the api's whole `ApiEndpoint`, and the
    // `@connections` management API is published at that same endpoint, so the registry is
    // told which address serves this api's connections: a call made THERE addresses exactly
    // the connections established there, and another api's live id is `GoneException` on it.
    connections::serve_connections_at(&registry, &authority);
    tracing::info!(%api_id, %authority, "opened a WebSocket api's data-plane socket");
    Ok(authority)
}

/// The handler the core hands each connection accepted on ONE api's socket.
///
/// It carries the shared store (so every connection reads the same records the control plane
/// writes), the WebSocket CONNECTION REGISTRY the `@connections` management API serves (so a
/// connection this socket carries is the connection that API reports), and the api its
/// socket belongs to — the api is known from the socket itself, so no host or path has to
/// name it.
struct StageSocket {
    state: ::engine::ServiceState<ServiceStore>,
    registry: ::engine::ServiceState<connections::ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState>,
    api_id: String,
    callback: EmbeddedEmulator,
    /// The store the stage's own `AWS/ApiGateway` series are appended to, or `None` when this
    /// emulator carries no CloudWatch.
    metrics: Option<::engine::ServiceState<CloudWatchState>>,
    upgrader: Option<TlsUpgrader>,
}

impl NativeHandler for StageSocket {
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        // Owned clones before the future: the core's handler contract hands back a `'static`
        // future, so nothing may borrow `&self` across it.
        let state = self.state.clone();
        let registry = self.registry.clone();
        let api_id = self.api_id.clone();
        let callback = self.callback.clone();
        let metrics = self.metrics.clone();
        let upgrader = self.upgrader.clone();
        Box::pin(async move {
            serve_connection(state, registry, api_id, callback, metrics, upgrader, conn).await;
        })
    }
}

/// Serve one accepted connection: negotiate encryption, decide the handshake, complete (or
/// refuse) it, then carry frames until either side closes.
// The handshake callback's `Err` is the REFUSAL RESPONSE the WebSocket implementation writes
// (`http::Response<Option<String>>`), which is that dependency's own signature and is large
// by the `http` crate's measure. Boxing it is not available: the callback's type is the one
// `accept_hdr_async_with_config` takes.
#[allow(clippy::result_large_err)]
async fn serve_connection(
    state: ::engine::ServiceState<ServiceStore>,
    registry: ::engine::ServiceState<connections::ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState>,
    api_id: String,
    callback: EmbeddedEmulator,
    metrics: Option<::engine::ServiceState<CloudWatchState>>,
    upgrader: Option<TlsUpgrader>,
    conn: ListenerConn,
) {
    let Some(mut conn) = negotiated(conn, upgrader).await else {
        return;
    };
    // The request head is read HERE, before the WebSocket implementation sees it, because
    // the decision below is AWAITED; every byte read is replayed to that implementation, so
    // the handshake it completes is the one the client sent. See the module docs.
    let Some(head) = request_head(&mut conn).await else {
        return;
    };
    let Some(offer) = Offer::of(&head) else {
        tracing::debug!(%api_id, "a connection opened with no readable HTTP request head");
        return;
    };
    if !offer.is_opening_handshake() {
        // Not a handshake: this socket is the api's whole `ApiEndpoint`, and the
        // `@connections` management API is published at that same endpoint, so the request
        // is served as the ordinary HTTP it is — with every byte already read replayed, so
        // the HTTP implementation reads the request the client sent. Whatever the verdict,
        // such a request is ANSWERED rather than dropped.
        let conn: ListenerConn = Box::new(Prefixed::new(head, conn));
        super::endpoint_http::serve(callback, conn).await;
        return;
    }
    // One id for the CONNECTION REQUEST, minted by this socket: the event the `$connect`
    // route's integration is handed reports it, and so does the stage's access-log record of
    // the same call.
    let request_id = crate::service::invoke::proxy_event::mint_request_id();
    let started = Instant::now();
    let decision = admission(&state, &callback, metrics, &api_id, &offer, &request_id).await;
    let (admitted, refusal) = match &decision.outcome {
        Ok(admitted) => (Some(admitted.clone()), None),
        Err(refusal) => (None, Some(refusal.clone())),
    };
    let logged_refusal = refusal.clone();
    let conn: ListenerConn = Box::new(Prefixed::new(head, conn));
    let decide = move |_request: &HandshakeRequest,
                       response: HandshakeResponse|
     -> Result<HandshakeResponse, ErrorResponse> {
        match &refusal {
            None => Ok(response),
            Some(refusal) => Err(refusal.rendered()),
        }
    };
    let completed = match ::tokio_tungstenite::accept_hdr_async_with_config(
        conn,
        decide,
        Some(socket_limits()),
    )
    .await
    {
        Ok(socket) => Some(socket),
        Err(error) => {
            // Either the refusal above (already written to the client by the handshake) or a
            // client that did not complete the handshake at all.
            tracing::debug!(%api_id, %error, "a WebSocket opening handshake was not completed");
            None
        }
    };
    // The STAGE's record of the connection request, written once the client has been answered
    // — the status it was answered with is part of it: the `101` only where the handshake was
    // COMPLETED, the refusal's own status where the gateway refused it, and none at all for a
    // client that was admitted and then did not finish the handshake, where nothing was
    // answered to report. A handshake that resolved no stage of this api resolved no
    // `AccessLogSettings` either, and leaves no record.
    if let Some(connection) = decision.connection.as_ref() {
        let status = match (&logged_refusal, &completed) {
            (Some(refusal), _) => Some(refusal.status),
            (None, Some(_)) => Some(SWITCHING_PROTOCOLS),
            (None, None) => None,
        };
        access_logs::delivered(
            &state,
            &callback,
            connection,
            &access_logs::Served {
                call: Call::Connect,
                route_key: dispatch::CONNECT_ROUTE_KEY.to_string(),
                request_id,
                status,
                response_length: logged_refusal.as_ref().and_then(Refusal::body_length),
                latency: access_logs::latency_since(started),
                error_message: logged_refusal.as_ref().and_then(Refusal::logged_message),
            },
        )
        .await;
    }
    let (Some(socket), Some(accepted)) = (completed, admitted) else {
        return;
    };
    carry_frames(state, registry, callback, accepted, socket).await;
}

/// The status a COMPLETED opening handshake carries (RFC 6455 §4.1: the server's handshake
/// is a `101 Switching Protocols`), which is what the stage's record of an admitted
/// connection request reports as the status it was answered with.
const SWITCHING_PROTOCOLS: u16 = 101;

/// The bytes of one HTTP request HEAD, read up to and including the blank line that ends it,
/// or `None` for a client that sent none within [`MAX_HEAD_BYTES`].
///
/// Reading stops at the end of the head, and whatever arrived in the same chunk is kept in
/// the answer: the caller replays every byte read, so a client that sent more behind its
/// handshake loses none of it.
async fn request_head(conn: &mut ListenerConn) -> Option<Vec<u8>> {
    /// The blank line that ends an HTTP request head (RFC 9112 §2.1).
    const HEAD_TERMINATOR: &[u8] = b"\r\n\r\n";
    let mut head: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 1024];
    while head.len() < MAX_HEAD_BYTES {
        match conn.read(&mut chunk).await {
            Ok(0) | Err(_) => return None,
            Ok(read) => head.extend_from_slice(&chunk[..read]),
        }
        if head
            .windows(HEAD_TERMINATOR.len())
            .any(|window| window == HEAD_TERMINATOR)
        {
            return Some(head);
        }
    }
    None
}

/// How many bytes of request head this data plane reads before giving up on a client.
///
/// Stated here rather than left to a default, so the bound a caller's bytes are buffered
/// under is citable: it is what stands between one local caller and the host's memory, since
/// the head is read into a buffer sized by what that caller sent. It is far above the few
/// hundred bytes of an RFC 6455 opening handshake, and above the 8 KiB of request line and
/// fields a server is conventionally held to.
const MAX_HEAD_BYTES: usize = 16 * 1024;

/// How many header fields of one opening handshake are read. Well above the handful a
/// WebSocket client sends; a client that sends more is not served at all, rather than served
/// from a part of what it said.
const MAX_HEADER_FIELDS: usize = 96;

/// The OPENING HANDSHAKE as this data plane reads it: what it addressed, and what it said.
///
/// Parsed by the maintained HTTP/1 parser (`httparse`, the one the WebSocket implementation
/// itself reads a handshake with), so no request grammar is written here.
#[derive(Debug, Clone)]
struct Offer {
    /// The method the request line names, kept because what distinguishes an opening
    /// handshake from an ordinary request begins with it (RFC 6455 §4.1: a `GET`).
    method: String,
    /// The request target as written: the path, and any query string after it.
    target: String,
    /// Every header field, under the name the client sent it with; a field that appeared
    /// more than once is one comma-separated value (RFC 9110 §5.3). The spelling is KEPT
    /// because it is what the event reports (`super::event`), and every lookup here is
    /// case-insensitive instead.
    headers: BTreeMap<String, String>,
}

impl Offer {
    /// The handshake `head` states, or `None` when those bytes are no complete HTTP request.
    fn of(head: &[u8]) -> Option<Self> {
        let mut fields = [::httparse::EMPTY_HEADER; MAX_HEADER_FIELDS];
        let mut request = ::httparse::Request::new(&mut fields);
        if !matches!(
            request.parse(head),
            Ok(::httparse::Status::Complete(_consumed))
        ) {
            return None;
        }
        let target = request.path?.to_string();
        let method = request.method?.to_string();
        let mut headers: BTreeMap<String, String> = BTreeMap::new();
        for field in request.headers.iter() {
            let Ok(value) = std::str::from_utf8(field.value) else {
                continue;
            };
            // A field the client repeated is one comma-separated value, under the spelling
            // it first used.
            match headers
                .iter_mut()
                .find(|(name, _)| name.eq_ignore_ascii_case(field.name))
            {
                Some((_, held)) => {
                    held.push(',');
                    held.push_str(value.trim());
                }
                None => {
                    headers.insert(field.name.to_string(), value.trim().to_string());
                }
            }
        }
        Some(Self {
            method,
            target,
            headers,
        })
    }

    /// Whether these bytes are an RFC 6455 OPENING HANDSHAKE, by the one reading of that
    /// question this service has ([`super::Handshake::is_opening`]) — so the socket and the
    /// service endpoint cannot disagree about what a handshake is. Everything else on this
    /// socket is an ordinary HTTP request ([`super::endpoint_http`]).
    fn is_opening_handshake(&self) -> bool {
        super::Handshake::of_fields(
            &self.method,
            event::field(&self.headers, ::http::header::HOST.as_str()).unwrap_or_default(),
            self.path(),
            event::field(&self.headers, "connection"),
            event::field(&self.headers, "upgrade"),
            event::field(&self.headers, super::SEC_WEBSOCKET_KEY),
        )
        .is_opening()
    }

    /// The value this handshake carries for the header `name`, read case-insensitively as a
    /// header name is.
    fn header(&self, name: &str) -> Option<String> {
        event::field(&self.headers, name).map(str::to_string)
    }

    /// The path the handshake addressed, with its query string removed.
    fn path(&self) -> &str {
        super::super::data_plane::path_of(&self.target)
    }
}

/// The largest MESSAGE a client may send over a WEBSOCKET api, in bytes: the 128 KiB
/// per-message ceiling an API Gateway WebSocket api enforces on the way in, which is also the
/// modelled maximum of a payload pushed the other way (`PostToConnection$Data`,
/// `@length(max: 131072)`).
///
/// It is a ceiling of the GATEWAY, not of the implementation that reads the socket: a message
/// past it — a fragmented message whose ASSEMBLED payload goes past it included — reaches no
/// route and is answered with the close that states the ceiling ([`refused`],
/// [`CloseCode::Size`]).
const MAX_MESSAGE_BYTES: usize = 131_072;

/// The largest FRAME a client may send over a WEBSOCKET api, in bytes: the 32 KiB an API
/// Gateway WebSocket api publishes as its frame quota, so a message at [`MAX_MESSAGE_BYTES`]
/// is fragmented and no single frame may be larger. A frame past it is refused with the same
/// close the ceiling carries, and the reader is what stops at it.
const MAX_FRAME_BYTES: usize = 32 * 1024;

/// How many bytes of ASSEMBLED fragments this socket's reader buffers for one message before
/// it stops reading: twice [`MAX_MESSAGE_BYTES`].
///
/// It sits ABOVE the gateway's own per-message ceiling on purpose. A message that passes that
/// ceiling is REFUSED BY THE GATEWAY, with the close that states the ceiling, and a refusal
/// the client reads is one written over a socket whose bytes were read in full — so the reader
/// must not be the thing that stops first for a message only a little past the ceiling. What
/// is left here is the backstop that stands between one local caller and the host's memory,
/// since a message is read into a buffer sized by what that caller sent; it is stated rather
/// than left to the WebSocket implementation's own default, so the bound a caller's bytes are
/// buffered under is citable and survives a dependency bump.
const READ_CEILING_BYTES: usize = 2 * MAX_MESSAGE_BYTES;

/// How many bytes may wait on the WRITE side of one socket before the implementation stops
/// buffering: the depth of the pushes queued for a connection
/// ([`connections::CONNECTION_PUSH_DEPTH`]) times the largest payload one carries, so a
/// client that stops reading cannot pin more than the queue it is already bounded by.
const MAX_WRITE_BUFFER_BYTES: usize = connections::CONNECTION_PUSH_DEPTH * MAX_MESSAGE_BYTES;

/// The explicit size bounds every socket of this data plane is read and written under; see
/// the constants above.
fn socket_limits() -> ::tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
    ::tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(READ_CEILING_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES))
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_WRITE_BUFFER_BYTES)
}

/// What the handshake resolved: the CONNECTION it offered, as every event of that
/// connection reports it, and the publisher of the serving stage's own metric series.
#[derive(Debug, Clone)]
struct Admitted {
    connection: Connection,
    /// Where every later call of this connection is reported: the stage that admitted it is
    /// the stage its frames are counted under, so the publisher resolved for the handshake is
    /// carried on rather than resolved again per frame.
    metrics: StageMetrics,
}

/// How a handshake this endpoint does not complete is answered: an HTTP status, and the
/// body the client reads with it.
#[derive(Debug, Clone)]
struct Refusal {
    status: u16,
    /// What the client reads: `Gateway` for a refusal the GATEWAY states, which carries the
    /// `{"message": …}` body API Gateway's own endpoint answers with, and `Backend` for one
    /// the connection request's own integration decided, which carries that integration's
    /// own body — "the response body is the $connect integration's own body bytes".
    body: RefusalBody,
}

/// The two bodies a refused handshake can carry; see [`Refusal::body`].
#[derive(Debug, Clone)]
enum RefusalBody {
    Gateway(String),
    Backend(Option<String>),
}

impl Refusal {
    /// A refusal the GATEWAY states, with the sentence it words it in.
    fn gateway(status: u16, message: &str) -> Self {
        Self {
            status,
            body: RefusalBody::Gateway(message.to_string()),
        }
    }

    /// A refusal the CONNECTION REQUEST's own integration decided: its status, and its own
    /// body relayed to the client.
    ///
    /// The body travels only when the integration's bytes are text, which is what an HTTP
    /// refusal body is and what the handshake implementation writes; a reply that is not
    /// text is answered with the status alone rather than with bytes altered to fit.
    fn from_backend(status: u16, body: Option<Vec<u8>>) -> Self {
        Self {
            status,
            body: RefusalBody::Backend(body.and_then(|bytes| String::from_utf8(bytes).ok())),
        }
    }

    /// The handshake refusal as the RFC 6455 implementation writes it: the status, and the
    /// body with the media type that body is. A status the `http` crate does not build is
    /// answered as the internal failure it is.
    fn rendered(&self) -> ErrorResponse {
        let mut response = ::http::Response::builder().status(self.status);
        let body = match &self.body {
            RefusalBody::Gateway(message) => {
                response = response.header(
                    "content-type",
                    super::super::data_plane::JSON_CONTENT_TYPE,
                );
                Some(::serde_json::json!({ FRAME_MESSAGE_MEMBER: message }).to_string())
            }
            // The integration's own bytes, relayed as it returned them: nothing here says
            // what they are, so no media type is stated for them.
            RefusalBody::Backend(body) => body.clone(),
        };
        response.body(body).unwrap_or_else(|_| {
            ::http::Response::builder()
                .status(super::super::data_plane::UNPLACEABLE_STATUS)
                .body(None)
                .expect("500 with no body builds")
        })
    }

    /// How many bytes the client read with this refusal, which is what the stage's record
    /// reports as the length of what it was answered with.
    fn body_length(&self) -> Option<usize> {
        self.rendered().body().as_ref().map(String::len)
    }

    /// The sentence this refusal was worded in, as `$context.error.message` reports it: the
    /// gateway's own for a refusal it stated, and the `message` member of the body for one
    /// the connection request's integration decided — which is where an integration that
    /// refuses a client words its reason.
    fn logged_message(&self) -> Option<String> {
        match &self.body {
            RefusalBody::Gateway(message) => Some(message.clone()),
            RefusalBody::Backend(body) => ::serde_json::from_str::<::serde_json::Value>(
                body.as_deref().unwrap_or_default(),
            )
            .ok()
            .as_ref()
            .and_then(|document| document.get(FRAME_MESSAGE_MEMBER))
            .and_then(|message| message.as_str().map(str::to_string)),
        }
    }
}

/// Whether this opening handshake is completed, from the api's own records and from what the
/// connection request's own backend answered.
///
/// The steps, in the order API Gateway applies them, with the core's own browser check in
/// front of them:
///
/// 0. a handshake a BROWSER made from another site is refused before anything of the api is
///    read ([`browser_refusal`]) — the check the core applies to every request on one of its
///    own HTTP listeners, applied here because an RFC 6455 handshake is a request a page can
///    make to a loopback port without a CORS preflight;
/// 1. the api's `DisableExecuteApiEndpoint` has not turned this endpoint off — this socket is
///    the DEFAULT endpoint the api announces, so while the member is set every handshake on it
///    is refused `403` `{"message":"Forbidden"}` and it accepts again as soon as the member is
///    cleared;
/// 2. the path names a stage THIS api holds, and that stage has a deployment released to it;
///    a step that resolves nothing is `403` `{"message":"Forbidden"}`, which is what the
///    `execute-api` endpoint answers for a stage it does not serve;
/// 3. the AUTHORIZATION the `$connect` route carries is applied before anything that route
///    fronts ([`super::connect_authorization`]): a handshake missing an identity source is
///    `401` `{"message":"Unauthorized"}` with the authorizer never invoked, and past that
///    precondition the authorizer's own function decides — a DENY is `403`
///    ([`super::connect_authorizer`]), and what it ALLOWS travels with the connection into
///    every later call's event;
/// 4. the CONNECTION REQUEST is served by the `$connect` route's integration, PERFORMED///    ([`super::integration`]), and the handshake completes only on a `2xx` reply. A reply
///    OUTSIDE `2xx` refuses the handshake with the status that integration answered and the
///    body it answered, because `$connect` is where a backend admits or refuses a client; a
///    reply naming no status, or none at all, is the gateway's own `500`.
///
/// The connection's own id is minted BEFORE the connection request is served, because the
/// event that request is served with carries it: `$connect` is where a handler learns the
/// id it will later address the client by.
///
/// Every step from 2 on is also METERED on the stage it resolved ([`metrics`]): the connection
/// request is one message of the stage, the `$connect` route's integration counts the message
/// it was sent, the integration that served it reports how long it took, a `4xx` refusal is a
/// client error, and a connection request that was not admitted is a route that did not
/// execute.
///
/// Nothing holds the store lock while a backend is awaited: every record the decision needs
/// is read under one lock and owned from then on.
async fn admission(
    state: &::engine::ServiceState<ServiceStore>,
    callback: &EmbeddedEmulator,
    metrics_store: Option<::engine::ServiceState<CloudWatchState>>,
    api_id: &str,
    offer: &Offer,
    request_id: &str,
) -> Decided {
    if let Some(refusal) = browser_refusal(offer) {
        return Decided::refused(None, refusal);
    }
    let stage_name = super::super::data_plane::first_segment(offer.path())
        .unwrap_or_default()
        .to_string();
    let offered = match offered(state, api_id, &stage_name, offer) {
        Ok(offered) => offered,
        Err(refusal) => return Decided::refused(None, refusal),
    };
    // A refusal from here on is METERED, so the publisher is built before the step that can
    // refuse: a handshake refused before a stage of the api resolved belongs to no stage and
    // is counted under none.
    let metrics = StageMetrics::new(metrics_store, api_id, &stage_name);
    let per_route_connect = offered.per_route_connect;
    let route = per_route_connect.as_deref();
    // The connection request reached a deployed stage, which is one message of that stage;
    // `ConnectCount` is published below, where the `$connect` route's integration is sent it.
    metrics.message(route);
    // The refusal that route's own AUTHORIZATION owes by itself, read with the records above
    // and answered here so it is metered on the stage it was refused at.
    if let Some((status, message)) = offered.unauthorized {
        metrics.served(route, refused_connection(status));
        return Decided::refused(
            Some(offered.connection),
            Refusal::gateway(status, message),
        );
    }
    let mut connection = offered.connection;
    if let Some((authorizer, identity)) = offered.authorizer {
        match super::connect_authorizer::decided(callback, &connection, &authorizer, &identity)
            .await
        {
            Ok(decided) => {
                connection.authorizer_context = decided.context;
                connection.authorizer_principal = decided.principal_id;
            }
            Err((status, message)) => {
                // An authorizer that refused, or one this configuration could not run at all,
                // is a route that did not execute.
                metrics.served(route, refused_connection(status));
                return Decided::refused(
                    Some(connection),
                    Refusal::gateway(status, message),
                );
            }
        }
    }
    let admitted = |connection: Connection| Admitted {
        connection,
        metrics: metrics.clone(),
    };
    let Some(connect) = offered.connect else {
        // No `$connect` route: there is nothing to refuse the connection, and no integration
        // was sent the connection request.
        metrics.served(route, CallOutcome::default());
        return Decided::admitted(admitted(connection));
    };
    // The connection request IS sent to the `$connect` route's integration from here.
    metrics.connect_dispatched(route);
    let began = Instant::now();
    let performed =
        integration::perform(callback, &connection, &connect, &Call::Connect, request_id).await;
    match performed {
        Ok(answered) if dispatch::is_success_status(answered.status_code.as_deref()) => {
            metrics.integration_latency(route, began.elapsed());
            metrics.served(route, CallOutcome::default());
            Decided::admitted(admitted(connection))
        }
        Ok(answered) => {
            // The STATUS the integration answered is what the handshake is refused with, and
            // the body it answered is what the client reads: a `$connect` route is where a
            // backend admits or refuses a client, so its own answer is the refusal rather
            // than a status this gateway chose. A reply naming no status at all names no
            // decision, which is the gateway's own `500`.
            let status = answered
                .status_code
                .as_deref()
                .and_then(|status| status.trim().parse::<u16>().ok());
            tracing::info!(
                %api_id,
                stage = %stage_name,
                status = ?answered.status_code,
                "refusing a WebSocket handshake: the connection request's integration did \
                 not reply successfully"
            );
            // The integration ANSWERED — it refused the client rather than failing — so the
            // call has a latency to report, and the refusal is metered by the status that
            // reached the client.
            metrics.integration_latency(route, began.elapsed());
            let refusal = match status {
                Some(status) => {
                    metrics.served(route, refused_connection(status));
                    Refusal::from_backend(status, answered.payload)
                }
                None => {
                    metrics.served(
                        route,
                        refused_connection(super::super::data_plane::UNPLACEABLE_STATUS),
                    );
                    internal_server_error()
                }
            };
            Decided::refused(Some(connection), refusal)
        }
        Err(unserved) => {
            tracing::warn!(
                %api_id,
                stage = %stage_name,
                ?unserved,
                "refusing a WebSocket handshake: the connection request's integration \
                 produced no reply"
            );
            // The integration itself could not answer, which is the one failure that is the
            // INTEGRATION's rather than the client's.
            metrics.served(
                route,
                CallOutcome {
                    integration_error: true,
                    ..refused_connection(super::super::data_plane::UNPLACEABLE_STATUS)
                },
            );
            Decided::refused(Some(connection), internal_server_error())
        }
    }
}

/// What a handshake came to, and what the gateway knows about the connection request it was:
/// the outcome the client is answered with, and the CONNECTION the api's records resolved for
/// it, which is what the stage's own record of the call is written from.
///
/// The connection is reported for a refusal too, because a refused connection request is a
/// call the stage LOGS: only a handshake that resolved no deployed stage of this api resolves
/// no connection at all, and such a handshake reaches no `AccessLogSettings` either.
#[derive(Debug, Clone)]
struct Decided {
    connection: Option<Connection>,
    outcome: Result<Admitted, Refusal>,
}

impl Decided {
    /// A handshake every step let through.
    fn admitted(admitted: Admitted) -> Self {
        Self {
            connection: Some(admitted.connection.clone()),
            outcome: Ok(admitted),
        }
    }

    /// A handshake refused at the step `refusal` carries, with what the gateway had resolved
    /// about the connection request by then.
    fn refused(connection: Option<Connection>, refusal: Refusal) -> Self {
        Self {
            connection,
            outcome: Err(refusal),
        }
    }
}

/// What a CONNECTION REQUEST that was not admitted ended in, by the status the client was
/// refused with: a `4xx` is a client error, and a connection request that was refused at all
/// is a route whose execution did not reach a successful end.
fn refused_connection(status: u16) -> CallOutcome {
    CallOutcome {
        client_error: super::super::data_plane::is_client_error_status(status),
        execution_error: true,
        integration_error: false,
    }
}

/// What the handshake OFFERED, read out of the store under one lock: the connection it would
/// establish, the authorizer decision its `$connect` route still owes, that route's
/// integration to perform, and whether the serving stage publishes per-route series for it.
#[derive(Debug, Clone)]
struct Offered {
    connection: Connection,
    authorizer: Option<super::OwedDecision>,
    connect: Option<integration::Selected>,
    per_route_connect: Option<String>,
    /// The refusal the connection request's AUTHORIZATION owes by itself — the handshake
    /// missing an identity source, and a route whose authorizer the api does not hold — which
    /// is answered by the caller rather than here, because a refusal of a resolved stage is
    /// METERED on that stage ([`admission`]).
    unauthorized: Option<super::HandshakeRefusal>,
}

/// Read everything the decision needs out of the store, or the refusal a step that resolves
/// nothing owes.
fn offered(
    state: &::engine::ServiceState<ServiceStore>,
    api_id: &str,
    stage_name: &str,
    offer: &Offer,
) -> Result<Offered, Refusal> {
    let mut store = state.lock();
    // One id per connection, minted from the transport's own counter and never reused.
    let connection_id = store.websocket_transport.mint_connection_id();
    // This socket IS the api's DEFAULT `execute-api` endpoint — its address is the one the
    // api's `ApiEndpoint` announces — so it upgrades nothing while the api turned that
    // endpoint off: the handshake is refused `403` `{"message":"Forbidden"}` HERE, under this
    // one lock and before the stage, the authorizer or the `$connect` integration is read, so
    // a client learns at once that the endpoint is off rather than waiting on a backend
    // nothing should have called ([`super::super::data_plane::serves_default_endpoint`]).
    if store
        .apis
        .get(api_id)
        .is_some_and(|api| !super::super::data_plane::serves_default_endpoint(api))
    {
        tracing::debug!(
            %api_id,
            stage = %stage_name,
            "a handshake reached an api whose default execute-api endpoint is disabled"
        );
        return Err(forbidden());
    }
    let Some((api, stage, deployed)) = integration::deployed(&store, api_id, stage_name) else {
        // The api was deleted between the handshake arriving and now (its socket is
        // closing), or the path named no stage of it, or that stage has no deployment
        // released to it: an endpoint that serves nothing.
        tracing::debug!(
            %api_id,
            stage = %stage_name,
            "a handshake named no deployed stage of this api"
        );
        return Err(forbidden());
    };
    let authorization = super::connect_authorization(
        api,
        stage,
        deployed,
        |name: &str| offer.header(name),
        &offer.target,
    );
    let (authorizer, unauthorized) = match authorization {
        Ok(authorizer) => (authorizer, None),
        Err(refusal) => (None, Some(refusal)),
    };
    Ok(Offered {
        connection: Connection {
            api_id: api_id.to_string(),
            stage_name: stage_name.to_string(),
            connection_id,
            region: api.region.clone(),
            connected_at: SystemTime::now(),
            headers: offer.headers.clone(),
            query_parameters: super::super::data_plane::joined_query_parameters(&offer.target),
            stage_variables: stage.stage_variables.clone(),
            // Filled in by the authorizer's own decision, when a route names one.
            authorizer_context: None,
            authorizer_principal: None,
        },
        authorizer,
        connect: integration::select_route_key(deployed, dispatch::CONNECT_ROUTE_KEY),
        // Whether the stage this handshake addressed publishes a per-route series for the
        // route that serves a connection request, read under this same lock.
        per_route_connect: metrics::publishes_per_route(stage, dispatch::CONNECT_ROUTE_KEY)
            .then(|| dispatch::CONNECT_ROUTE_KEY.to_string()),
        unauthorized,
    })
}

/// `403` `{"message":"Forbidden"}`, the answer an `execute-api` endpoint gives for a stage it
/// does not serve.
fn forbidden() -> Refusal {
    Refusal::gateway(
        super::super::data_plane::FORBIDDEN_STATUS,
        super::super::data_plane::FORBIDDEN_MESSAGE,
    )
}

/// The refusal a handshake a BROWSER made from ANOTHER SITE owes, or `None` for every other
/// client.
///
/// An RFC 6455 handshake is one of the few requests a page can make to a port on the
/// developer's own machine with no CORS preflight to stop it, so the browser-set fields are
/// read by the CORE'S OWN reading of them (`emulator_server::classify_browser_request`,
/// which the core applies to every request on one of its HTTP listeners) rather than by a
/// second rule written here: a request carrying no `Origin` and no `Sec-Fetch-Site` is not a
/// browser's and is served, a same-origin one is served, and a cross-site one is refused
/// `403` before any record of the api is read. The policy is the default one — same-origin
/// only, and no remote origin reaches a data plane — because nothing here is an operator
/// knob.
fn browser_refusal(offer: &Offer) -> Option<Refusal> {
    let verdict = ::emulator_server::classify_browser_request(
        offer.header(::http::header::ORIGIN.as_str()).as_deref(),
        offer.header("sec-fetch-site").as_deref(),
        &::emulator_server::RemoteOrigins::SameOriginOnly,
        false,
    );
    match verdict {
        ::emulator_server::BrowserVerdict::Allow => None,
        ::emulator_server::BrowserVerdict::Forbid(reason) => {
            tracing::info!(%reason, "refusing a WebSocket handshake a browser made cross-site");
            Some(forbidden())
        }
    }
}

/// `500` `{"message":"Internal server error"}`, the answer a gateway gives for a call its
/// api's own configuration cannot be served from.
fn internal_server_error() -> Refusal {
    Refusal::gateway(super::super::data_plane::UNPLACEABLE_STATUS, INTERNAL_MESSAGE)
}

/// The sentence a `500` carries, as API Gateway words it.
const INTERNAL_MESSAGE: &str = "Internal server error";

/// Carry one established connection: register it, dispatch every frame the client sends,
/// write every payload posted to it, invoke the `$disconnect` route when it ends, and forget
/// it.
///
/// Every message the connection carries is METERED on the stage that admitted it — each frame
/// the client sent, and each frame written back to it, a reply and an out-of-band push alike
/// (`MessageCount` is "the number of messages sent to the WebSocket API, either from or to the
/// client").
async fn carry_frames<S>(
    state: ::engine::ServiceState<ServiceStore>,
    registry: ::engine::ServiceState<connections::ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState>,
    callback: EmbeddedEmulator,
    accepted: Admitted,
    socket: ::tokio_tungstenite::WebSocketStream<S>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (pushes, mut pushed): (Sender<Vec<u8>>, Receiver<Vec<u8>>) =
        channel(connections::CONNECTION_PUSH_DEPTH);
    let connection = accepted.connection;
    let metrics = accepted.metrics;
    let api_id = connection.api_id.clone();
    // The id this connection was minted when the handshake was decided, which is the id the
    // `$connect` route's own event already reported.
    let connection_id = connection.connection_id.clone();
    // The connection EXISTS from here: it is registered in the registry the `@connections`
    // management API serves, with the ENDPOINT this api is reached at (so a `@connections`
    // call made at another api's endpoint does not address it) and the channel that makes a
    // `PostToConnection` to it a real delivery over this socket. The registry holds the only
    // sender, so a `DeleteConnection` that removes the record also ends this loop and closes
    // the socket.
    let endpoint = state.lock().websocket_transport.authority(&api_id);
    let registered = connections::open_connection(
        &registry,
        connection_id.clone(),
        connections::Identity {
            source_ip: LOOPBACK_SOURCE_IP.to_string(),
            user_agent: connection
                .user_agent()
                .unwrap_or_default()
                .to_string(),
        },
        SystemTime::now(),
        endpoint,
        Some(pushes),
    );
    if !registered {
        tracing::warn!(%api_id, %connection_id, "a minted ConnectionId was already registered");
        return;
    }
    tracing::info!(
        %api_id,
        stage = %connection.stage_name,
        %connection_id,
        "a WebSocket connection is open"
    );
    let (mut outgoing, mut incoming) = socket.split();
    // How the client ended the connection, for the `$disconnect` route's own event.
    let mut closed: Option<(Option<u16>, Option<String>)> = None;
    loop {
        ::tokio::select! {
            push = pushed.recv() => {
                let Some(frame) = push else {
                    // The registry dropped this connection's sender: a `DeleteConnection`
                    // removed the record (or the api's socket is closing), and "the
                    // connection is closed" is what that call does to the client. The
                    // close is stated to it rather than left to the socket being dropped,
                    // so a client library reports a NORMAL closure instead of an abnormal
                    // one it would otherwise retry through.
                    let _ = outgoing.send(Message::Close(Some(server_close()))).await;
                    break;
                };
                if outgoing.send(frame_of(frame)).await.is_err() {
                    break;
                }
                // A payload pushed to this connection is a message the api sent the client,
                // and it belongs to no route: it was posted out of band through the
                // `@connections` management API rather than dispatched to a route.
                metrics.message(None);
            }
            frame = incoming.next() => {
                let body = match frame {
                    Some(Ok(Message::Text(text))) => text.as_str().to_string(),
                    // A WEBSOCKET api carries TEXT messages: a client DATA frame of any
                    // other kind is a payload this plane does not serve, so it selects no
                    // route, reaches no integration, and ends the connection with the
                    // close RFC 6455 §7.4.1 states for "data within a message that it does
                    // not understand" (1003).
                    Some(Ok(Message::Binary(bytes))) => {
                        tracing::info!(
                            %api_id,
                            %connection_id,
                            payload_bytes = bytes.len(),
                            "refusing a client data frame that is not text"
                        );
                        refused(&mut outgoing, CloseCode::Unsupported, BINARY_UNSUPPORTED_REASON)
                            .await;
                        break;
                    }
                    Some(Ok(Message::Close(frame))) => {
                        closed = Some(match frame {
                            Some(frame) => (
                                Some(u16::from(frame.code)),
                                Some(frame.reason.as_str().to_string()),
                            ),
                            None => (None, None),
                        });
                        // RFC 6455 §5.5.1: on receiving a Close frame "an endpoint MUST
                        // send a Close frame in response", and the closing handshake is
                        // what a client waits for before it reports the connection ended
                        // normally — a socket that simply stops is an abnormal close it
                        // would otherwise retry through. The implementation QUEUED that
                        // reply (echoing the status the client sent) when it read the
                        // client's frame; closing the write half is what writes it, so
                        // nothing about the protocol is stated here.
                        let _ = outgoing.close().await;
                        break;
                    }
                    // Ping, Pong and Frame are the protocol's own: the implementation answers
                    // a ping itself and nothing of the api serves them.
                    Some(Ok(_)) => continue,
                    // A message the reader itself stops at carries the SAME refusal the
                    // ceiling below carries, and for the same reason the gateway states it:
                    // one frame past [`MAX_FRAME_BYTES`], or fragments past the backstop
                    // [`READ_CEILING_BYTES`]. The client reads why its message was refused
                    // instead of losing the connection with nothing said.
                    Some(Err(SocketError::Capacity(exceeded))) => {
                        tracing::info!(
                            %api_id,
                            %connection_id,
                            %exceeded,
                            "refusing a client message past the bounds this socket is read under"
                        );
                        refused(&mut outgoing, CloseCode::Size, MESSAGE_TOO_BIG_REASON).await;
                        break;
                    }
                    Some(Err(error)) => {
                        tracing::debug!(
                            %api_id,
                            %connection_id,
                            %error,
                            "a WebSocket connection failed while a frame was read"
                        );
                        break;
                    }
                    None => break,
                };
                // The per-message ceiling, which the GATEWAY enforces: a message whose
                // ASSEMBLED payload passes [`MAX_MESSAGE_BYTES`] reaches no route and ends
                // the connection with the close RFC 6455 §7.4.1 states for a message "too
                // big to process" (1009).
                if body.len() > MAX_MESSAGE_BYTES {
                    tracing::info!(
                        %api_id,
                        %connection_id,
                        payload_bytes = body.len(),
                        ceiling_bytes = MAX_MESSAGE_BYTES,
                        "refusing a client message past the per-message ceiling"
                    );
                    refused(&mut outgoing, CloseCode::Size, MESSAGE_TOO_BIG_REASON).await;
                    break;
                }
                let served =
                    served_frame(&state, &callback, &metrics, &connection, &body).await;
                if let Some(reply) = served.reply {
                    if outgoing.send(frame_of(reply)).await.is_err() {
                        break;
                    }
                    // The reply reached the client, which is one more message of the stage.
                    metrics.message(served.per_route.as_deref());
                }
                // The STAGE's record of the message, written once the client has been
                // answered: one record per route execution. It carries the call's `$context`
                // and the stage's format, never the frame's payload, and the frames that can
                // drive it are the ones this socket already reads under its own message and
                // frame ceilings ([`socket_limits`]) on a connection the core's listener cap
                // admitted.
                access_logs::delivered(&state, &callback, &connection, &served.logged).await;
            }
        }
    }
    // The CLOSE is a call of this connection too: the `$disconnect` route's own integration
    // is invoked with it, before the connection's record goes, so a handler that reads the
    // registry still sees the connection it is being told about.
    let (status_code, reason) = closed.unwrap_or((None, None));
    let logged = disconnected(
        &state,
        &callback,
        &metrics,
        &connection,
        Call::Disconnect {
            status_code,
            reason,
        },
    )
    .await;
    // The STAGE's record of the close, which is the third route execution of a session that
    // ran.
    access_logs::delivered(&state, &callback, &connection, &logged).await;
    // The connection has ended: its record goes, so its id addresses a connection that no
    // longer exists — which is `GoneException` from each of the three `@connections`
    // operations. The id stays ISSUED, so it is never refused as one that never existed.
    connections::close_connection(&registry, &connection_id);
    tracing::info!(%api_id, %connection_id, "a WebSocket connection is closed");
}

/// Invoke the `$disconnect` route's integration for a connection that has ended, if the
/// serving deployment holds such a route, and answer what the stage LOGS about the close.
///
/// "API Gateway invokes [the `$disconnect` route] when a client disconnects from your API",
/// and the close is a one-way call: nothing can be sent back over a socket that has ended, so
/// what the integration replied is logged and never answered. The call is a message of the
/// stage like the others, and the integration that served it reports how long it took.
///
/// The close is a route execution whether or not the api holds a `$disconnect` route, so it
/// is always reported for the stage's record; the status is the one that route's integration
/// produced, and none where nothing ran.
async fn disconnected(
    state: &::engine::ServiceState<ServiceStore>,
    callback: &EmbeddedEmulator,
    metrics: &StageMetrics,
    connection: &Connection,
    call: Call,
) -> access_logs::Served {
    let request_id = crate::service::invoke::proxy_event::mint_request_id();
    let started = Instant::now();
    let mut logged = access_logs::Served {
        call: call.clone(),
        route_key: dispatch::DISCONNECT_ROUTE_KEY.to_string(),
        request_id: request_id.clone(),
        status: None,
        response_length: None,
        latency: Duration::default(),
        error_message: None,
    };
    let selected = {
        let store = state.lock();
        integration::deployed(&store, &connection.api_id, &connection.stage_name).and_then(
            |(_api, stage, deployed)| {
                integration::select_route_key(deployed, dispatch::DISCONNECT_ROUTE_KEY).map(
                    |selected| {
                        let per_route = metrics::publishes_per_route(stage, &selected.route_key)
                            .then(|| selected.route_key.clone());
                        (selected, per_route)
                    },
                )
            },
        )
    };
    let Some((selected, per_route)) = selected else {
        logged.latency = access_logs::latency_since(started);
        return logged;
    };
    let route = per_route.as_deref();
    metrics.message(route);
    let began = Instant::now();
    match integration::perform(callback, connection, &selected, &call, &request_id).await {
        Ok(answered) => {
            logged.status = answered
                .status_code
                .as_deref()
                .and_then(|status| status.trim().parse::<u16>().ok());
            metrics.integration_latency(route, began.elapsed());
            metrics.served(route, CallOutcome::default());
        }
        Err(unserved) => {
            tracing::info!(
                api_id = %connection.api_id,
                connection_id = %connection.connection_id,
                ?unserved,
                "the $disconnect route's integration produced no reply"
            );
            metrics.served(
                route,
                CallOutcome {
                    execution_error: true,
                    integration_error: true,
                    ..CallOutcome::default()
                },
            );
        }
    }
    logged.latency = access_logs::latency_since(started);
    logged
}


/// The address a connection on a core-owned loopback listener comes from. The core does not
/// hand the handler the peer address, and every connection to a loopback listener comes from
/// loopback, so that is what a connection is attributed to.
const LOOPBACK_SOURCE_IP: &str = "127.0.0.1";

/// A pushed payload as a frame: always a TEXT frame, which is what API Gateway delivers a
/// `PostToConnection` payload as — bytes that are not valid UTF-8 travel with each invalid
/// sequence replaced by the Unicode replacement character, as a text frame's payload must be
/// (RFC 6455 §5.6, "text frames … interpreted as UTF-8"), rather than being relayed as a
/// binary frame the client would read as a different kind of message.
fn frame_of(frame: Vec<u8>) -> Message {
    Message::Text(String::from_utf8_lossy(&frame).into_owned().into())
}

/// The close the GATEWAY states when it ends a connection the client did not close: the
/// normal closure of RFC 6455 §7.4.1 (1000, "the purpose for which the connection was
/// established has been fulfilled"), with no reason phrase — a `DeleteConnection` is an
/// ordinary end of the connection, not a protocol or policy failure.
fn server_close() -> CloseFrame {
    CloseFrame {
        code: CloseCode::Normal,
        reason: String::new().into(),
    }
}

/// Refuse the client's MESSAGE: state the close its refusal carries and end the connection.
///
/// A message a WEBSOCKET api does not serve — one whose frame is not text, one past the
/// per-message ceiling or the frame quota — is refused with the RFC 6455 §7.4.1 status code of
/// that refusal (`1003` for data the endpoint does not accept, `1009` for a message too big to
/// process) and the sentence the gateway words it with, WRITTEN before the socket is let go: a
/// connection that merely stops tells a client nothing, so nothing keyed on the close code can
/// run. No route is selected and no integration is invoked for such a message.
async fn refused<W>(outgoing: &mut W, code: CloseCode, reason: &str)
where
    W: ::futures_util::Sink<Message> + Unpin,
{
    let _ = outgoing
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.to_string().into(),
        })))
        .await;
    // The write half is closed here rather than left to the socket being dropped, as the
    // closing handshake of a client's own close is ([`carry_frames`]).
    let _ = outgoing.close().await;
}

/// The sentences API Gateway words a refused message's close with, carried in the CLOSE
/// frame's payload after the status code (RFC 6455 §5.5.1, "the body MAY contain … a reason"):
/// one for a client data frame that is not text, one for a message past the per-message
/// ceiling.
const BINARY_UNSUPPORTED_REASON: &str = "Binary is not supported";
const MESSAGE_TOO_BIG_REASON: &str = "Message too big";

/// What one frame is answered with, and what the stage LOGS about serving it.
///
/// Three answers, all from the configuration the stage deployed and from what the serving
/// route's integration replied ([`integration::perform`]):
///
/// * a message the api routes nowhere — no route's key is selected and there is no `$default`
///   route — is answered with the gateway's `Forbidden` frame;
/// * a message whose route's integration produced no reply is answered with the gateway's
///   `Internal server error` frame;
/// * a message served by a TWO-WAY route (one that declares a `RouteResponse`) is answered
///   with the payload that integration replied — the body a function returned, or what a
///   `MOCK` integration's selected response rendered. A route that declares none is one-way:
///   its integration still runs, and nothing is sent back.
///
/// Each of the three is METERED on the serving stage: the frame itself is one message, the
/// integration that served it reports how long it took, a message the api routes nowhere is a
/// client error, and an integration that produced no reply is an integration error of a route
/// that did not execute. So is the fourth outcome, which answers the client nothing: a frame
/// whose stage no longer serves anything is a message of the stage that it could not execute.
///
/// The store is read under one lock and released before the integration is performed — and
/// before anything is published, so no store of this service is held while the metric store is
/// written: one connection's backend must not stall another connection's frame.
async fn served_frame(
    state: &::engine::ServiceState<ServiceStore>,
    callback: &EmbeddedEmulator,
    metrics: &StageMetrics,
    connection: &Connection,
    body: &str,
) -> ServedFrame {
    let api_id = connection.api_id.as_str();
    let connection_id = connection.connection_id.as_str();
    // One id for THIS message, minted by the socket that carries it: the event its route's
    // integration is handed reports it, a gateway frame names it, and so does the stage's
    // access-log record of the call.
    let request_id = crate::service::invoke::proxy_event::mint_request_id();
    let started = Instant::now();
    let call = Call::Message(body.to_string());
    let answer = |reply: Option<Vec<u8>>,
                  route_key: &str,
                  per_route: Option<String>,
                  status: Option<u16>,
                  error_message: Option<String>| ServedFrame {
        logged: access_logs::Served {
            call: call.clone(),
            route_key: route_key.to_string(),
            request_id: request_id.clone(),
            status,
            response_length: reply.as_ref().map(Vec::len),
            latency: access_logs::latency_since(started),
            error_message,
        },
        reply,
        per_route,
    };
    let serving = {
        let store = state.lock();
        match integration::deployed(&store, api_id, &connection.stage_name) {
            None => Serving::Undeployed,
            Some((api, stage, deployed)) => match integration::select_message(api, deployed, body)
            {
                None => Serving::Unrouted,
                Some(selected) => {
                    // Whether this stage publishes a per-route series for the route that
                    // serves the frame, read under the same lock the route was selected under.
                    let per_route = metrics::publishes_per_route(stage, &selected.route_key)
                        .then(|| selected.route_key.clone());
                    Serving::Route(Box::new(selected), per_route)
                }
            },
        }
    };
    let (selected, per_route) = match serving {
        Serving::Undeployed => {
            tracing::info!(
                %api_id,
                %connection_id,
                "a WebSocket message reached a stage that no longer serves anything"
            );
            // The frame reached the api, so it is a message of the stage; nothing of the stage
            // could execute it, and the client is answered nothing.
            metrics.message(None);
            metrics.served(
                None,
                CallOutcome {
                    execution_error: true,
                    ..CallOutcome::default()
                },
            );
            return answer(None, "", None, None, None);
        }
        Serving::Unrouted => {
            tracing::info!(%api_id, %connection_id, "a WebSocket message selected no route");
            // A message the api routes nowhere belongs to no route, and the frame it is
            // answered with states the client's own error.
            metrics.message(None);
            metrics.served(
                None,
                CallOutcome {
                    client_error: true,
                    ..CallOutcome::default()
                },
            );
            let frame = gateway_frame(
                super::super::data_plane::FORBIDDEN_MESSAGE,
                connection_id,
                &request_id,
            );
            return answer(
                Some(frame.into_bytes()),
                "",
                None,
                Some(super::super::data_plane::FORBIDDEN_STATUS),
                Some(super::super::data_plane::FORBIDDEN_MESSAGE.to_string()),
            );
        }
        Serving::Route(selected, per_route) => (*selected, per_route),
    };
    let route = per_route.as_deref();
    metrics.message(route);
    let began = Instant::now();
    let performed =
        integration::perform(callback, connection, &selected, &call, &request_id).await;
    match performed {
        Ok(answered) => {
            tracing::debug!(
                %api_id,
                %connection_id,
                route_key = %selected.route_key,
                two_way = selected.two_way,
                "a WebSocket message was served"
            );
            metrics.integration_latency(route, began.elapsed());
            metrics.served(route, CallOutcome::default());
            let status = answered
                .status_code
                .as_deref()
                .and_then(|status| status.trim().parse::<u16>().ok());
            let reply = selected.two_way.then_some(answered.payload).flatten();
            answer(reply, &selected.route_key, per_route.clone(), status, None)
        }
        Err(unserved) => {
            tracing::info!(
                %api_id,
                %connection_id,
                ?unserved,
                "a WebSocket message's route produced no reply"
            );
            // The integration could not answer: that is the integration's own failure, and
            // the route it fronts did not execute.
            metrics.served(
                route,
                CallOutcome {
                    execution_error: true,
                    integration_error: true,
                    ..CallOutcome::default()
                },
            );
            let frame = gateway_frame(INTERNAL_MESSAGE, connection_id, &request_id);
            answer(
                Some(frame.into_bytes()),
                &selected.route_key,
                per_route.clone(),
                Some(super::super::data_plane::UNPLACEABLE_STATUS),
                Some(INTERNAL_MESSAGE.to_string()),
            )
        }
    }
}

/// What SERVES one frame, read out of the store under one lock: the route the api selected for
/// it and the route key the per-route series are published under when the stage asks for them,
/// or the reason no route serves it.
#[derive(Debug, Clone)]
enum Serving {
    /// The route that serves the frame, and `Some(route_key)` when its stage publishes
    /// per-route series for it. BOXED, because a route and its integration are far larger
    /// than the two reasons nothing serves the frame and would otherwise size this value for
    /// every one of them.
    Route(Box<integration::Selected>, Option<String>),
    /// The api routes the frame nowhere: no route's key is selected and it holds no `$default`
    /// route.
    Unrouted,
    /// The stage this connection was admitted on serves nothing any more — its deployment or
    /// its api is gone.
    Undeployed,
}

/// What serving one frame came to: the payload the client reads back where anything was sent,
/// the route it is attributed to when the serving stage publishes its per-route series (`None`
/// for the gateway's own frame, which belongs to no route), and what the stage LOGS about the
/// call.
#[derive(Debug)]
struct ServedFrame {
    reply: Option<Vec<u8>>,
    per_route: Option<String>,
    logged: access_logs::Served,
}

/// A frame the GATEWAY sends rather than a route: its reason, the connection it is about and
/// the id of the call, which is the shape API Gateway's own `Forbidden` and
/// `Internal server error` frames have.
fn gateway_frame(message: &str, connection_id: &str, request_id: &str) -> String {
    let frame: BTreeMap<&str, String> = [
        (FRAME_MESSAGE_MEMBER, message.to_string()),
        (FRAME_CONNECTION_ID_MEMBER, connection_id.to_string()),
        (FRAME_REQUEST_ID_MEMBER, request_id.to_string()),
    ]
    .into_iter()
    .collect();
    ::serde_json::to_string(&frame).unwrap_or_else(|_| {
        ::serde_json::json!({ FRAME_MESSAGE_MEMBER: message }).to_string()
    })
}
