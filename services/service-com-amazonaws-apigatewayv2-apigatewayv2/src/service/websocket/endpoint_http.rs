// SPDX-License-Identifier: CC0-1.0

//! What a core-owned SOCKET of this service answers an ordinary HTTP request with: this
//! emulator's own front door, on that socket's port.
//!
//! Two sockets of this service are reached by plain HTTP requests, and both are served here
//! so one implementation decides how such a request is read, bounded and handed on:
//!
//! * a WEBSOCKET api's own socket, for every request on it that is NOT an opening handshake
//!   ([`super::transport`]) — handed to the router exactly as it arrived;
//! * the socket a CUSTOM DOMAIN NAME is reached at
//!   ([`crate::service::domain_name_socket`]) — RE-ADDRESSED first, at the api and stage that
//!   domain name's api mapping routes the request's base path to.
//!
//! # Why a socket serves plain HTTP at all
//!
//! The address that socket landed on is the api's WHOLE `ApiEndpoint` — "the URI of the
//! API … the stage name is typically appended to this URI to form a complete path to a
//! deployed API stage" (`Api$ApiEndpoint`) — and a WebSocket api's endpoint carries two
//! things, not one: the RFC 6455 opening handshake a client dials
//! ([`super::transport`]), and the `@connections` management API a BACKEND calls to push
//! to, report on and close a connection, which AWS publishes on that same endpoint
//! (`https://{api-id}.execute-api.{region}.amazonaws.com/{stage}/@connections/{id}`, the
//! URL `ApiGatewayManagementApi`'s own callers are configured with).
//!
//! A socket that read every connection as a handshake answered the second kind with
//! nothing at all: the WebSocket implementation finds no `Sec-WebSocket-Key` to accept,
//! writes no status line, and the connection is closed — which every SDK classifies as a
//! retryable transport failure, so the single most common `@connections` outcome ("this
//! connection is gone") became a retry storm instead of an immediate `410`. Whatever the
//! verdict, a `@connections` request is answered with a complete HTTP response.
//!
//! # What is served here
//!
//! The transport is this module's; the ANSWER is not. A request accepted here is handed to
//! the router serving the call that opened the listener
//! ([`::smithy_server_runtime::EmbeddedEmulator::http_connector`]) — by default exactly as it
//! arrived: method, target, headers and body unchanged, so the signature the client computed
//! over them still verifies and the service the router resolves is the one that models the
//! path. `@connections` is modelled by `ApiGatewayManagementApi`, which claims that path space
//! (this service deliberately does not: `super::super::data_plane::CONNECTIONS_SEGMENT`),
//! so that is the service that answers, from the connection registry this api's socket
//! registers its connections in.
//!
//! A socket whose address stands for something the request's own target does not name hands a
//! [`Readdress`] instead: that reading decides, per request, the URI and `Host` the router is
//! asked about — which is how a custom domain name's socket serves the api and stage its api
//! mapping names, and how a path no mapping of it answers to is refused here with no service
//! asked at all.
//!
//! Before the dispatch, the transport-safety decisions the core makes for its own HTTP
//! listeners are applied from the core's own predicates — the `Host` allowlist that guards
//! against DNS rebinding and the browser (`Origin` / `Sec-Fetch-Site`) check — and the
//! request is read under one deadline, one in-flight cap and one body ceiling, all the
//! core's, so a slow or oversized caller cannot hold a connection permit open.

use std::net::IpAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ::bytes::Bytes;
use ::emulator_server::{listener_caps, BrowserVerdict, ListenerConn, RemoteOrigins};
use ::http_body_util::{BodyExt as _, Full, Limited};
use ::smithy_server_runtime::EmbeddedEmulator;
use ::tokio::sync::{OwnedSemaphorePermit, Semaphore};
use aws_smithy_runtime_api::client::http::HttpConnector;
use aws_smithy_runtime_api::http::Request as RtRequest;
use aws_smithy_types::body::SdkBody;

