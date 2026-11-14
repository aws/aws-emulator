// SPDX-License-Identifier: CC0-1.0

//! The SOCKET a Classic load balancer's listeners are reached on: the address the load
//! balancer publishes as its `DNSName`, and the hand-off of every request it accepts to the
//! data plane ([`super::data_plane`]).
//!
//! `CreateAccessPointOutput$DNSName` is "the DNS name of the load balancer" — the address a
//! CLIENT then dials — so it has to be an address something answers on. A name under
//! Elastic Load Balancing's own zone resolves to nothing on the machine this emulator runs
//! on, so a load balancer that published one published an address no client could reach. So
//! each load balancer gets a socket of the EMULATOR's own, opened through the core
//! (`engine.listeners().tcp_plaintext(..)`, which owns the socket, the bind interface, the
//! port range and the connection caps), and publishes the `host:port` that socket answers
//! on ([`crate::state::LoadBalancerEndpoint`]).
//!
//! ONE address serves both schemes a Classic load balancer's listeners carry: the first
//! byte of a connection says whether the client opened with a TLS record or with a request
//! in the clear ([`::service_common::in_band_tls`]), so an `https://` client of an HTTPS or
//! SSL listener and an `http://` client of an HTTP or TCP listener dial the same published
//! address. A TLS connection is handed to the core's own TLS server, which is the only
//! thing in this emulator that holds a key: this module touches no certificate and no key
//! material.
//!
//! Nothing about the ANSWER is decided here: which listener the request reached, which
//! registered instances it may be sent to and which of them it is sent to all stay in
//! [`super::data_plane`], which serves the same request however it was addressed. So a
//! request this socket accepts is handed to that one plane over the in-process transport
//! this emulator reaches itself on, addressed as the RESERVED data-plane path of the region
//! and name of the load balancer this socket was opened for, and marked as having arrived
//! encrypted when it did — so the plane evaluates the listeners that carry traffic of the
//! kind the client sent. It names no listener PORT, and deliberately: this socket is the
//! load balancer's ONE address, so a connection it accepts addresses the load balancer
//! rather than one of its listeners, and which listener answers is the plane's own rule
//! (the lowest-numbered one of that kind, which `super::data_plane` states).
//!
//! Every status and header is computed from what a create or a modify persisted, and this
//! module writes NOTHING: serving traffic moves no status, consumes no quota and bumps no
//! counter.

use ::std::future::Future;
use ::std::pin::Pin;
use ::std::sync::{Arc, OnceLock};

use ::bytes::Bytes;
use ::emulator_server::{listener_caps, ListenerConn, NativeHandler, TlsUpgrader};
use ::http_body_util::{BodyExt as _, Full, Limited};
use ::smithy_server_runtime::EmbeddedEmulator;
use ::tokio::sync::{OwnedSemaphorePermit, Semaphore};
use aws_smithy_runtime_api::client::http::HttpConnector as _;
use aws_smithy_runtime_api::http::Request as RtRequest;
use aws_smithy_types::body::SdkBody;

/// The host the forwarded leg addresses this service's own front door at: the endpoint
/// prefix this model is published under, so the request it hands over is routed to this
/// service exactly as a client's own API call to it is.
const SERVICE_ENDPOINT: &str = "elasticloadbalancing.amazonaws.com";

/// The header the forwarded leg marks a connection the client opened with TLS by, which is
/// what tells the plane the request reached an HTTPS or SSL listener rather than an HTTP or
/// TCP one.
///
/// SET by this leg and dropped from whatever the client sent under that name
/// ([`HEADERS_NOT_FORWARDED`]), so what the plane reads is the scheme the client really
/// dialled this socket with.
pub const ENCRYPTED_CONNECTION_HEADER: &str = "x-emulator-elb-encrypted";

/// How long one request on this socket may take before the connection answers the status a
/// load balancer answers for an answer that did not arrive.
///
/// An EMULATOR bound: the request is served in this process, so this only bounds a request
/// whose serving does not return.
const REQUEST_DEADLINE: ::std::time::Duration = ::std::time::Duration::from_secs(30);

/// What this socket answers a request its own machinery could not hand to the plane at all.
const BAD_GATEWAY: u16 = 502;

/// And the status it answers a request that did not finish inside [`REQUEST_DEADLINE`] —
/// the status a Classic load balancer answers when no answer came back in time.
const GATEWAY_TIMEOUT: u16 = 504;

/// What it answers a request carrying more than one connection may.
const PAYLOAD_TOO_LARGE: u16 = 413;

/// What it answers a request that arrived while this service was already reading as many
/// request bodies as it reads at once.
const SERVICE_UNAVAILABLE: u16 = 503;

/// How long the phase BEFORE a request may take — the first byte and, for a client that
/// dialled TLS, the handshake. A client that completes TCP and then sends nothing holds the
/// connection permit the core issued for it only until this deadline.
const HANDSHAKE_DEADLINE: ::std::time::Duration = ::std::time::Duration::from_secs(10);

