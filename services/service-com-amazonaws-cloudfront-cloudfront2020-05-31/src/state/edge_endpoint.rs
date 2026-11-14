// SPDX-License-Identifier: CC0-1.0

//! The ADDRESSES this service publishes for the traffic it serves, and the listeners that
//! answer there.
//!
//! Two members are such an address. `Distribution$DomainName` is "the domain name that
//! CloudFront assigns when you create a distribution", and
//! `ConnectionGroup$RoutingEndpoint` is "the routing endpoint (also known as the DNS name)
//! that is assigned to the connection group" — the name a distribution TENANT's own domain
//! is pointed at. Each is the address a VIEWER is handed: a client takes it and fetches
//! `https://<it>/<object>`. A name under a zone this emulator does not serve resolves to
//! nothing on the machine it runs on, so a resource that reported one would publish an
//! address no client could reach — the one thing a published endpoint must never be. So each
//! of them gets a listener of the EMULATOR's own, opened through the core
//! (`engine.listeners().tcp_plaintext(..)`, which owns the socket and the bind interface),
//! and publishes the `host:port` that listener answers on. A viewer dials exactly what it
//! was told.
//!
//! A viewer reaches either address over HTTP or over HTTPS — "viewers can use HTTP or
//! HTTPS" is what `ViewerProtocolPolicy` is about — so both are served at that one address:
//! the connection's first byte says which the client opened with, and a TLS record is handed
//! to the core's own TLS server before the request is read
//! ([`::service_common::in_band_tls`], so this service never touches a key or a
//! certificate).
//!
//! The listener is the ADDRESS, not the data plane: everything a viewer request is answered
//! BY — resolving the distribution, the geographic restriction, the cache behaviour, the
//! cache and the origin fetch — stays in [`crate::service::edge`], which serves the same
//! request however it was addressed. So the handler here hands the request it accepts to
//! that one data plane, through the in-process transport this emulator reaches itself over,
//! addressed as the RESERVED data-plane path of the distribution it resolved
//! (`/_aws/cloudfront/{id}/{viewer-path}`): one serving path, reached from several
//! addressings.
//!
//! WHICH distribution that is, is the only thing the two addresses differ in
//! ([`EdgeTarget`]): a distribution's own address serves the one distribution it was opened
//! for, and a connection group's serves the tenant whose DOMAIN the viewer addressed, among
//! the tenants that group routes.
//!
//! A multi-tenant distribution gets no address of its own: it "has no traffic domain of its
//! own" — its viewers reach it through a connection group — so there is nothing to publish
//! and no listener to open for it.
//!
//! A listener's life is its resource's: the handle is held on the store under that
//! resource's id, and dropping it (the delete) closes the port and severs every connection
//! in flight on it.
//!
//! # Which viewers an address admits
//!
//! These addresses are the emulator's own loopback ports, so a page the developer's browser
//! visits can send requests to them — and CORS response headers would only hide the answer
//! from that page, not stop the request arriving. Every request is therefore put through the
//! core's own browser check ([`refused_before_the_edge`]) before the edge sees it, and a
//! request a browser sent from another site is refused `403` unless the distribution this
//! address serves NAMES its origin: the origins the CORS configurations of the response
//! headers policies its cache behaviours attach carry
//! ([`EdgeTarget::granted_origins`]), which is a statement made through the CONTROL plane,
//! where no browser reaches. A distribution that names none admits none, which is the core's
//! own default.
//!
//! That grant is written to make a response headers policy's headers right, so the fact that
//! it also decides which pages this port answers is ANNOUNCED where the operator of this
//! emulator sees it: a warning naming the distribution, the address and the origin, once per
//! address and origin ([`EdgeTarget::announce`]).

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use ::bytes::Bytes;
use ::emulator_server::{listener_caps, BrowserVerdict, ListenerConn, RemoteOrigins, TlsUpgrader};
use ::http_body_util::{BodyExt as _, Full, Limited};
use ::smithy_server_runtime::EmbeddedEmulator;
use ::tokio::sync::{OwnedSemaphorePermit, Semaphore};
use aws_smithy_runtime_api::client::http::HttpConnector;
use aws_smithy_runtime_api::http::Request as RtRequest;
use aws_smithy_types::body::SdkBody;

use super::SERVICE_ENDPOINT;