/// Speak HTTP/1.1 on one accepted connection until the client closes it, answering each
/// request from this emulator's router — each handed on exactly as it arrived.
///
/// `conn` is the connection with its encryption already negotiated and the bytes already
/// read off it replayed ([`::service_common::in_band_tls`]), so the request this reads is
/// the one the client sent.
///
/// HTTP/1.1 only, as the emulator's own front door offers: every request here is fully
/// buffered, so nothing relies on HTTP/2 streaming, and a client that expects an interim
/// `100 Continue` for a large body gets one. The header-read deadline is the core's, so a
/// client that opens a connection and dribbles a request line cannot hold the connection
/// permit the core issued for it.
pub async fn serve(callback: EmbeddedEmulator, conn: ListenerConn) {
    serve_readdressed(callback, conn, None).await;
}

/// Where one request this endpoint accepted is handed to the router.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Addressed {
    /// Exactly as it arrived — target, `Host` and every other header unchanged.
    AsSent,
    /// At `uri`, with `host` as the request's `Host`: the socket's own address stands for
    /// something the client's target does not name, so the request is re-addressed at what it
    /// stands for before any service is asked about it.
    At { uri: String, host: String },
    /// Nowhere: this endpoint answers it itself with `status` and `message`, in the
    /// `{"message": …}` envelope every answer of this service's data planes carries
    /// ([`refusal`]), and no service is asked about it.
    Refused(u16, &'static str),
}

/// How a socket decides [`Addressed`] for one request: read from the request's own head, per
/// request, so a record written after the connection was opened still decides it.
pub(crate) type Readdress = Arc<dyn Fn(&::http::request::Parts) -> Addressed + Send + Sync>;

/// [`serve`], with each request re-addressed by `readdress` before the router is asked.
pub(crate) async fn serve_readdressed(
    callback: EmbeddedEmulator,
    conn: ListenerConn,
    readdress: Option<Readdress>,
) {
    let io = ::hyper_util::rt::TokioIo::new(conn);
    let service = ::hyper::service::service_fn(move |request: ::http::Request<_>| {
        let callback = callback.clone();
        let readdress = readdress.clone();
        async move {
            // A panic anywhere behind this address — in the dispatch machinery or in the
            // handler of whichever service claims the request — is CONTAINED here and
            // answered as the protocol-shaped internal failure, exactly as the endpoint a
            // control call arrives on answers the same request: one caller cannot take the
            // connection, the listener or the process down with it, and the next request is
            // served normally.
            let facts = ::emulator_server::RequestFacts::from_request(&request);
            Ok::<_, ::std::convert::Infallible>(
                ::emulator_server::contain_panics(facts, answer(callback, readdress, request))
                    .await,
            )
        }
    });
    if let Err(error) = ::hyper::server::conn::http1::Builder::new()
        .timer(::hyper_util::rt::TokioTimer::new())
        .header_read_timeout(request_deadline())
        .serve_connection(io, service)
        .await
    {
        // A connection the client dropped, or a request this emulator could not frame, ends
        // here and takes nothing else with it: the next connection is served normally.
        ::tracing::debug!(
            %error,
            "a request on a core-owned socket of this service ended before its response"
        );
    }
}

/// How many requests this endpoint reads at once, which is what bounds the memory the
/// bodies in flight on it hold: the core's own HTTP in-flight cap, one permit per request,
/// taken before a body is read and released when the answer is written.
///
/// One allowance for the whole process rather than one per api, so the bytes being read at
/// once do not grow with the number of apis that hold a socket.
fn in_flight() -> Arc<Semaphore> {
    static IN_FLIGHT: OnceLock<Arc<Semaphore>> = OnceLock::new();
    Arc::clone(IN_FLIGHT.get_or_init(|| Arc::new(Semaphore::new(listener_caps::MAX_HTTP_INFLIGHT))))
}

