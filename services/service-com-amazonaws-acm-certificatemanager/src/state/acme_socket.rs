// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS an ACME client dials an endpoint's ACME surface at, and the socket that
//! answers there.
//!
//! `AcmeEndpoint$EndpointUrl` is not a label: it is the one handle an ACME client
//! (certbot, lego, acme.sh, the python `acme` library) is configured with, and what such a
//! client takes out of the answers at it are HTTP FIELDS — the anti-replay nonce RFC 8555
//! §7.2 spells `Replay-Nonce`, the account URL §7.3 answers in `Location`, the resource
//! map §7.1 points at in `Link`. A client reads a field under the name it was served, and
//! the first of them decides whether an exchange starts at all: without a nonce no JWS can
//! be signed, so no registration, order or revocation is reachable.
//!
//! WHO WRITES THE NAME is the transport, not the handler. A response's field names are held
//! normalised to lower case by the time any handler of this emulator has stated them, so
//! what reaches a client is whatever the HTTP/1.1 encoder serving the connection writes —
//! and the encoder behind the emulator's own front door writes those normalised bytes. A
//! surface whose client reads field names as received therefore cannot be published there.
//!
//! So an endpoint's ACME surface is published on a socket of its OWN, which the CORE owns
//! ([`::emulator_server::Listeners::tcp_plaintext`] — the same bind interface
//! (`EMULATOR_BIND_ADDRESS`, loopback unless the operator says otherwise), port range,
//! connection caps, idle timeout and teardown as every other native data plane this
//! emulator publishes), served by an HTTP/1.1 server THIS plane configures: the same
//! `hyper` server its sibling planes are served by, with the encoder's title-case option
//! set, so every field name is written in the canonical spelling its specification states
//! (`Replay-Nonce`, `Cache-Control`, `Location`, `Link`, `Content-Type`). The published
//! `EndpointUrl` of every endpoint names that address, so a client that is handed it reads
//! the fields RFC 8555 names under the names RFC 8555 gives them.
//!
//! The encryption is negotiated IN BAND ([`::service_common::in_band_tls`]), so the one
//! address serves both the `https://` an ACME client dials and a request made in the clear,
//! and this service never touches a key or a certificate.
//!
//! WHAT ANSWERS is not written here. Every request this socket accepts is handed to the
//! router that is serving the call which opened it, over the in-process transport this
//! emulator reaches itself through, so the same [`crate::service::acme_http`] surface
//! answers whether a request arrived here or at the front door — and a request that
//! presents a credential is verified by the emulator's own authenticator, as it is
//! everywhere else. Only requests addressing this service's ACME plane are forwarded; the
//! socket is that plane's address and nothing else's.
//!
//! The socket's life is the store's: the handle is held on the service state, so the
//! address stays open for as long as an endpoint can be read, and dropping the store closes
//! it and severs every connection in flight. ONE socket serves every endpoint, because the
//! endpoint is named by the PATH; it is opened by the call that creates an endpoint — there
//! is no ACME client before then — and read by every call that reports endpoint metadata.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ::aws_smithy_runtime_api::client::http::HttpConnector;
use ::aws_smithy_runtime_api::http::Request as RtRequest;
use ::aws_smithy_types::body::SdkBody;
use ::bytes::Bytes;
use ::emulator_server::{
    listener_caps, BrowserVerdict, ListenerConn, RemoteOrigins, ResourceListener, TlsUpgrader,
};
use ::http_body_util::{BodyExt as _, Full, Limited};
use ::smithy_server_runtime::EmbeddedEmulator;
use ::tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// The scheme the published ACME address is written under: RFC 8555 §6.1 requires an ACME
/// server's resources to be `https://`, and the socket terminates TLS through the core for
/// a client that dials it ([`::service_common::in_band_tls`]).
pub const ACME_SOCKET_SCHEME: &str = "https";

/// The address this service's ACME surface is served at, and the handle that keeps it open.
pub struct AcmeSocket {
    /// The `host:port` a client dials, which is the authority of every ACME URL this
    /// service publishes to a caller on this machine.
    authority: String,
    /// The socket itself. Never read: it is held because dropping it closes the port.
    _listener: ResourceListener,
}

/// Written by hand rather than derived: a core `ResourceListener` is not `Debug`. The
/// socket is reported by the address it answers on, which is what a reader needs.
impl std::fmt::Debug for AcmeSocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcmeSocket")
            .field("authority", &self.authority)
            .finish()
    }
}