/// How many viewer request BODIES this service reads at once, across every distribution's
/// edge listener: the core's own HTTP in-flight cap, one permit per request, taken before a
/// byte of a body is read and released when the answer is written.
///
/// What one in-flight request may buffer is capped by [`body_ceiling`], so what the bodies
/// in flight hold at once is this count times that ceiling — the product the core's own
/// HTTP listeners bound, rather than the connection count (or the number of distributions)
/// times it. ONE semaphore for the whole service, because a caller chooses how many
/// distributions exist and each one would otherwise raise the peak again. A request that
/// finds no permit is shed, which is what the core's own HTTP listener answers such a
/// request.
fn in_flight() -> Arc<Semaphore> {
    static IN_FLIGHT: OnceLock<Arc<Semaphore>> = OnceLock::new();
    Arc::clone(IN_FLIGHT.get_or_init(|| Arc::new(Semaphore::new(listener_caps::MAX_HTTP_INFLIGHT))))
}

/// The reserved path a viewer request is handed to the data plane under, ahead of the
/// distribution's id: `/_aws/cloudfront/{id}/{viewer-path}`.
///
/// The segments are [`crate::service::edge`]'s own prefixed path-style addressing, which
/// that module reads on whatever host a request arrives at — so the request this listener
/// forwards resolves to exactly the distribution it was opened for, whatever the viewer
/// path spells.
const EDGE_PATH_PREFIX: &str = "/_aws/cloudfront";

/// The headers the forwarded leg does not copy from the viewer's request.
///
/// `Host` is the listener's own address, which the forwarded request replaces with this
/// service's endpoint (the data plane resolves the distribution from the reserved path
/// instead), and the two framing headers describe a body this leg re-frames itself.
///
/// The BROWSER-SET headers are not copied either, and for a different reason: they are the
/// browser's own classification of the request it sent to THIS address, which is where that
/// classification is read and the request admitted or refused
/// ([`refused_before_the_edge`]). The leg below is this emulator calling itself, not a
/// browser calling anything, and a leg carrying them is one the emulator's own front door
/// refuses as a cross-site browser request — for a viewer request this address has already
/// admitted. The one value the data plane still needs, the viewer's `Origin`, travels under a
/// reserved name of this emulator's own instead
/// ([`crate::service::edge::response_headers::VIEWER_ORIGIN`]).
const HEADERS_NOT_FORWARDED: [&str; 8] = [
    "host",
    "content-length",
    "transfer-encoding",
    VIEWER_ORIGIN_HEADER,
    "origin",
    "sec-fetch-site",
    "sec-fetch-mode",
    "sec-fetch-dest",
];

/// The reserved name the viewer's `Origin` is forwarded under, stated once by the module that
/// reads it.
const VIEWER_ORIGIN_HEADER: &str = crate::service::edge::response_headers::VIEWER_ORIGIN;

/// WHICH distribution a viewer request this address accepted is served from — the one
/// thing the two addresses of this service differ in.
///
/// A DISTRIBUTION's own edge domain serves exactly the distribution it was opened for,
/// whatever the viewer addressed it as ([`ServedDistribution`]). A CONNECTION GROUP's
/// routing endpoint serves the tenants routed through that group, and a viewer reaches one
/// of them by the DOMAIN it addressed — "the routing endpoint (also known as the DNS name)
/// that is assigned to the connection group" is the address a tenant's own domain is
/// pointed at — so the request is resolved against this service's own records
/// ([`ServedConnectionGroup`]). Everything else about serving the request is the data
/// plane's and is shared.
trait EdgeTarget: Send + Sync + 'static {
    /// The distribution whose data plane serves a request addressed to `host`, or `None`
    /// when this address serves none for it — a listener whose resource has not been minted
    /// yet, or a host this address routes to no tenant.
    fn distribution_of(&self, host: Option<&str>) -> Option<String>;

    /// Whether `host` is a name this address ROUTES, beside the addresses of the interface
    /// it is bound on ([`host_allowed`]): a connection group's viewers address the TENANT's
    /// domain, which is a name this service holds a record for rather than an address of
    /// this machine.
    fn routes_host(&self, host: &str) -> bool;

    /// This service's own records, which is where everything this address decides is read
    /// from.
    fn records(&self) -> &::engine::ServiceState<super::ComAmazonawsCloudfrontCloudfront20200531State>;

    /// The `host:port` this address answers on, once the create that opened it has bound the
    /// listener — which is what the announcement below names the address by.
    fn address(&self) -> Option<String>;

    /// The cross-origin grants this address has ANNOUNCED already, so each is announced once
    /// rather than per request ([`Self::announce`]).
    fn announced(&self) -> &Mutex<BTreeSet<String>>;

    /// The browser origins this address serves a request addressed to `host` from: the
    /// origins the distribution it resolves to GRANTS in the CORS configuration of the
    /// response headers policies its cache behaviours attach
    /// ([`super::ComAmazonawsCloudfrontCloudfront20200531State::cors_granted_origins`]).
    ///
    /// A distribution that grants none — one with no CORS configuration at all — admits
    /// none, which is the default this address carries otherwise
    /// ([`refused_before_the_edge`]).
    ///
    /// At most [`MAX_CORS_ORIGINS_READ_PER_REQUEST`] of them are read, so what one viewer
    /// request costs here is bounded whatever a configuration holds.
    fn granted_origins(&self, host: Option<&str>) -> Vec<String> {
        let Some(distribution) = self.distribution_of(host) else {
            return Vec::new();
        };
        let granted = self
            .records()
            .lock()
            .cors_granted_origins(&distribution, MAX_CORS_ORIGINS_READ_PER_REQUEST);
        self.announce(&distribution, &granted);
        granted
    }

    /// Say, where the operator of this emulator sees it, that this address now serves a
    /// browser request from an origin that is not its own.
    ///
    /// A distribution's CORS configuration is written to make a policy's headers right; that
    /// it also decides which pages this loopback address answers is a consequence worth
    /// stating, so the grant is announced ONCE per address and origin, naming the
    /// distribution, the address and the origin. The record is bounded by
    /// [`MAX_CORS_ORIGINS_READ_PER_REQUEST`], the same bound the read above carries, and the
    /// announcement stops when it is full; nothing about which requests are admitted depends
    /// on it.
    fn announce(&self, distribution: &str, granted: &[String]) {
        if granted.is_empty() {
            return;
        }
        let mut announced = self
            .announced()
            .lock()
            .unwrap_or_else(::std::sync::PoisonError::into_inner);
        for origin in granted {
            if announced.len() >= MAX_CORS_ORIGINS_READ_PER_REQUEST
                || !announced.insert(origin.clone())
            {
                continue;
            }
            ::tracing::warn!(
                distribution = %distribution,
                address = %self.address().unwrap_or_default(),
                origin = %origin,
                "this distribution's response headers policy names this origin, so this \
                 address serves a browser request a page of that origin sends; every other \
                 site is refused"
            );
        }
    }
}