/// How long one request on this endpoint may take, from its first header byte to the
/// answer: the core's own HTTP request deadline, so the endpoint bounds a request exactly
/// as the core's HTTP listeners do.
fn request_deadline() -> Duration {
    Duration::from_secs(listener_caps::HTTP_REQUEST_DEADLINE_SECS)
}

/// How many bytes of one request this endpoint reads: the core's per-connection ceiling.
fn body_ceiling() -> usize {
    usize::try_from(listener_caps::MAX_BYTES_PER_CONN).unwrap_or(usize::MAX)
}

/// Answer one request made to a core-owned socket of this service.
async fn answer(
    callback: EmbeddedEmulator,
    readdress: Option<Readdress>,
    request: ::http::Request<::hyper::body::Incoming>,
) -> ::http::Response<Full<Bytes>> {
    let (parts, body) = request.into_parts();
    // WHERE this request goes, decided from its own head before a byte of its body is read:
    // a socket that stands for something the target does not name re-addresses it, and a
    // request that socket answers for nothing at all is refused here.
    let addressed = match readdress.as_ref().map(|readdress| readdress(&parts)) {
        Some(Addressed::Refused(status, message)) => return refusal(status, message),
        Some(addressed) => addressed,
        None => Addressed::AsSent,
    };
    if let Some(refused) = refused_before_dispatch(&parts, &addressed) {
        return refused;
    }
    // One permit per request IN FLIGHT, taken before a byte of the body is read: what the
    // bodies being read hold at once is then the in-flight cap times the body ceiling
    // below, rather than the connection cap times it. A request that finds no permit is
    // shed, which is what the core's own HTTP listener answers such a request.
    let Ok(permit) = in_flight().try_acquire_owned() else {
        return plain(SERVICE_UNAVAILABLE, SERVER_BUSY);
    };
    // The body ceiling and the deadline are the core's: a request is read only up to what
    // one connection may carry, and reading it and answering it together may not outlast
    // one request deadline.
    let limited = Limited::new(body, body_ceiling());
    let served = ::tokio::time::timeout(request_deadline(), async move {
        // Held for the whole read and dispatch, and released with it.
        let _permit: OwnedSemaphorePermit = permit;
        let Ok(collected) = limited.collect().await else {
            return plain(PAYLOAD_TOO_LARGE, BODY_TOO_LARGE);
        };
        dispatched(&callback, parts, &addressed, collected.to_bytes()).await
    })
    .await;
    served.unwrap_or_else(|_| plain(REQUEST_TIMEOUT, REQUEST_TIMED_OUT))
}

/// Hand one request to this emulator's own router and answer what it answers.
///
/// For [`Addressed::AsSent`], method, target, headers and body all travel UNCHANGED — the
/// `Host` field included — so the signature the client computed over them still verifies and
/// the service the router resolves from the SigV4 signing name and the modelled path is the
/// one the API belongs to. Nothing re-addresses such a request: a `@connections` call is
/// already addressed at the path its own service models.
///
/// For [`Addressed::At`], the target and the `Host` are the ones the socket resolved and every
/// other header is the client's own; the body is re-framed by this leg, so the framing headers
/// of the request it arrived in do not travel.
async fn dispatched(
    callback: &EmbeddedEmulator,
    parts: ::http::request::Parts,
    addressed: &Addressed,
    body: Bytes,
) -> ::http::Response<Full<Bytes>> {
    let Some(forwarded) = forwarded(parts, addressed, body) else {
        return plain(BAD_REQUEST, MALFORMED_REQUEST);
    };
    let answered = match callback.http_connector().call(forwarded).await {
        Ok(answered) => answered,
        Err(error) => {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a request on a core-owned socket of this service reached no service of this \
                 emulator"
            );
            return plain(UNPLACEABLE, NOT_ANSWERED);
        }
    };
    let mut builder = ::http::Response::builder().status(answered.status().as_u16());
    for (name, value) in answered.headers().iter() {
        builder = builder.header(name, value);
    }
    let body = Bytes::from(
        answered
            .into_body()
            .bytes()
            .map(<[u8]>::to_vec)
            .unwrap_or_default(),
    );
    builder
        .body(Full::new(body.clone()))
        .unwrap_or_else(|_| ::http::Response::new(Full::new(body)))
}

