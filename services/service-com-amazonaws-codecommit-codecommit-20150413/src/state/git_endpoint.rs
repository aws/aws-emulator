// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS a git client dials this repository's git transport at, and the
//! listener that answers there.
//!
//! `RepositoryMetadata.cloneUrlHttp` is "the URL to use for cloning the repository
//! over HTTPS", and it is not a label: it is an address a STOCK git client is handed
//! and DIALS, with the configuration it already has and nothing in between to
//! rewrite it. So the announced value has to be an origin that client reaches as it
//! stands.
//!
//! The emulator's own front door is served over TLS with a certificate generated at
//! run time, which no default trust store accepts: a git client handed an `https://`
//! URL on it stops at the handshake, before one byte of the git protocol is
//! exchanged, and the whole transport — and everything only reachable through it —
//! is unusable. The core's certificate is the core's, so this service does not get
//! to present another; what the core does offer is a listener that hands the handler
//! the connection's first byte ([`::emulator_server::Listeners::tcp_plaintext`], the
//! same socket, bind interface, port range, connection caps, idle timeout and
//! teardown as its TLS sibling). The git transport is announced on one of those, as
//! `http://host:port/v1/repos/<repositoryName>` — the same path form CodeCommit's
//! own clone URL carries, on an origin a git client dials with no extra trust
//! material.
//!
//! What a client sends to that port never leaves this machine: the interface is the
//! CORE's (`EMULATOR_BIND_ADDRESS`, loopback unless the operator says otherwise, and
//! the core owns the bind), and every request is dispatched inside this process.
//!
//! The encryption is still negotiated IN BAND
//! ([`::service_common::in_band_tls`]), so a client that dials the same address with
//! `https://` is handed to the core's own TLS server and served too: one address,
//! both schemes, and this service never touches a key or a certificate.
//!
//! The listener is the ADDRESS, not the transport: everything a git request is
//! answered BY stays in [`crate::service::git_http`], which serves the same request
//! however it was addressed. The handler hands every request it accepts to the
//! router that is serving the call which opened the listener, over the in-process
//! transport this emulator reaches itself through, so the address answers whatever
//! this emulator answers — including every control-plane call, since the request is
//! dispatched by the same router. Nothing is synthesised here.
//!
//! The listener's life is the store's: the handle is held on the service state, so
//! the port stays open for as long as a repository can be read, and dropping the
//! store closes it and severs every connection in flight on it. One listener serves
//! every repository, because the repository is named by the PATH; the address is
//! opened by the call that creates a repository — there is nothing to clone before
//! then — and read by every call that reports repository metadata.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ::aws_smithy_runtime_api::client::http::HttpConnector;
use ::aws_smithy_runtime_api::http::Request as RtRequest;
use ::aws_smithy_types::body::SdkBody;
use ::bytes::Bytes;
use ::emulator_server::{listener_caps, BrowserVerdict, ListenerConn, RemoteOrigins, TlsUpgrader};
use ::http_body_util::{BodyExt as _, Full, Limited};
use ::smithy_server_runtime::EmbeddedEmulator;
use ::tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// The scheme the announced git address is published under: the one the transport
/// its listener speaks in the clear names, so the value is an origin the git client
/// that just read the metadata dials as it stands.
pub const GIT_SCHEME: &str = "http";

/// How many git request BODIES this address reads at once: the core's own HTTP
/// in-flight cap, one permit per request, taken before a byte of a body is read and
/// released when the answer is written.
///
/// What one in-flight request may buffer is capped by [`body_ceiling`], so what the
/// bodies in flight hold at once is this count times that ceiling — the product the
/// core's own HTTP listeners bound — rather than the connection count times it. ONE
/// semaphore for the whole service, because a caller chooses how many connections
/// exist. A request that finds no permit is shed, which is what the core's own HTTP
/// listener answers such a request.
fn in_flight() -> Arc<Semaphore> {
    static IN_FLIGHT: OnceLock<Arc<Semaphore>> = OnceLock::new();
    Arc::clone(IN_FLIGHT.get_or_init(|| Arc::new(Semaphore::new(listener_caps::MAX_HTTP_INFLIGHT))))
}

/// The address this service's git transport is served at, and the handle that keeps
/// it open.
pub struct GitEndpoint {
    /// The `host:port` a git client dials, which is the authority of every announced
    /// clone URL.
    authority: String,
    /// The listener itself. Never read: it is held because dropping it closes the
    /// port.
    _listener: ::emulator_server::ResourceListener,
}

/// Written by hand rather than derived: a core `ResourceListener` is not `Debug`. The
/// listener is reported by the address it answers on, which is what a reader needs.
impl std::fmt::Debug for GitEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitEndpoint")
            .field("authority", &self.authority)
            .finish()
    }
}

impl GitEndpoint {
    /// The `host:port` a git client reaches this service's git transport at.
    pub fn authority(&self) -> &str {
        &self.authority
    }
}