/// The most origins the admission above reads out of one distribution's configuration for one
/// viewer request.
///
/// A viewer request is decided before a path has selected a cache behaviour, so the grants of
/// EVERY behaviour's policy are read: a distribution carries at most
/// [`super::MAX_CACHE_BEHAVIORS`] ordered behaviours plus the default one, and this allows ten
/// granted origins for each of them — the share CloudFront's published quota gives a policy's
/// other per-policy lists ([`super::MAX_HEADERS_IN_POLICY`]). So what one request costs here is
/// a constant, whatever a caller's configuration holds, and an origin past the bound is simply
/// not admitted: this fails CLOSED.
const MAX_CORS_ORIGINS_READ_PER_REQUEST: usize =
    super::MAX_HEADERS_IN_POLICY * (super::MAX_CACHE_BEHAVIORS + 1);

/// The target of a DISTRIBUTION's own edge address: the one distribution the create that
/// opened the listener minted, whatever host the viewer addressed it by.
struct ServedDistribution {
    /// Published to the handler once the create that opened the listener has minted the id.
    /// A request that arrives before then is answered as a request for no distribution.
    distribution: Arc<OnceLock<String>>,
    /// The `host:port` this address answers on, published once the listener is bound.
    address: Arc<OnceLock<String>>,
    /// This service's own records, read for what the distribution this address serves
    /// GRANTS ([`EdgeTarget::granted_origins`]). Read, never written.
    state: ::engine::ServiceState<super::ComAmazonawsCloudfrontCloudfront20200531State>,
    /// The grants already announced ([`EdgeTarget::announce`]).
    announced: Mutex<BTreeSet<String>>,
}

impl EdgeTarget for ServedDistribution {
    fn distribution_of(&self, _host: Option<&str>) -> Option<String> {
        self.distribution.get().cloned()
    }

    /// A viewer reaches this address by DIALLING it, so the only hosts it serves are the
    /// addresses of the interface it is bound on.
    fn routes_host(&self, _host: &str) -> bool {
        false
    }

    fn records(
        &self,
    ) -> &::engine::ServiceState<super::ComAmazonawsCloudfrontCloudfront20200531State> {
        &self.state
    }

    fn address(&self) -> Option<String> {
        self.address.get().cloned()
    }

    fn announced(&self) -> &Mutex<BTreeSet<String>> {
        &self.announced
    }
}

/// The target of a CONNECTION GROUP's routing endpoint: the distribution of the tenant
/// whose domain the viewer addressed, among the tenants this group routes.
///
/// The store is read, never written: resolving a viewer request changes nothing a later
/// caller sees.
struct ServedConnectionGroup {
    /// Published to the handler once the create that opened the listener has minted the
    /// group's id, exactly as [`ServedDistribution`]'s is.
    group: Arc<OnceLock<String>>,
    /// The `host:port` this address answers on, published once the listener is bound.
    address: Arc<OnceLock<String>>,
    /// This service's own records, the single source of truth for which tenant a domain is
    /// routed to.
    state: ::engine::ServiceState<super::ComAmazonawsCloudfrontCloudfront20200531State>,
    /// The grants already announced ([`EdgeTarget::announce`]).
    announced: Mutex<BTreeSet<String>>,
}