/// The request the router is handed, or `None` for one that does not render as a request the
/// router can be handed at all.
fn forwarded(
    parts: ::http::request::Parts,
    addressed: &Addressed,
    body: Bytes,
) -> Option<RtRequest> {
    let (uri, host) = match addressed {
        // Refused before this point ([`answer`]); nothing is forwarded for it.
        Addressed::Refused(_, _) => return None,
        Addressed::AsSent => {
            let forwarded = ::http::Request::from_parts(parts, SdkBody::from(body));
            return RtRequest::try_from(forwarded).ok();
        }
        Addressed::At { uri, host } => (uri, host),
    };
    let mut builder = ::http::Request::builder()
        .method(parts.method.clone())
        .uri(uri)
        .header(::http::header::HOST, host);
    for (name, value) in parts.headers.iter() {
        if HEADERS_NOT_FORWARDED.contains(&name.as_str()) {
            continue;
        }
        builder = builder.header(name, value);
    }
    RtRequest::try_from(builder.body(SdkBody::from(body)).ok()?).ok()
}

/// The headers a RE-ADDRESSED request does not copy from the client's own.
///
/// `Host` is the socket's own address, which the re-addressed request replaces with the one
/// the socket resolved, and the two framing headers describe a body this leg re-frames itself.
const HEADERS_NOT_FORWARDED: [&str; 3] = ["host", "content-length", "transfer-encoding"];

/// The transport-safety refusals the core applies to its own HTTP listeners, applied here
/// from the core's own predicates so this endpoint and a core listener decide them alike:
/// the `Host` allowlist (the DNS-rebinding guard a browser needs) and the browser check.
/// `None` lets the request through to the router.
///
/// The `Host` ALLOWLIST is applied only where the socket's address is the one a client
/// addresses it by — a WebSocket api's own endpoint ([`Addressed::AsSent`]), where a `Host`
/// that is not this emulator's address names nothing this endpoint serves. A socket whose
/// address STANDS FOR a name of the customer's own ([`Addressed::At`], a custom domain name) is
/// dialled under that name: its `Host` is the domain a caller pointed at the address, which no
/// allowlist of this emulator's addresses can hold, and what decided the request is the
/// RECORD that socket serves rather than the host field. The browser check still applies to
/// both.
///
/// The WebSocket refusal those listeners carry has no place here: an upgrade request on
/// this socket is the handshake it serves ([`super::transport`]), and it never reaches this
/// module.
fn refused_before_dispatch(
    parts: &::http::request::Parts,
    addressed: &Addressed,
) -> Option<::http::Response<Full<Bytes>>> {
    let header = |name: &str| parts.headers.get(name).and_then(|value| value.to_str().ok());

    // HTTP/1.1 requires a `Host`, and without an authority the rebinding guard cannot be
    // run at all, so a request carrying none is refused rather than served: this fails
    // CLOSED, exactly as the core's own HTTP listener does.
    let Some(host) = header("host").or_else(|| parts.uri.host()) else {
        return Some(plain(MISDIRECTED_REQUEST, HOST_REQUIRED));
    };
    if matches!(addressed, Addressed::AsSent)
        && !host_allowed(::service_common::data_plane_addressing::host_only(host))
    {
        return Some(plain(MISDIRECTED_REQUEST, HOST_NOT_ALLOWED));
    }
    if let BrowserVerdict::Forbid(reason) = ::emulator_server::classify_browser_request(
        header("origin"),
        header("sec-fetch-site"),
        // This endpoint's address is this emulator's, reached from the machine it runs on:
        // the same default a core HTTP listener carries, which admits no remote origin.
        &RemoteOrigins::SameOriginOnly,
        false,
    ) {
        return Some(plain(FORBIDDEN, reason));
    }
    None
}