/// The `host:port` this service's git transport is served at, opening the listener on
/// first use, or `None` when this engine vends no listener at all — in which case the
/// caller announces the endpoint the request arrived at instead.
///
/// Opening the address changes no repository and is idempotent: the first listener is
/// kept, so the git transport answers at the same address for every repository and for
/// as long as the store lives, and a client may key a remote on it. The store guard is
/// taken and released around the bind rather than held across it, because a lock held
/// across an await would make this future non-`Send`.
pub async fn ensure_git_endpoint<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<crate::state::ComAmazonawsCodecommitCodecommit20150413State>,
    callback: &EmbeddedEmulator,
) -> Option<String> {
    let existing = {
        let store = state.lock();
        store
            .git_endpoint
            .as_ref()
            .map(|endpoint| endpoint.authority.clone())
    };
    if existing.is_some() {
        return existing;
    }
    let shared = (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()?;
    let listeners = shared.listeners();
    let handler = GitFrontDoor {
        callback: callback.clone(),
        // The core's own TLS server, lent to this plaintext listener so the
        // `https://` half of the address is terminated by the core rather than here.
        // A listener this emulator could prepare none for serves the `http://` half
        // alone.
        upgrader: listeners.tls_upgrader().ok(),
    };
    // Opened on a task of its OWN, so a runtime that cannot bind a socket at all —
    // one built with no IO driver, which is what a host driving this emulator in
    // process may hand a request — leaves this call without an address of its own
    // instead of taking the request down with it.
    let opened = ::tokio::spawn(async move { listeners.tcp_plaintext(handler).await.ok() }).await;
    let listener = opened.ok().flatten()?;
    let authority = ::service_common::data_plane_addressing::listener_authority(&listener);

    let mut store = state.lock();
    // Another request may have opened one while this one was binding; keep the first,
    // so an announced address never changes under a client. Dropping the loser closes
    // its port.
    if let Some(endpoint) = store.git_endpoint.as_ref() {
        return Some(endpoint.authority.clone());
    }
    store.git_endpoint = Some(GitEndpoint {
        authority: authority.clone(),
        _listener: listener,
    });
    Some(authority)
}

/// Serves the announced git address: it hands every request it accepts to the router
/// that answered the call which opened the listener, and answers with what that
/// router answered.
struct GitFrontDoor {
    /// The emulator the call that opened this listener was served by — the same
    /// router, the same engine, the same state. It is an IN-PROCESS handle: a request
    /// this listener accepts is dispatched inside this process and nothing here dials
    /// out, so the only address involved is the one the core bound.
    callback: EmbeddedEmulator,
    upgrader: Option<TlsUpgrader>,
}

impl ::emulator_server::NativeHandler for GitFrontDoor {
    /// One call per accepted connection. The handles are cloned into the connection's
    /// own future, which is what the core's `'static` handler contract asks for.
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let callback = self.callback.clone();
        let upgrader = self.upgrader.clone();
        Box::pin(serve_connection(callback, upgrader, conn))
    }
}

/// Speak HTTP/1.1 on one accepted connection until the client closes it, answering
/// each request from this emulator's router.
///
/// HTTP/1.1 only, as the emulator's own front door offers and as a git client speaks:
/// every request is fully buffered, so nothing relies on HTTP/2 streaming. The
/// header-read deadline and the body ceiling are the core's, so a client that dribbles
/// a request line, or sends more than one connection may carry, cannot hold the
/// connection permit the core issued for it.
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
            // A panic anywhere behind this address — in the dispatch machinery or in
            // the handler the forwarded request reaches — is CONTAINED here and
            // answered as the protocol-shaped internal failure, exactly as the
            // endpoint a control call arrives on answers the same request: one client
            // cannot take the connection, the listener or the process down with it.
            let facts = ::emulator_server::RequestFacts::from_request(&request);
            Ok::<_, ::std::convert::Infallible>(
                ::emulator_server::contain_panics(facts, answer(callback, request)).await,
            )
        }
    });
    if let Err(error) = ::hyper::server::conn::http1::Builder::new()
        .timer(::hyper_util::rt::TokioTimer::new())
        .header_read_timeout(request_deadline())
        .serve_connection(io, service)
        .await
    {
        // A connection the client dropped, or a request this emulator could not
        // frame, ends here and takes nothing else with it: the next connection is
        // served normally.
        ::tracing::debug!(%error, "a connection to the announced git address ended before its response");
    }
}

/// How long one request on this address may take, from its first header byte to the
/// answer: the core's own HTTP request deadline, so this address bounds a request
/// exactly as the core's HTTP listeners do.
fn request_deadline() -> Duration {
    Duration::from_secs(listener_caps::HTTP_REQUEST_DEADLINE_SECS)
}

/// How many bytes of one request this address reads: the core's per-connection
/// ceiling.
fn body_ceiling() -> usize {
    usize::try_from(listener_caps::MAX_BYTES_PER_CONN).unwrap_or(usize::MAX)
}