impl EdgeTarget for ServedConnectionGroup {
    /// The distribution of the tenant `host` names: the tenant this service routes that
    /// domain to (`distribution_tenant_by_domain`, which answers only for a domain whose
    /// certificate is validated), provided it is routed through THIS group and its own
    /// `Enabled` says it serves traffic — "whether the distribution tenant is enabled".
    ///
    /// The host is read AS ADDRESSED first and then without its port, the same order
    /// [`crate::service::edge`] reads a distribution's published name in: a domain this
    /// emulator published for a resource carries a PORT, which is part of the name a viewer
    /// was handed, while a customer's own domain does not and is addressed with whatever
    /// port the viewer dialled.
    ///
    /// `None` for every other host, so one group's address never serves another group's
    /// tenant and a disabled tenant serves nothing.
    fn distribution_of(&self, host: Option<&str>) -> Option<String> {
        let group = self.group.get()?;
        let addressed = host?;
        let store = self.state.lock();
        let tenant = store
            .distribution_tenant_by_domain(addressed)
            .or_else(|| {
                store.distribution_tenant_by_domain(
                    ::service_common::data_plane_addressing::host_only(addressed),
                )
            })?;
        (&tenant.connection_group_id == group && tenant.enabled)
            .then(|| tenant.distribution_id.clone())
    }

    fn routes_host(&self, host: &str) -> bool {
        self.distribution_of(Some(host)).is_some()
    }

    fn records(
        &self,
    ) -> &::engine::ServiceState<super::ComAmazonawsCloudfrontCloudfront20200531State> {
        &self.state
    }

    fn address(&self) -> Option<String> {
        self.address.get().cloned()
    }

    fn announced(&self) -> &Mutex<BTreeSet<String>> {
        &self.announced
    }
}

/// One address of this service: the `host:port` it answers on, the resource it was opened
/// for, and the handle that keeps the port open.
///
/// Dropping it closes the port, so the record is held for exactly as long as the resource
/// it belongs to.
pub struct EdgeEndpoint {
    /// The `host:port` a viewer dials, which is what the resource publishes — a
    /// distribution's `DomainName`, a connection group's `RoutingEndpoint`.
    authority: String,
    /// The resource this listener serves, published to the handler once the create that
    /// opened the listener has minted its id.
    resource: Arc<OnceLock<String>>,
    /// The listener itself. Never read: it is held because dropping it closes the port.
    _listener: ::emulator_server::ResourceListener,
}

impl std::fmt::Debug for EdgeEndpoint {
    /// Everything about this record that is not a live socket: the address it answers on
    /// and the resource it serves. `ResourceListener` is not `Debug`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EdgeEndpoint")
            .field("authority", &self.authority)
            .field("resource", &self.resource.get())
            .finish()
    }
}

impl EdgeEndpoint {
    /// The `host:port` a viewer reaches this resource at — a distribution's `DomainName`,
    /// a connection group's `RoutingEndpoint`.
    pub fn authority(&self) -> &str {
        &self.authority
    }

    /// Bind this listener to the resource it was opened for, once the create that opened it
    /// has minted the id. Called once; a second call changes nothing.
    pub fn serves(&self, resource_id: &str) {
        let _ = self.resource.set(resource_id.to_string());
    }
}