/// How many request BODIES this service reads at once across EVERY load balancer's socket:
/// the core's own HTTP in-flight cap, one permit per request, taken before a byte of a body
/// is read and released when the answer is written.
///
/// What one in-flight request may buffer is capped by the core's per-connection ceiling, so
/// what the bodies in flight hold at once is this count times that ceiling — rather than the
/// connection count, or the number of load balancers a caller chose to create, times it. ONE
/// semaphore for the whole service, for that reason. A request that finds no permit is shed
/// with the status a load balancer answers a request it could not take.
fn in_flight() -> Arc<Semaphore> {
    static IN_FLIGHT: OnceLock<Arc<Semaphore>> = OnceLock::new();
    Arc::clone(IN_FLIGHT.get_or_init(|| Arc::new(Semaphore::new(listener_caps::MAX_HTTP_INFLIGHT))))
}

/// The headers the forwarded leg does not copy from the client's request.
///
/// `Host` is this socket's own address, which the forwarded request replaces with the
/// address of this service's front door (the plane resolves the load balancer from the
/// reserved path instead), and the two framing headers describe a body this leg re-frames
/// itself. The encrypted-connection marker is this leg's own: a client cannot claim to have
/// dialled TLS when it did not.
const HEADERS_NOT_FORWARDED: [&str; 4] = [
    "host",
    "content-length",
    "transfer-encoding",
    ENCRYPTED_CONNECTION_HEADER,
];

/// Serves ONE load balancer's socket: it speaks HTTP/1.1 on every connection the core hands
/// it — under TLS where the client opened with it — and gives each request to this service's
/// own data plane.
///
/// Cheap to clone — every field is a handle or a name — which is what the core's `'static`
/// per-connection handler contract asks for.
#[derive(Clone)]
pub struct ListenerSocket {
    /// The region the load balancer this socket serves lives in. A Classic load balancer's
    /// name is unique within one region only, so the region travels with the name in the
    /// reserved path the forwarded leg addresses the plane under.
    region: String,
    /// The load balancer this socket was opened for.
    load_balancer_name: String,
    /// The handle into the emulator this socket belongs to: how a request it accepts
    /// reaches the data plane. The socket is not a second emulator, it is a second ADDRESS
    /// of this one.
    callback: EmbeddedEmulator,
    /// The core's own TLS server, for a client that dials this address with `https://`.
    /// `None` when this emulator has none prepared, in which case such a connection is
    /// closed rather than served in the clear.
    upgrader: Option<TlsUpgrader>,
}

impl ListenerSocket {
    /// The socket handler for one load balancer.
    pub fn new(
        region: String,
        load_balancer_name: String,
        callback: EmbeddedEmulator,
        upgrader: Option<TlsUpgrader>,
    ) -> Self {
        Self {
            region,
            load_balancer_name,
            callback,
            upgrader,
        }
    }
}

impl NativeHandler for ListenerSocket {
    /// One call per accepted connection. The handles are cloned into the connection's own
    /// future, which is what the core's `'static` handler contract asks for.
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let socket = self.clone();
        Box::pin(async move { socket.serve_connection(conn).await })
    }
}

impl ListenerSocket {
    /// Serve one accepted connection: negotiate what the client opened with, then speak
    /// HTTP/1.1 on it until the client closes it.
    async fn serve_connection(&self, conn: ListenerConn) {
        // The one reading of a connection whose encryption is negotiated in band: a TLS
        // record is handed to the core's own TLS server and read decrypted, a request in the
        // clear is read as it stands, and a handshake this emulator cannot complete leaves
        // nothing to serve.
        let Some((conn, encrypted)) = ::tokio::time::timeout(
            HANDSHAKE_DEADLINE,
            ::service_common::in_band_tls::negotiated_scheme(conn, self.upgrader.clone()),
        )
        .await
        .ok()
        .flatten()
        else {
            return;
        };
        let io = ::hyper_util::rt::TokioIo::new(conn);
        let callback = self.callback.clone();
        let region = self.region.clone();
        let load_balancer_name = self.load_balancer_name.clone();
        let service = ::hyper::service::service_fn(move |request: ::http::Request<_>| {
            let callback = callback.clone();
            let region = region.clone();
            let load_balancer_name = load_balancer_name.clone();
            async move {
                // A panic anywhere behind this address is CONTAINED here and answered as
                // the protocol-shaped internal failure, exactly as the endpoint a control
                // call arrives on answers it: one client cannot take the connection, the
                // listener or the process down with it.
                let facts = ::emulator_server::RequestFacts::from_request(&request);
                Ok::<_, ::std::convert::Infallible>(
                    ::emulator_server::contain_panics(
                        facts,
                        answer(callback, region, load_balancer_name, encrypted, request),
                    )
                    .await,
                )
            }
        });
        if let Err(error) = ::hyper::server::conn::http1::Builder::new()
            .timer(::hyper_util::rt::TokioTimer::new())
            .header_read_timeout(REQUEST_DEADLINE)
            .serve_connection(io, service)
            .await
        {
            // A connection the client dropped, or a request this emulator could not frame,
            // ends here and takes nothing else with it: the next connection is served
            // normally.
            ::tracing::debug!(
                %error,
                region = %self.region,
                load_balancer = %self.load_balancer_name,
                "a connection to a load balancer's own address ended before its response"
            );
        }
    }
}