/// Answer one request to the announced git address from this emulator's router.
async fn answer(
    callback: EmbeddedEmulator,
    request: ::http::Request<::hyper::body::Incoming>,
) -> ::http::Response<Full<Bytes>> {
    let (parts, body) = request.into_parts();
    if let Some(refused) = refused_before_dispatch(&parts) {
        return refused;
    }
    // One permit per request IN FLIGHT, taken BEFORE a byte of the body is read: what
    // the bodies being read hold at once is then the in-flight cap times the ceiling
    // below, rather than the connection cap times it.
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
            // The router is this same process, so this is no answer about a
            // repository; it is reported as an answer this address could not produce.
            Err(_) => plain(502, NOT_SERVED),
        }
    })
    .await;
    served.unwrap_or_else(|_| plain(504, REQUEST_TIMED_OUT))
}

/// The request the router is handed for one request to this address: the same method,
/// path, query, body and headers — the `Host` among them, so the router resolves the
/// request exactly as it arrived and this service claims it by path as it does at the
/// front door.
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
    let uri = format!("{GIT_SCHEME}://{host}{}{query}", parts.uri.path());
    let mut builder = ::http::Request::builder()
        .method(parts.method.clone())
        .uri(uri);
    for (name, value) in parts.headers.iter() {
        // The two framing headers describe a body this leg re-frames itself.
        if name == ::http::header::CONTENT_LENGTH || name == ::http::header::TRANSFER_ENCODING {
            continue;
        }
        builder = builder.header(name, value);
    }
    let built = builder.body(SdkBody::from(body)).ok()?;
    RtRequest::try_from(built).ok()
}

/// The transport-safety refusals the core applies to its OWN HTTP listeners, applied
/// here from the core's own predicates so this address and a core listener decide them
/// alike: the `Host` allowlist (the DNS-rebinding guard a browser needs), the WebSocket
/// refusal, and the browser check. `None` lets the request through to the router.
fn refused_before_dispatch(
    parts: &::http::request::Parts,
) -> Option<::http::Response<Full<Bytes>>> {
    let header = |name: &str| parts.headers.get(name).and_then(|value| value.to_str().ok());

    // HTTP/1.1 requires a `Host`, and without an authority the rebinding guard cannot
    // be run at all, so a request carrying none is refused rather than served: this
    // fails CLOSED, exactly as the core's own HTTP listener does.
    let Some(host) = header("host").or_else(|| parts.uri.host()) else {
        return Some(plain(421, HOST_REQUIRED));
    };
    if !host_allowed(::service_common::data_plane_addressing::host_only(host)) {
        return Some(plain(421, HOST_NOT_ALLOWED));
    }
    if ::emulator_server::is_websocket_upgrade(
        header("connection"),
        header("upgrade"),
        header(":protocol"),
    ) {
        return Some(plain(501, NO_WEBSOCKETS));
    }
    if let BrowserVerdict::Forbid(reason) = ::emulator_server::classify_browser_request(
        header("origin"),
        header("sec-fetch-site"),
        // This address is the emulator's own, reached from the machine it runs on: the
        // same default a core HTTP listener carries, which admits no remote origin.
        &RemoteOrigins::SameOriginOnly,
        false,
    ) {
        return Some(plain(403, reason));
    }
    None
}

/// Whether a request's `Host` names the interface this address was bound on, by the
/// rule a core HTTP listener applies: a loopback `Host` on the default loopback bind
/// (which is what guards a browser against DNS rebinding), the operator's own address
/// when they bound a routable one, and any `Host` on a wildcard bind — there the
/// operator exposed the port deliberately and no single address names it.
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
            .parse::<::std::net::IpAddr>()
            .is_ok_and(|named| named == bound)
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

/// What it answers a request that arrived while this address was already reading as
/// many bodies as it reads at once.
const SERVER_BUSY: &str = "This address is serving as many requests as it can. Try again.\n";

/// What it answers a request it could not render as one to forward at all.
const NOT_A_REQUEST: &str = "This request could not be read.\n";

/// What it answers a request the router did not answer.
const NOT_SERVED: &str = "This request was not served.\n";

/// One answer of this address's own, for a request the router never saw. Plain text
/// rather than a service envelope: which protocol an answer belongs in is the claiming
/// service's to decide, and no service was asked about this one — and a git client
/// prints a plain-text body as the server's reason.
fn plain(status: u16, message: &str) -> ::http::Response<Full<Bytes>> {
    let body = Bytes::from(message.to_string());
    ::http::Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .header("content-length", body.len().to_string())
        .body(Full::new(body.clone()))
        // Every argument is this function's own, so the builder cannot fail; answer a
        // bare status rather than panicking if it ever does.
        .unwrap_or_else(|_| {
            let mut response = ::http::Response::new(Full::new(body));
            *response.status_mut() = ::http::StatusCode::INTERNAL_SERVER_ERROR;
            response
        })
}