/// Open the listener one distribution's edge is served on, and report the address it
/// answers at — or `None` when this engine vends no listeners, in which case the caller
/// falls back to the name CloudFront's own zone would carry.
///
/// `callback` is the handle into the emulator this request is running within
/// (`ctx.callback()`), which is how a request the listener accepts reaches the data plane:
/// the listener is not a second emulator, it is a second ADDRESS of this one.
pub async fn open_edge_endpoint<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: EmbeddedEmulator,
) -> Option<EdgeEndpoint> {
    let shared = (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()?;
    let resource: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
    let address: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
    let target = ServedDistribution {
        distribution: Arc::clone(&resource),
        address: Arc::clone(&address),
        state: shared.state(),
        announced: Mutex::new(BTreeSet::new()),
    };
    open(shared, callback, resource, address, Arc::new(target)).await
}

/// Open the listener one CONNECTION GROUP's routing endpoint is served on, and report the
/// address it answers at — or `None` on an engine that vends no listeners, exactly as
/// [`open_edge_endpoint`] answers for a distribution.
///
/// The address serves the tenants this group routes: a viewer dials it with the TENANT's
/// own domain as the host it addressed, and the request is served from the distribution
/// that tenant belongs to ([`ServedConnectionGroup`]).
pub async fn open_connection_group_endpoint<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: EmbeddedEmulator,
) -> Option<EdgeEndpoint> {
    let shared = (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()?;
    let resource: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
    let address: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
    let target = ServedConnectionGroup {
        group: Arc::clone(&resource),
        address: Arc::clone(&address),
        state: shared.state(),
        announced: Mutex::new(BTreeSet::new()),
    };
    open(shared, callback, resource, address, Arc::new(target)).await
}

/// Open one address of this service, serving `target`: the ONE place a listener of this
/// module is bound, so a distribution's edge domain and a connection group's routing
/// endpoint are opened, bounded and torn down alike.
async fn open(
    shared: &::engine::InMemoryEngine,
    callback: EmbeddedEmulator,
    resource: Arc<OnceLock<String>>,
    address: Arc<OnceLock<String>>,
    target: Arc<dyn EdgeTarget>,
) -> Option<EdgeEndpoint> {
    let listeners = shared.listeners();
    let handler = EdgeFrontDoor {
        target,
        callback,
        // The core's own TLS server, lent to this plaintext listener so the `https://`
        // half of the address is terminated by the core rather than here. A listener
        // this emulator could prepare none for serves the `http://` half alone.
        upgrader: listeners.tls_upgrader().ok(),
    };
    // Opened on a task of its OWN, so a runtime that cannot bind a socket at all — one
    // built with no IO driver, which is what a host that drives this emulator in process
    // may hand a request — leaves this create without an address of its own instead of
    // taking the request down with it: the join answers `Err`, this answers `None`, and the
    // caller falls back to the name CloudFront's own zone would carry.
    let opened = ::tokio::spawn(async move { listeners.tcp_plaintext(handler).await.ok() }).await;
    let listener = opened.ok().flatten()?;
    let authority = ::service_common::data_plane_addressing::listener_authority(&listener);
    // The handler now knows the address it answers on, which is what it names this address by
    // when it announces a cross-origin grant ([`EdgeTarget::announce`]).
    let _ = address.set(authority.clone());
    Some(EdgeEndpoint {
        authority,
        resource,
        _listener: listener,
    })
}

/// Serves one address of this service: it hands every request it accepts to this service's
/// own data plane, over the in-process transport, and answers with what that data plane
/// answered.
struct EdgeFrontDoor {
    target: Arc<dyn EdgeTarget>,
    callback: EmbeddedEmulator,
    upgrader: Option<TlsUpgrader>,
}

impl ::emulator_server::NativeHandler for EdgeFrontDoor {
    /// One call per accepted connection. The handles are cloned into the connection's own
    /// future, which is what the core's `'static` handler contract asks for.
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let target = Arc::clone(&self.target);
        let callback = self.callback.clone();
        let upgrader = self.upgrader.clone();
        Box::pin(serve_connection(target, callback, upgrader, conn))
    }
}