impl AcmeSocket {
    /// The `host:port` a client reaches this service's ACME surface at.
    pub fn authority(&self) -> &str {
        &self.authority
    }
}

/// The `host:port` this service's ACME surface is served at, opening the socket on first
/// use, or `None` when this engine vends no listener at all — in which case the caller
/// publishes the endpoint the request arrived at instead, as it did before this address
/// existed.
///
/// Opening the address changes no endpoint and is idempotent: the first socket is kept, so
/// the surface answers at the same address for every endpoint and for as long as the store
/// lives, and a client may keep a published URL. The store guard is taken and released
/// around the bind rather than held across it, because a lock held across an await would
/// make this future non-`Send`.
pub async fn ensure_acme_socket<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<crate::state::ComAmazonawsAcmCertificatemanagerState>,
    callback: &EmbeddedEmulator,
) -> Option<String> {
    let existing = {
        let store = state.lock();
        store
            .acme_socket
            .as_ref()
            .map(|socket| socket.authority.clone())
    };
    if existing.is_some() {
        return existing;
    }
    let shared = (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()?;
    let listeners = shared.listeners();
    let handler = AcmeDataPlane {
        callback: callback.clone(),
        // The core's own TLS server, lent to this plaintext socket so the `https://` half
        // of the address is terminated by the core rather than here. An emulator that
        // could prepare none serves the request made in the clear alone.
        upgrader: listeners.tls_upgrader().ok(),
    };
    // Opened on a task of its OWN, so a runtime that cannot bind a socket at all — one
    // built with no IO driver, which is what a host driving this emulator in process may
    // hand a request — leaves this call without an address of its own instead of taking
    // the request down with it.
    let opened = ::tokio::spawn(async move { listeners.tcp_plaintext(handler).await.ok() }).await;
    let listener = opened.ok().flatten()?;
    let authority = ::service_common::data_plane_addressing::listener_authority(&listener);

    let mut store = state.lock();
    // Another request may have opened one while this one was binding; keep the first, so a
    // published address never changes under a client. Dropping the loser closes its port.
    if let Some(socket) = store.acme_socket.as_ref() {
        return Some(socket.authority.clone());
    }
    store.acme_socket = Some(AcmeSocket {
        authority: authority.clone(),
        _listener: listener,
    });
    Some(authority)
}

/// Serves the published ACME address: it hands every request addressing this service's
/// ACME plane to the router that answered the call which opened the socket, and answers
/// with what that router answered.
struct AcmeDataPlane {
    /// The emulator the call that opened this socket was served by — the same router, the
    /// same engine, the same state. It is an IN-PROCESS handle: a request this socket
    /// accepts is dispatched inside this process and nothing here dials out, so the only
    /// address involved is the one the core bound.
    callback: EmbeddedEmulator,
    upgrader: Option<TlsUpgrader>,
}

impl ::emulator_server::NativeHandler for AcmeDataPlane {
    /// One call per accepted connection. The handles are cloned into the connection's own
    /// future, which is what the core's `'static` handler contract asks for.
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let callback = self.callback.clone();
        let upgrader = self.upgrader.clone();
        Box::pin(serve_connection(callback, upgrader, conn))
    }
}

/// Speak HTTP/1.1 on one accepted connection until the client closes it, answering each
/// request from this emulator's router.
///
/// The encoder's TITLE-CASE option is what this whole address exists for: with it, a field
/// name is written on the wire in its canonical, hyphen-separated spelling, which is the
/// form RFC 8555 states every field of this surface in and the form an ACME client looks
/// for. Without it the same encoder writes the normalised lower-case name, which is what
/// the emulator's own front door serves and what leaves a client with no nonce.
///
/// HTTP/1.1 only, as the emulator's own front door offers and as an ACME client speaks:
/// every request is fully buffered, so nothing relies on HTTP/2 streaming. The header-read
/// deadline and the body ceiling are the core's, so a client that dribbles a request line,
/// or sends more than one connection may carry, cannot hold the connection permit the core
/// issued for it.
async fn serve_connection(
    callback: EmbeddedEmulator,
    upgrader: Option<TlsUpgrader>,
    conn: ListenerConn,
) {
    let Some(conn) = ::service_common::in_band_tls::negotiated(conn, upgrader).await else {
        return;
    };
    let io = ::hyper_util::rt::TokioIo::new(conn);
    let service = ::hyper::service::service_fn(move |request: ::http::Request<_>| {
        let callback = callback.clone();
        async move {
            // A panic anywhere behind this address — in the dispatch machinery or in the
            // ACME surface the forwarded request reaches — is CONTAINED here and answered
            // as the protocol-shaped internal failure, exactly as the endpoint a control
            // call arrives on answers the same request: one client cannot take the
            // connection, the listener or the process down with it.
            let facts = ::emulator_server::RequestFacts::from_request(&request);
            Ok::<_, ::std::convert::Infallible>(
                ::emulator_server::contain_panics(facts, answer(callback, request)).await,
            )
        }
    });
    if let Err(error) = ::hyper::server::conn::http1::Builder::new()
        .timer(::hyper_util::rt::TokioTimer::new())
        .header_read_timeout(request_deadline())
        .title_case_headers(true)
        .serve_connection(io, service)
        .await
    {
        // A connection the client dropped, or a request this emulator could not frame, ends
        // here and takes nothing else with it: the next connection is served normally.
        ::tracing::debug!(%error, "a connection to the published ACME address ended before its response");
    }
}