/// Whether a request's `Host` names the interface this socket was bound on, by the rule a
/// core HTTP listener applies: a loopback `Host` on the default loopback bind (which is
/// what guards a browser against DNS rebinding), the operator's own address when they bound
/// a routable one, and any `Host` on a wildcard bind — there the operator exposed the port
/// deliberately and no single address names it.
fn host_allowed(host: &str) -> bool {
    if ::emulator_server::is_loopback_host(host) {
        return true;
    }
    let Ok(bound) = ::emulator_server::bind_address_from_env() else {
        return false;
    };
    bound.is_unspecified()
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|named| named == bound)
}

/// The statuses this module answers itself, for a request that never reached the router.
const BAD_REQUEST: u16 = 400;
const FORBIDDEN: u16 = 403;
const REQUEST_TIMEOUT: u16 = 408;
const PAYLOAD_TOO_LARGE: u16 = 413;
const MISDIRECTED_REQUEST: u16 = 421;
const UNPLACEABLE: u16 = 500;
const SERVICE_UNAVAILABLE: u16 = 503;

/// What this endpoint answers a request whose `Host` is not the address it serves.
const HOST_NOT_ALLOWED: &str = "This endpoint does not serve that host.\n";

/// What it answers a request that names no host at all, which no HTTP/1.1 request may.
const HOST_REQUIRED: &str = "This endpoint requires the host it is addressed by.\n";

/// What it answers a request carrying more than one connection may.
const BODY_TOO_LARGE: &str = "This request is larger than this endpoint accepts.\n";

/// What it answers a request that did not finish within one request deadline.
const REQUEST_TIMED_OUT: &str = "This request took longer than this endpoint allows.\n";

/// What it answers a request that arrived while it was already reading as many as it reads
/// at once.
const SERVER_BUSY: &str = "This endpoint is serving as many requests as it can. Try again.\n";

/// What it answers bytes that are no HTTP request the router can be handed at all.
const MALFORMED_REQUEST: &str = "This request is not one this endpoint can serve.\n";

/// What it answers when the emulator's own front door did not answer the forwarded
/// request — an in-process fault rather than anything the caller sent.
const NOT_ANSWERED: &str = "This endpoint could not be served. Try again later.\n";

/// One answer the SOCKET decided for a request it re-addresses nowhere: the status it refused
/// the request with, in the `{"message": …}` envelope every answer of this service's data
/// planes carries — the socket's address belongs to a resource of this service, so a refusal
/// of its own is worded as that plane words one rather than as this module's plain text.
fn refusal(status: u16, message: &str) -> ::http::Response<Full<Bytes>> {
    let body = Bytes::from(
        ::serde_json::json!({ super::super::data_plane::MESSAGE_MEMBER: message }).to_string(),
    );
    ::http::Response::builder()
        .status(status)
        .header("content-type", super::super::data_plane::JSON_CONTENT_TYPE)
        .header("content-length", body.len().to_string())
        .body(Full::new(body.clone()))
        // Every argument is this function's own, so the builder cannot fail; answer a bare
        // status rather than panicking if it ever does.
        .unwrap_or_else(|_| {
            let mut response = ::http::Response::new(Full::new(body));
            *response.status_mut() = ::http::StatusCode::INTERNAL_SERVER_ERROR;
            response
        })
}

/// One answer of this module's own, for a request that never reached the router. Plain text
/// rather than a service envelope: which protocol an answer belongs in is the claiming
/// service's to decide, and no service was asked about this one.
fn plain(status: u16, message: &str) -> ::http::Response<Full<Bytes>> {
    let body = Bytes::from(message.to_string());
    ::http::Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .header("content-length", body.len().to_string())
        .body(Full::new(body.clone()))
        // Every argument is this function's own, so the builder cannot fail; answer a bare
        // status rather than panicking if it ever does.
        .unwrap_or_else(|_| {
            let mut response = ::http::Response::new(Full::new(body));
            *response.status_mut() = ::http::StatusCode::INTERNAL_SERVER_ERROR;
            response
        })
}