/// Speak HTTP/1.1 on one accepted connection until the viewer closes it, answering each
/// request from this service's own data plane.
///
/// The encryption is negotiated in band first ([`::service_common::in_band_tls`]), so one
/// address serves the `http://` and the `https://` a viewer may dial. HTTP/1.1 only, as
/// the emulator's own front door offers: every request is fully buffered, so nothing
/// relies on HTTP/2 streaming. The header-read deadline and the body ceiling are the
/// core's, so a viewer that dribbles a request line or sends more than one connection may
/// carry cannot hold the connection permit the core issued for it.
async fn serve_connection(
    target: Arc<dyn EdgeTarget>,
    callback: EmbeddedEmulator,
    upgrader: Option<TlsUpgrader>,
    conn: ListenerConn,
) {
    // WHICH half of this one address the viewer dialled is read here and nowhere else: the
    // connection is all that says so, and the data plane is told through the scheme of the
    // request it is handed ([`forwarded`]), which is where the edge reads the viewer's
    // scheme from.
    let Some((conn, encrypted)) =
        ::service_common::in_band_tls::negotiated_scheme(conn, upgrader).await
    else {
        return;
    };
    let io = ::hyper_util::rt::TokioIo::new(conn);
    let service = ::hyper::service::service_fn(move |request: ::http::Request<_>| {
        let target = Arc::clone(&target);
        let callback = callback.clone();
        async move {
            // A panic anywhere behind this address — in the dispatch machinery or in the
            // handler of the operation the forwarded leg reaches — is CONTAINED here and
            // answered as the protocol-shaped internal failure, exactly as the endpoint a
            // control call arrives on answers the same request: one viewer cannot take the
            // connection, the listener or the process down with it, and the next request is
            // served normally.
            let facts = ::emulator_server::RequestFacts::from_request(&request);
            Ok::<_, ::std::convert::Infallible>(
                ::emulator_server::contain_panics(
                    facts,
                    answer(target, callback, request, encrypted),
                )
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
        // A connection the viewer dropped, or a request this emulator could not frame, ends
        // here and takes nothing else with it: the next connection is served normally.
        ::tracing::debug!(%error, "a connection to an address of this service ended before its response");
    }
}

/// How long one viewer request may take, from its first header byte to the answer: the
/// core's own HTTP request deadline, so this address bounds a request exactly as the core's
/// HTTP listeners do.
fn request_deadline() -> Duration {
    Duration::from_secs(listener_caps::HTTP_REQUEST_DEADLINE_SECS)
}

/// How many bytes of one viewer request this address reads: the core's per-connection
/// ceiling.
fn body_ceiling() -> usize {
    usize::try_from(listener_caps::MAX_BYTES_PER_CONN).unwrap_or(usize::MAX)
}

/// Answer one viewer request from the data plane of the distribution this address resolves
/// it to ([`EdgeTarget`]).
async fn answer(
    target: Arc<dyn EdgeTarget>,
    callback: EmbeddedEmulator,
    request: ::http::Request<::hyper::body::Incoming>,
    encrypted: bool,
) -> ::http::Response<Full<Bytes>> {
    let (parts, body) = request.into_parts();
    // One permit per request IN FLIGHT, taken BEFORE this request makes this service do
    // anything at all — before the stored records the admission is decided from are read and
    // before a byte of the body is: what the requests in flight cost at once is then the
    // in-flight cap times what one of them may cost, rather than the connection cap (or the
    // number of distributions) times it. Everything the permit covers is inside the request
    // deadline below as well, so neither bound leaves work outside it.
    let Ok(permit) = in_flight().try_acquire_owned() else {
        return generated(SERVICE_UNAVAILABLE, SERVER_BUSY);
    };
    let limited = Limited::new(body, body_ceiling());
    let served = ::tokio::time::timeout(request_deadline(), async move {
        // Held for the whole read and dispatch, and released with it.
        let _permit: OwnedSemaphorePermit = permit;
        let addressed = parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
            .or_else(|| parts.uri.host().map(str::to_string));
        if let Some(refused) = refused_before_the_edge(&parts, target.as_ref()) {
            return refused;
        }
        let Some(distribution) = target.distribution_of(addressed.as_deref()) else {
            // Either the create that opened this listener has not named its resource yet, or
            // this address routes the host the viewer addressed to no tenant: there is
            // nothing here to serve, reported as the edge reports a request it can resolve to
            // no distribution.
            return generated(BAD_GATEWAY, NO_DISTRIBUTION);
        };
        let Ok(collected) = limited.collect().await else {
            return generated(PAYLOAD_TOO_LARGE, BODY_TOO_LARGE);
        };
        let Some(forwarded) = forwarded(parts, collected.to_bytes(), &distribution, encrypted)
        else {
            return generated(BAD_GATEWAY, NO_DISTRIBUTION);
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
            // The data plane is this same process, so this is no answer about a
            // distribution; it is reported as the edge reports an answer it could not
            // produce.
            Err(_) => generated(BAD_GATEWAY, NO_DISTRIBUTION),
        }
    })
    .await;
    served.unwrap_or_else(|_| generated(GATEWAY_TIMEOUT, REQUEST_TIMED_OUT))
}

/// The request the data plane is handed for one viewer request: the same method, body and
/// headers, addressed to this service's own endpoint under the reserved data-plane path of
/// `distribution` — so the viewer path travels where this client can carry it.
///
/// The SCHEME of that address is the one the viewer dialled (`encrypted`), which is how the
/// data plane learns it: this one address serves both halves, and the scheme is what the
/// edge states to an origin as `CloudFront-Forwarded-Proto`.
///
/// The distribution segment is what makes this listener serve ONE distribution, so the
/// viewer path may not be able to walk out from under it: a path carrying a dot segment is
/// refused ([`within_the_distribution`]) rather than forwarded, and the built target is
/// checked to still begin with this distribution's own prefix before it is sent. `None` for
/// such a path, and for a viewer request that does not render as one — in both cases this
/// listener addresses nothing.
fn forwarded(
    parts: ::http::request::Parts,
    body: Bytes,
    distribution: &str,
    encrypted: bool,
) -> Option<RtRequest> {
    let viewer_path = within_the_distribution(parts.uri.path())?;
    let query = parts
        .uri
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let scheme = if encrypted { "https" } else { "http" };
    let within = format!("{EDGE_PATH_PREFIX}/{distribution}");
    let path = format!("{within}{viewer_path}");
    // The built path still names THIS distribution's data plane: a path that does not is not
    // forwarded at all, whatever it would have resolved to.
    if !path.starts_with(&format!("{within}/")) {
        return None;
    }
    let uri = format!("{scheme}://{SERVICE_ENDPOINT}{path}{query}");
    let mut builder = ::http::Request::builder()
        .method(parts.method.clone())
        .uri(uri)
        .header(::http::header::HOST, SERVICE_ENDPOINT);
    for (name, value) in parts.headers.iter() {
        if HEADERS_NOT_FORWARDED.contains(&name.as_str()) {
            continue;
        }
        builder = builder.header(name, value);
    }
    // The viewer's `Origin`, under the reserved name of this emulator's own: the data plane
    // decides the response headers policy's CORS group from it, and this leg stays a request
    // no browser sent.
    if let Some(origin) = parts
        .headers
        .get(::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        builder = builder.header(VIEWER_ORIGIN_HEADER, origin);
    }
    let built = builder.body(SdkBody::from(body)).ok()?;
    RtRequest::try_from(built).ok()
}

/// The viewer path as this listener may forward it, or `None` for one that could address
/// something other than this distribution's data plane.
///
/// The forwarded target carries the viewer path under `{EDGE_PATH_PREFIX}/{distribution}`,
/// and a relative path reference (RFC 3986 `.` and `..` segments, in any spelling a decoder
/// would read as one) is resolved by whoever resolves that target — so such a path could name
/// a prefix above the distribution's own. It is refused here instead, which is the answer a
/// listener gives a request it resolves to no distribution: a viewer addresses an object,
/// never a path above the distribution it addressed.
fn within_the_distribution(path: &str) -> Option<&str> {
    if !path.starts_with('/') {
        return None;
    }
    let dot_segment = path.split('/').any(|segment| {
        let decoded = segment.replace("%2e", ".").replace("%2E", ".");
        decoded == "." || decoded == ".."
    });
    (!dot_segment).then_some(path)
}

/// The transport-safety refusals the core applies to its OWN HTTP listeners, applied here
/// from the core's own predicates so this address and a core listener decide them alike:
/// the `Host` allowlist (the DNS-rebinding guard a browser needs), the WebSocket refusal,
/// and the browser check. `None` lets the request through to the data plane.
///
/// `target` widens the `Host` allowlist by exactly the names this address ROUTES
/// ([`EdgeTarget::routes_host`]): a connection group's viewers address the TENANT's own
/// domain rather than the loopback address they dialled, so a name this service holds a
/// routed tenant record for is served there, and every other name is refused as before. The
/// admitted names are this service's own records — the same names the emulator's front door
/// already serves a viewer request on host-style — never a name taken from the request, and
/// the browser check below still decides what a browser may do with any of them.
///
/// The browser check runs on EVERY request this address accepts, as it does on a core HTTP
/// listener, and the only thing that differs is the allowlist it runs against: the ORIGINS
/// the distribution this address serves NAMES ([`EdgeTarget::granted_origins`]), through the
/// CORS configuration of the response headers policies its cache behaviours attach. That is a
/// statement made through the CONTROL plane, which no browser reaches, so it is an
/// allowlist the operator of this emulator wrote, carried by the core's own
/// [`RemoteOrigins::Exact`] — read here from this service's own records and never from the
/// request. Every other browser request from another site is refused before the edge exactly
/// as the core refuses it, whatever its method: this address is reachable from a page the
/// developer's browser visits, and CORS response headers would only hide the answer from that
/// page, not stop the request arriving. A distribution that names no origin therefore carries
/// the core's own default, admitting none, and a grant that admits an origin is announced
/// where the operator sees it ([`EdgeTarget::announce`]).
///
/// The `allow_remote` the check is given is `true` because this address is a VIEWER data
/// plane and nothing else: it serves one distribution's objects, never this emulator's
/// control plane or its administrative endpoint, which is what that flag keeps a configured
/// remote origin away from.
fn refused_before_the_edge(
    parts: &::http::request::Parts,
    target: &dyn EdgeTarget,
) -> Option<::http::Response<Full<Bytes>>> {
    let header = |name: &str| parts.headers.get(name).and_then(|value| value.to_str().ok());

    // HTTP/1.1 requires a `Host`, and without an authority the rebinding guard cannot be
    // run at all, so a request carrying none is refused rather than served: this fails
    // CLOSED, exactly as the core's own HTTP listener does.
    let Some(host) = header("host").or_else(|| parts.uri.host()) else {
        return Some(generated(MISDIRECTED_REQUEST, HOST_REQUIRED));
    };
    let bare = ::service_common::data_plane_addressing::host_only(host);
    if !host_allowed(bare) && !target.routes_host(host) {
        return Some(generated(MISDIRECTED_REQUEST, HOST_NOT_ALLOWED));
    }
    if ::emulator_server::is_websocket_upgrade(
        header("connection"),
        header("upgrade"),
        header(":protocol"),
    ) {
        return Some(generated(NOT_IMPLEMENTED, NO_WEBSOCKETS));
    }
    let granted = target.granted_origins(Some(host));
    if let BrowserVerdict::Forbid(reason) = ::emulator_server::classify_browser_request(
        header("origin"),
        header("sec-fetch-site"),
        // The origins the distribution this address serves NAMES, and nothing else.
        &RemoteOrigins::Exact(granted),
        true,
    ) {
        return Some(generated(FORBIDDEN, reason));
    }
    None
}

/// Whether a viewer request's `Host` names the interface this address was bound on, by the
/// rule a core HTTP listener applies: a loopback `Host` on the default loopback bind (which
/// is what guards a browser against DNS rebinding), the operator's own address when they
/// bound a routable one, and any `Host` on a wildcard bind — there the operator exposed the
/// port deliberately and no single address names it.
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

/// What this address answers a request it could not hand to the data plane at all:
/// CloudFront's own status for an answer it produced itself instead of an origin's.
const BAD_GATEWAY: u16 = 502;

/// What it answers a request whose `Host` is not the address it serves, or that names none.
const MISDIRECTED_REQUEST: u16 = 421;

/// What it answers a WebSocket upgrade, which this address does not serve.
const NOT_IMPLEMENTED: u16 = 501;

/// What it answers a request a browser made cross-site.
const FORBIDDEN: u16 = 403;

/// What it answers a request that arrived while this service was already reading as many
/// viewer bodies as it reads at once.
const SERVICE_UNAVAILABLE: u16 = 503;

/// The body of an answer for a request shed at the in-flight cap.
const SERVER_BUSY: &str = "This address is serving as many requests as it can. Try again.\n";

/// What it answers a request that did not finish within one request deadline.
const GATEWAY_TIMEOUT: u16 = 504;

/// What it answers a request carrying more than one connection may.
const PAYLOAD_TOO_LARGE: u16 = 413;

/// The body of an answer for a request that reached no distribution.
const NO_DISTRIBUTION: &str = "This address serves no distribution.\n";

/// The body of an answer for a request larger than this address accepts.
const BODY_TOO_LARGE: &str = "This request is larger than this address accepts.\n";

/// The body of an answer for a request that outlasted one request deadline.
const REQUEST_TIMED_OUT: &str = "This request took longer than this address allows.\n";

/// The body of an answer for a request whose `Host` is not the address it serves.
const HOST_NOT_ALLOWED: &str = "This address does not serve that host.\n";

/// The body of an answer for a request that names no host at all, which no HTTP/1.1 request
/// may.
const HOST_REQUIRED: &str = "This address requires the host it is addressed by.\n";

/// The body of an answer for a WebSocket upgrade, which no distribution's edge serves.
const NO_WEBSOCKETS: &str = "This address does not serve WebSocket connections.\n";

/// One answer of this address's own, for a viewer request the data plane never saw. Plain
/// text, as the edge's own generated answers are.
fn generated(status: u16, message: &str) -> ::http::Response<Full<Bytes>> {
    let body = Bytes::from(message.to_string());
    ::http::Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(Full::new(body.clone()))
        // Every argument is this function's own, so the builder cannot fail; answer a bare
        // status rather than panicking if it ever does.
        .unwrap_or_else(|_| {
            let mut response = ::http::Response::new(Full::new(body));
            *response.status_mut() = ::http::StatusCode::BAD_GATEWAY;
            response
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_viewer_path_that_could_leave_the_distribution_is_not_forwarded() {
        // Rows differ along WHERE the dot segment sits and HOW it is spelled — a leading,
        // middle and trailing segment, a single dot, percent-encoded dots in either case —
        // and along the ordinary paths that must still be forwarded: a dot inside a name, a
        // hidden file, the root, and a path whose query carries dots.
        let cases: [(&str, &str, bool); 12] = [
            ("the root", "/", true),
            ("an object", "/items/one.json", true),
            ("a dot inside a segment", "/a..b/c", true),
            ("a hidden file", "/.well-known/x", true),
            ("three dots", "/.../x", true),
            ("a leading parent segment", "/../2020-05-31/distribution", false),
            ("a parent segment in the middle", "/items/../../x", false),
            ("a trailing parent segment", "/items/..", false),
            ("a single dot segment", "/./items", false),
            ("a percent-encoded parent segment", "/%2e%2e/x", false),
            ("a percent-encoded parent segment, upper case", "/%2E%2E/x", false),
            ("a percent-encoded dot segment", "/%2e/x", false),
        ];
        for (case, path, forwarded) in cases {
            let actual = within_the_distribution(path).is_some();
            assert_eq!(
                actual, forwarded,
                "case {case:?} ({path:?}): expected the path to be forwarded == \
                 {forwarded}, got {actual}"
            );
        }
    }
}