/// How many request BODIES this address reads at once: the core's own HTTP in-flight cap,
/// one permit per request, taken before a byte of a body is read and released when the
/// answer is written.
///
/// What one in-flight request may buffer is capped by [`body_ceiling`], so what the bodies
/// in flight hold at once is this count times that ceiling — the product the core's own HTTP
/// listeners bound — rather than the connection count times it. ONE semaphore for the whole
/// service, because a client chooses how many connections exist. A request that finds no
/// permit is shed, which is what the core's own HTTP listener answers such a request.
fn in_flight() -> Arc<Semaphore> {
    static IN_FLIGHT: OnceLock<Arc<Semaphore>> = OnceLock::new();
    Arc::clone(IN_FLIGHT.get_or_init(|| Arc::new(Semaphore::new(listener_caps::MAX_HTTP_INFLIGHT))))
}

/// How long one request on this address may take, from its first field byte to the answer:
/// the core's own HTTP request deadline, so this address bounds a request exactly as the
/// core's HTTP listeners do.
fn request_deadline() -> Duration {
    Duration::from_secs(listener_caps::HTTP_REQUEST_DEADLINE_SECS)
}

/// How many bytes of one request this address reads: the core's per-connection ceiling.
fn body_ceiling() -> usize {
    usize::try_from(listener_caps::MAX_BYTES_PER_CONN).unwrap_or(usize::MAX)
}

/// Answer one request to the published ACME address from this emulator's router.
async fn answer(
    callback: EmbeddedEmulator,
    request: ::http::Request<::hyper::body::Incoming>,
) -> ::http::Response<Full<Bytes>> {
    let (parts, body) = request.into_parts();
    if let Some(refused) = refused_before_dispatch(&parts) {
        return refused;
    }
    // One permit per request IN FLIGHT, taken BEFORE a byte of the body is read: what the
    // bodies being read hold at once is then the in-flight cap times the ceiling below,
    // rather than the connection cap times it.
    let Ok(permit) = in_flight().try_acquire_owned() else {
        return plain(503, SERVER_BUSY);
    };
    let limited = Limited::new(body, body_ceiling());
    let served = ::tokio::time::timeout(request_deadline(), async move {
        // Held for the whole read and dispatch, and released with it.
        let _permit: OwnedSemaphorePermit = permit;
        let Ok(collected) = limited.collect().await else {
            return plain(413, BODY_TOO_LARGE);
        };
        let Some(forwarded) = forwarded(parts, collected.to_bytes()) else {
            return plain(400, NOT_A_REQUEST);
        };
        match callback.http_connector().call(forwarded).await {
            Ok(response) => {
                let status = response.status().as_u16();
                let mut answer = ::http::Response::builder().status(status);
                for (name, value) in response.headers().iter() {
                    answer = answer.header(name, value);
                }
                let body = Bytes::from(
                    response
                        .into_body()
                        .bytes()
                        .map(<[u8]>::to_vec)
                        .unwrap_or_default(),
                );
                answer
                    .body(Full::new(body.clone()))
                    .unwrap_or_else(|_| ::http::Response::new(Full::new(body)))
            }
            // The router is this same process, so this is no answer about an ACME endpoint;
            // it is reported as an answer this address could not produce.
            Err(_) => plain(502, NOT_SERVED),
        }
    })
    .await;
    served.unwrap_or_else(|_| plain(504, REQUEST_TIMED_OUT))
}