/// Answer one request this socket accepted, from this service's own data plane.
async fn answer(
    callback: EmbeddedEmulator,
    region: String,
    load_balancer_name: String,
    encrypted: bool,
    request: ::http::Request<::hyper::body::Incoming>,
) -> ::http::Response<Full<Bytes>> {
    let (parts, body) = request.into_parts();
    // One permit per request IN FLIGHT, taken BEFORE a byte of the body is read: what the
    // bodies being read hold at once is then the in-flight cap times the ceiling below,
    // rather than the connection cap (or the number of load balancers) times it.
    let Ok(permit) = in_flight().try_acquire_owned() else {
        return rendered(SERVICE_UNAVAILABLE);
    };
    let ceiling = usize::try_from(listener_caps::MAX_BYTES_PER_CONN).unwrap_or(usize::MAX);
    let limited = Limited::new(body, ceiling);
    let served = ::tokio::time::timeout(REQUEST_DEADLINE, async move {
        // Held for the whole read and dispatch, and released with it.
        let _permit: OwnedSemaphorePermit = permit;
        let Ok(collected) = limited.collect().await else {
            return rendered(PAYLOAD_TOO_LARGE);
        };
        let Some(forwarded) = forwarded(
            parts,
            collected.to_bytes(),
            &region,
            &load_balancer_name,
            encrypted,
        ) else {
            return rendered(BAD_GATEWAY);
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
            // The plane is this same process, so this is no answer about a load balancer: it
            // is reported the way a load balancer reports an answer it could not produce.
            Err(_) => rendered(BAD_GATEWAY),
        }
    })
    .await;
    served.unwrap_or_else(|_| rendered(GATEWAY_TIMEOUT))
}

/// The request the data plane is handed for one client request: the same method, path,
/// query, headers and body, addressed to this service's own front door under the reserved
/// data-plane path of `load_balancer_name` IN `region`, and marked encrypted when the client
/// dialled TLS.
///
/// The reserved prefix is what tells the plane WHICH load balancer must answer — the region
/// included, because a Classic load balancer's name is unique within one region only — and
/// the client's own path is appended to it, so a path that could take the request out from
/// under that prefix is not forwarded at all: a request target carrying a dot segment
/// ([`::service_common::data_plane_addressing::carries_a_dot_segment`]) renders as no
/// forwarded request, and this address answers it
/// itself.
///
/// `None` when the client's own request does not render as one, which addresses nothing.
fn forwarded(
    parts: ::http::request::Parts,
    body: Bytes,
    region: &str,
    load_balancer_name: &str,
    encrypted: bool,
) -> Option<RtRequest> {
    let request_path = parts.uri.path();
    if ::service_common::data_plane_addressing::carries_a_dot_segment(request_path) {
        return None;
    }
    let query = parts
        .uri
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let prefix = crate::service::data_plane::data_plane_path(region, load_balancer_name);
    let uri = format!("https://{SERVICE_ENDPOINT}{prefix}{request_path}{query}");
    // The leg has to address the reserved path of THIS load balancer: a target that renders
    // to anything else addresses another plane, and is not forwarded.
    let addressed = uri.parse::<::http::Uri>().ok()?;
    if !addressed.path().starts_with(prefix.as_str()) {
        return None;
    }
    let mut builder = ::http::Request::builder()
        .method(parts.method.clone())
        .uri(uri)
        .header(::http::header::HOST, SERVICE_ENDPOINT);
    if encrypted {
        builder = builder.header(ENCRYPTED_CONNECTION_HEADER, "1");
    }
    for (name, value) in parts.headers.iter() {
        if HEADERS_NOT_FORWARDED.contains(&name.as_str()) {
            continue;
        }
        builder = builder.header(name, value);
    }
    let built = builder.body(SdkBody::from(body)).ok()?;
    RtRequest::try_from(built).ok()
}

/// One answer this socket renders itself, for a request the data plane never saw.
fn rendered(status: u16) -> ::http::Response<Full<Bytes>> {
    ::http::Response::builder()
        .status(status)
        .body(Full::new(Bytes::new()))
        .unwrap_or_else(|_| ::http::Response::new(Full::new(Bytes::new())))
}