/// The request the router is handed: the same method, path, query, body and fields — the
/// `Host` among them, so the URLs the answer carries are minted on the address the client
/// is talking to and this service claims the request by path as it does at the front door.
///
/// `None` for a request that does not render as one at all, which addresses nothing.
fn forwarded(parts: ::http::request::Parts, body: Bytes) -> Option<RtRequest> {
    let host = parts
        .headers
        .get(::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| parts.uri.host())?;
    let query = parts
        .uri
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let uri = format!(
        "{ACME_SOCKET_SCHEME}://{host}{}{query}",
        parts.uri.path()
    );
    let mut builder = ::http::Request::builder()
        .method(parts.method.clone())
        .uri(uri);
    for (name, value) in parts.headers.iter() {
        // The two framing fields describe a body this leg re-frames itself.
        if name == ::http::header::CONTENT_LENGTH || name == ::http::header::TRANSFER_ENCODING {
            continue;
        }
        builder = builder.header(name, value);
    }
    let built = builder.body(SdkBody::from(body)).ok()?;
    RtRequest::try_from(built).ok()
}

/// The refusals this address decides itself, before any request reaches the router: the
/// transport-safety checks the core applies to its OWN HTTP listeners, taken from the
/// core's own predicates so this address and a core listener decide them alike, and the
/// plane this socket is the address OF. `None` lets the request through to the router.
fn refused_before_dispatch(
    parts: &::http::request::Parts,
) -> Option<::http::Response<Full<Bytes>>> {
    let sent = |name: &str| parts.headers.get(name).and_then(|value| value.to_str().ok());

    // HTTP/1.1 requires a `Host`, and without an authority the rebinding guard cannot be
    // run at all, so a request carrying none is refused rather than served: this fails
    // CLOSED, exactly as the core's own HTTP listener does.
    let Some(host) = sent("host").or_else(|| parts.uri.host()) else {
        return Some(plain(421, HOST_REQUIRED));
    };
    if !::service_common::data_plane_addressing::host_names_the_bound_interface(host) {
        return Some(plain(421, HOST_NOT_ALLOWED));
    }
    if ::emulator_server::is_websocket_upgrade(sent("connection"), sent("upgrade"), sent(":protocol"))
    {
        return Some(plain(501, NO_WEBSOCKETS));
    }
    if let BrowserVerdict::Forbid(reason) = ::emulator_server::classify_browser_request(
        sent("origin"),
        sent("sec-fetch-site"),
        // This address is the emulator's own, reached from the machine it runs on: the same
        // default a core HTTP listener carries, which admits no remote origin.
        &RemoteOrigins::SameOriginOnly,
        false,
    ) {
        return Some(plain(403, reason));
    }
    // A target that resolves out of the prefix naming the plane names another path
    // entirely, and that prefix is the only thing that says whose surface must answer.
    let path = parts.uri.path();
    if ::service_common::data_plane_addressing::carries_a_dot_segment(path) {
        return Some(plain(400, NOT_A_REQUEST));
    }
    // This socket is the address of ONE plane. A request addressing anything else is not
    // forwarded at all: the control plane is reached at the emulator's own endpoint.
    if !crate::state::addresses_acme_local_plane(path) {
        return Some(plain(404, NO_SUCH_RESOURCE));
    }
    None
}

/// One answer of this address's own, for a request the router never saw. Plain text rather
/// than an ACME problem document: which protocol an answer belongs in is the claiming
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

/// What this address answers a request whose `Host` is not the address it serves.
const HOST_NOT_ALLOWED: &str = "This address does not serve that host.\n";

/// What it answers a request that names no host at all, which no HTTP/1.1 request may.
const HOST_REQUIRED: &str = "This address requires the host it is addressed by.\n";

/// What it answers a WebSocket upgrade, which this address does not serve.
const NO_WEBSOCKETS: &str = "This address does not serve WebSocket connections.\n";

/// What it answers a request carrying more than one connection may.
const BODY_TOO_LARGE: &str = "This request is larger than this address accepts.\n";

/// What it answers a request that did not finish within one request deadline.
const REQUEST_TIMED_OUT: &str = "This request took longer than this address allows.\n";

/// What it answers a request that arrived while this address was already reading as many
/// bodies as it reads at once.
const SERVER_BUSY: &str = "This address is serving as many requests as it can. Try again.\n";

/// What it answers a request it could not read as one at all.
const NOT_A_REQUEST: &str = "This request could not be read.\n";

/// What it answers a request that addresses something other than the ACME surface this
/// address is published for.
const NO_SUCH_RESOURCE: &str = "This address serves the ACME surface of an ACME endpoint.\n";

/// What it answers a request the router did not answer.
const NOT_SERVED: &str = "This request was not served.\n";
