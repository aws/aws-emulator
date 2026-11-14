// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS a cluster's Kubernetes API server is reached at, and what answers
//! there.
//!
//! `Cluster$endpoint` is "the endpoint for your Kubernetes API server" and
//! `Certificate$data` is "the Base64-encoded certificate data required to
//! communicate with your cluster. Add this to the `certificate-authority-data`
//! section of the kubeconfig file for your cluster". Neither member is a label a
//! caller reads back: the documented thing to do with the pair is to build a
//! kubeconfig out of them and then TALK to the cluster — `POST
//! /api/v1/namespaces/{namespace}/pods`, `POST
//! /api/v1/namespaces/{namespace}/serviceaccounts/{name}/token` and the rest of
//! the Kubernetes API. So the endpoint has to be an address this emulator
//! answers the Kubernetes API on, under a certificate the published authority
//! signed.
//!
//! A name under Amazon EKS's own zone resolves to nothing on the machine this
//! emulator runs on, and the emulator's own front door serves the AWS control
//! plane rather than a cluster's Kubernetes paths — so every cluster gets a
//! listener of the EMULATOR's own, opened through the core
//! (`engine.listeners().tcp_plaintext(..)`, which owns the socket, the bind
//! interface — loopback unless the operator chose otherwise — the port range and
//! the connection caps), and publishes the `host:port` that listener answers on.
//!
//! # What the address serves, and under which certificate
//!
//! The first byte of a connection says what the client opened with
//! ([`::service_common::in_band_tls::TLS_RECORD_FIRST_BYTE`]): a TLS record, or a
//! request in the clear. ONE announced address therefore serves both the
//! `https://` a client of AWS dials and the `http://` a client that verifies
//! nothing dials.
//!
//! A TLS connection is terminated HERE rather than by the core's own TLS server,
//! and that is the point: the core presents the EMULATOR's certificate, while
//! what a client built from this cluster's kubeconfig trusts is the cluster's own
//! published certificate authority. So the handshake is answered with the
//! certificate that authority issued when the cluster was created
//! ([`provision_cluster_authority`]) — leaf first, the authority behind it — and
//! a client trusting nothing but `certificateAuthority.data` verifies it. The key
//! stays in memory for as long as the cluster exists, goes into no response and
//! no log, and is never written to disk.
//!
//! # What answers the request
//!
//! The objects ([`super::kubernetes_objects`]) and nothing else: every answer is
//! computed from the namespaces, service accounts and pods the cluster holds. The
//! one write a request makes is the object it creates. A projected service account
//! token is minted by the EKS Auth plane ([`super::pod_identity_plane`]), because
//! that plane is what answers for the token afterwards — the cluster's record of
//! what it issued is what `AssumeRoleForPodIdentity` resolves a presented token
//! against, so there is one registry rather than two.
//!
//! Dropping the handle closes the port and severs what is in flight, which is what
//! `DeleteCluster` does: a deleted cluster stops answering.

use ::std::future::Future;
use ::std::pin::Pin;
use ::std::sync::{Arc, OnceLock};
use ::std::time::{Duration as StdDuration, SystemTime};

use ::bytes::Bytes;
use ::emulator_server::{listener_caps, ListenerConn, NativeHandler};
use ::http_body_util::{BodyExt as _, Full, Limited};
use ::service_common::in_band_tls::{Prefixed, TLS_RECORD_FIRST_BYTE};
use ::tokio::io::AsyncReadExt as _;
use ::tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::kubernetes_objects::{ApiRefusal, ClusterObjects};
use super::pod_identity_plane::AuthPlane;
use super::ComAmazonawsEksAwswesleyfrontendState as ServiceStore;

/// The path every Kubernetes core-API request is addressed under.
const CORE_API_PREFIX: [&str; 2] = ["api", "v1"];

/// How many request BODIES this service reads at once across every cluster's
/// Kubernetes API address: the core's own HTTP in-flight cap, one permit per
/// request, taken before a byte of a body is read and released when the answer is
/// written.
///
/// ONE semaphore for the whole service, because a caller chooses how many
/// clusters exist and each one would otherwise raise the peak again. A request
/// that finds no permit is shed with the status the API server answers a request
/// it could not take.
fn in_flight() -> Arc<Semaphore> {
    static IN_FLIGHT: OnceLock<Arc<Semaphore>> = OnceLock::new();
    Arc::clone(
        IN_FLIGHT.get_or_init(|| Arc::new(Semaphore::new(listener_caps::MAX_HTTP_INFLIGHT))),
    )
}

/// How long one request may take, from its first header byte to the answer: the
/// core's own HTTP request deadline, so this address bounds a request exactly as
/// the core's HTTP listeners do.
fn request_deadline() -> StdDuration {
    StdDuration::from_secs(listener_caps::HTTP_REQUEST_DEADLINE_SECS)
}

/// How long a TLS handshake on this address may take, matching the deadline the
/// core puts on a handshake its own TLS server runs: a client that completes TCP
/// and then stalls the handshake cannot hold the connection permit the core
/// issued for it.
const HANDSHAKE_DEADLINE: StdDuration = StdDuration::from_secs(10);

/// How many bytes of one request this address reads: the core's per-connection
/// ceiling.
fn body_ceiling() -> usize {
    usize::try_from(listener_caps::MAX_BYTES_PER_CONN).unwrap_or(usize::MAX)
}

/// One cluster's Kubernetes API plane: the objects it holds, the address it is
/// published at, and what keeps that address open.
pub struct KubernetesApiPlane {
    /// The `host:port` a client dials this cluster's API server at, and `None`
    /// for a cluster whose listener could not be opened — which then publishes no
    /// endpoint at all rather than an address nothing serves.
    authority: Option<String>,
    /// The Kubernetes objects this cluster holds.
    pub objects: ClusterObjects,
    /// The TLS server this address answers a `https://` client with: the
    /// certificate the cluster's own certificate authority issued, and its key.
    /// `None` when the material could not be loaded, in which case a client that
    /// dialled TLS is dropped rather than answered in the clear.
    tls: Option<Arc<::rustls::ServerConfig>>,
    /// Held only to keep the port open; the core owns the socket. Dropping it
    /// closes the port and severs what is in flight.
    _listener: Option<::emulator_server::ResourceListener>,
}

impl KubernetesApiPlane {
    /// The URL this cluster publishes as its `endpoint`, and `None` for a plane
    /// with no address.
    #[must_use]
    pub fn endpoint_url(&self) -> Option<String> {
        self.authority
            .as_ref()
            .map(|authority| format!("https://{authority}"))
    }

    /// The `host:port` this plane answers on.
    #[must_use]
    pub fn authority(&self) -> Option<&str> {
        self.authority.as_deref()
    }
}

/// Written by hand rather than derived: the plane holds a live socket and a TLS
/// configuration carrying a private key, neither of which renders. What is
/// diagnosable is the address it answers on and how many objects it holds.
impl ::std::fmt::Debug for KubernetesApiPlane {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("KubernetesApiPlane")
            .field("authority", &self.authority)
            .field("namespaces", &self.objects.namespaces.len())
            .finish_non_exhaustive()
    }
}

/// The names the certificate this cluster's API server presents is issued for.
///
/// A client reaches the endpoint at the address the listener answers on, which on
/// this machine is a loopback address or the interface the operator bound, so
/// those are the names the certificate has to carry for a verifying client to
/// accept it. The host Amazon EKS publishes for the cluster is carried too, so a
/// client that resolves that name to this machine verifies the certificate as
/// well.
#[must_use]
pub fn api_server_certificate_names(resource_id: &str, region: &str) -> Vec<String> {
    let mut names = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];
    if let Ok(bound) = ::emulator_server::bind_address_from_env() {
        let name = bound.to_string();
        if !bound.is_unspecified() && !names.contains(&name) {
            names.push(name);
        }
    }
    let published = super::derive_cluster_endpoint(resource_id, region);
    if let Some(host) = published.strip_prefix("https://") {
        names.push(host.to_string());
    }
    names
}

/// Open the listener the cluster keyed `key` publishes its Kubernetes API at, seed
/// the objects its API server starts with, and PUBLISH the address on the stored
/// record — so every later read of that cluster reports the endpoint this emulator
/// answers it at.
///
/// Called by the create that stored the cluster, after the store's lock is
/// released. IDEMPOTENT, and checked before a socket is bound: a cluster that
/// already has a plane keeps it, because a client may be dialling it.
///
/// A cluster whose listener cannot be opened — an engine that vends none, no
/// runtime to bind a socket on, a port range that is exhausted — keeps its objects
/// and publishes NO endpoint: an address nothing serves is worse than none.
pub async fn publish<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<ServiceStore>,
    key: &(String, String),
    identity: super::ApiServerIdentity,
    now: SystemTime,
) {
    if state.lock().kubernetes_api.contains_key(key) {
        return;
    }
    let tls = tls_server_config(&identity);
    if tls.is_none() {
        ::tracing::warn!(
            region = %key.0,
            cluster = %key.1,
            "a cluster's API server certificate could not be loaded, so its endpoint serves \
             no TLS"
        );
    }
    let auth = super::pod_identity_plane::auth_plane(engine);
    let listener = open(engine, state, key, auth).await;
    let authority = listener
        .as_ref()
        .map(::service_common::data_plane_addressing::listener_authority);
    let plane = KubernetesApiPlane {
        authority,
        objects: ClusterObjects::for_new_cluster(now),
        tls,
        _listener: listener,
    };
    let endpoint = plane.endpoint_url();
    let mut store = state.lock();
    // The cluster may have been deleted while the port was being bound; there is
    // then nothing to publish, and the listener closes as this returns.
    let Some(record) = store.clusters.get_mut(key) else {
        return;
    };
    record.endpoint = endpoint;
    store.kubernetes_api.insert(key.clone(), plane);
}

/// Open one cluster's listener, or `None` when this engine vends none, when there
/// is no runtime to bind a socket on, and when the socket cannot be bound at all.
async fn open<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<ServiceStore>,
    key: &(String, String),
    auth: Option<AuthPlane>,
) -> Option<::emulator_server::ResourceListener> {
    let listeners = shared_engine(engine)?.listeners();
    let handler = KubernetesApiSocket {
        state: state.clone(),
        key: key.clone(),
        auth,
    };
    // The runtime is asked for rather than assumed, and the bind runs on a task of
    // its OWN: a host driving this emulator in process may poll a request on
    // another executor, or on a runtime built with no IO driver, and neither can
    // open a socket.
    let runtime = ::tokio::runtime::Handle::try_current().ok()?;
    let opened = runtime
        .spawn(async move { listeners.tcp_plaintext(handler).await })
        .await;
    match opened {
        Ok(Ok(listener)) => Some(listener),
        Ok(Err(error)) => {
            ::tracing::warn!(
                %error,
                region = %key.0,
                cluster = %key.1,
                "a cluster's Kubernetes API address could not be opened"
            );
            None
        }
        Err(_cancelled) => None,
    }
}

/// The TLS server configuration this cluster's API server answers a handshake
/// with: the certificate its own certificate authority issued, the authority
/// behind it, and the leaf's key.
///
/// The provider is named explicitly rather than taken from the process default:
/// this process links more than one, and a configuration that has to work
/// whichever was installed cannot depend on which.
fn tls_server_config(identity: &super::ApiServerIdentity) -> Option<Arc<::rustls::ServerConfig>> {
    let chain: Vec<::rustls::pki_types::CertificateDer<'static>> = identity
        .certificate_chain_der()
        .into_iter()
        .map(::rustls::pki_types::CertificateDer::from)
        .collect();
    let key = ::rustls::pki_types::PrivateKeyDer::Pkcs8(
        ::rustls::pki_types::PrivatePkcs8KeyDer::from(identity.private_key_der()),
    );
    let provider = Arc::new(::rustls::crypto::aws_lc_rs::default_provider());
    let config = ::rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .ok()?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .ok()?;
    Some(Arc::new(config))
}

/// The shared engine behind a handler's `Eng`, which is what vends the core's
/// listeners.
fn shared_engine<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<&::engine::InMemoryEngine> {
    (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()
}

/// Serves ONE cluster's published Kubernetes API address.
///
/// Cheap to clone — both fields are a handle or a key — which is what the core's
/// `'static` per-connection handler contract asks for.
#[derive(Clone)]
struct KubernetesApiSocket {
    /// The ONE shared store the control plane writes, so every connection is
    /// served from the objects and the configuration that store holds.
    state: ::engine::ServiceState<ServiceStore>,
    /// The `(region, cluster name)` this socket serves.
    key: (String, String),
    /// The plane a token this cluster issues is minted on.
    auth: Option<AuthPlane>,
}

impl NativeHandler for KubernetesApiSocket {
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let socket = self.clone();
        Box::pin(async move { socket.serve_connection(conn).await })
    }
}

impl KubernetesApiSocket {
    /// Serve one accepted connection: negotiate what the client opened with, then
    /// speak HTTP/1.1 on it until the client closes it.
    async fn serve_connection(&self, conn: ListenerConn) {
        let Some(conn) = self.negotiated(conn).await else {
            return;
        };
        let io = ::hyper_util::rt::TokioIo::new(conn);
        let state = self.state.clone();
        let key = self.key.clone();
        let auth = self.auth.clone();
        let service = ::hyper::service::service_fn(move |request: ::http::Request<_>| {
            let state = state.clone();
            let key = key.clone();
            let auth = auth.clone();
            async move {
                // A panic anywhere behind this address is CONTAINED here and
                // answered as the protocol-shaped internal failure, exactly as the
                // endpoint a control call arrives on answers one: a single client
                // cannot take the connection, the listener or the process down with
                // it.
                let facts = ::emulator_server::RequestFacts::from_request(&request);
                Ok::<_, ::std::convert::Infallible>(
                    ::emulator_server::contain_panics(
                        facts,
                        answer(state, auth, key, request),
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
            // A connection the client dropped, or a request this emulator could
            // not frame, ends here and takes nothing else with it.
            ::tracing::debug!(
                %error,
                "a connection to a cluster's Kubernetes API address ended before its response"
            );
        }
    }

    /// Read what the client opened the connection with and hand back the stream
    /// the request is read from: a TLS record is handshaked under this cluster's
    /// own certificate, a request in the clear is read as it stands.
    ///
    /// `None` when there is nothing to serve on the connection: a client that sent
    /// nothing, a read that failed, a handshake that did not complete, and a
    /// cluster whose certificate could not be loaded.
    async fn negotiated(&self, mut conn: ListenerConn) -> Option<ListenerConn> {
        let mut first = [0u8; 1];
        match ::tokio::time::timeout(HANDSHAKE_DEADLINE, conn.read(&mut first)).await {
            Ok(Ok(1)) => {}
            _ => return None,
        }
        let prefixed: ListenerConn = Box::new(Prefixed::new(first.to_vec(), conn));
        if first[0] != TLS_RECORD_FIRST_BYTE {
            return Some(prefixed);
        }
        // Read under the store's lock and RELEASED before the handshake runs: a
        // handshake never holds this service's store.
        let tls = {
            let store = self.state.lock();
            store
                .kubernetes_api
                .get(&self.key)
                .and_then(|plane| plane.tls.clone())
        }?;
        let acceptor = ::tokio_rustls::TlsAcceptor::from(tls);
        match ::tokio::time::timeout(HANDSHAKE_DEADLINE, acceptor.accept(prefixed)).await {
            Ok(Ok(stream)) => Some(Box::new(stream) as ListenerConn),
            Ok(Err(error)) => {
                ::tracing::debug!(
                    %error,
                    "a TLS handshake on a cluster's Kubernetes API address was not completed"
                );
                None
            }
            Err(_) => None,
        }
    }
}

/// Answer one request to a cluster's Kubernetes API address.
///
/// The body is read under ONE in-flight permit and a byte ceiling, and the whole
/// answer under the request deadline, so what the bodies in flight hold at once is
/// the in-flight cap times that ceiling rather than the connection count times it.
async fn answer(
    state: ::engine::ServiceState<ServiceStore>,
    auth: Option<AuthPlane>,
    key: (String, String),
    request: ::http::Request<::hyper::body::Incoming>,
) -> ::http::Response<Full<Bytes>> {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    // The media type the client wrote its body in, which is what says how the
    // body is read ([`read_manifest`]).
    let content_type = request
        .headers()
        .get(::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let Ok(permit) = in_flight().try_acquire_owned() else {
        return rendered(&ApiRefusal {
            status: 503,
            reason: "ServiceUnavailable",
            message: "the server is currently unable to handle the request".to_string(),
            resource: None,
        });
    };
    let limited = Limited::new(request.into_body(), body_ceiling());
    let served = ::tokio::time::timeout(request_deadline(), async move {
        // Held for the whole read and dispatch, and released with it.
        let _permit: OwnedSemaphorePermit = permit;
        let Ok(collected) = limited.collect().await else {
            return rendered(&ApiRefusal {
                status: 413,
                reason: "RequestEntityTooLarge",
                message: "the request body is larger than this server reads".to_string(),
                resource: None,
            });
        };
        let body = collected.to_bytes();
        match serve(
            &state,
            auth.as_ref(),
            &key,
            method.as_str(),
            &path,
            &content_type,
            &body,
        ) {
            Ok((status, document)) => json_response(status, &document),
            Err(refusal) => rendered(&refusal),
        }
    })
    .await;
    served.unwrap_or_else(|_| {
        rendered(&ApiRefusal {
            status: 504,
            reason: "Timeout",
            message: "the request did not complete within the server's deadline".to_string(),
            resource: None,
        })
    })
}

/// Serve one Kubernetes API request against the cluster keyed `key`, answering the
/// HTTP status and the object document, or the `Status` the API server refuses it
/// with.
///
/// The ONE place a Kubernetes request is routed, so the published address and any
/// later in-process caller answer identically. Nothing but the objects decides an
/// answer; the only write is the object a create makes.
pub fn serve(
    state: &::engine::ServiceState<ServiceStore>,
    auth: Option<&AuthPlane>,
    key: &(String, String),
    method: &str,
    path: &str,
    content_type: &str,
    body: &[u8],
) -> Result<(u16, ::serde_json::Value), ApiRefusal> {
    let segments: Vec<&str> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let now = SystemTime::now();

    // The discovery documents a Kubernetes client reads before anything else.
    match segments.as_slice() {
        ["version"] => {
            return Ok((200, version_document(state, key)));
        }
        ["api"] => {
            return Ok((
                200,
                ::serde_json::json!({
                    "kind": "APIVersions",
                    "versions": [super::kubernetes_objects::CORE_API_VERSION],
                }),
            ));
        }
        ["apis"] => {
            return Ok((
                200,
                ::serde_json::json!({ "kind": "APIGroupList", "apiVersion": "v1", "groups": [] }),
            ));
        }
        _ => {}
    }

    let Some(rest) = segments.strip_prefix(&CORE_API_PREFIX[..]) else {
        return Err(ApiRefusal::no_such_path(path));
    };

    // The resource list of the core group, which is what a client reads to learn
    // which resources this API server serves and how each one is addressed. A
    // client that cannot read it addresses nothing, whatever the server holds.
    if rest.is_empty() {
        if method != ::http::Method::GET.as_str() {
            return Err(ApiRefusal::method_not_allowed(method, path));
        }
        return Ok((200, core_api_resource_list()));
    }

    // The cluster's NODES. They are not objects a client creates: they are the
    // instances this cluster's node groups launched, so they are READ from the
    // node groups the control plane holds ([`super::kubernetes_nodes`]).
    match (method, rest) {
        (method, ["nodes"]) if method == ::http::Method::GET.as_str() => {
            let store = state.lock();
            // A cluster this store no longer holds answers nothing, as every
            // other path on this address does.
            plane(&store, key)?;
            return Ok((
                200,
                ::serde_json::json!({
                    "kind": "NodeList",
                    "apiVersion": super::kubernetes_objects::CORE_API_VERSION,
                    "metadata": {},
                    "items": store.cluster_nodes(&key.0, &key.1),
                }),
            ));
        }
        (method, ["nodes", name]) if method == ::http::Method::GET.as_str() => {
            let store = state.lock();
            plane(&store, key)?;
            return match store.cluster_node(&key.0, &key.1, name) {
                Some(node) => Ok((200, node)),
                None => Err(ApiRefusal::not_found("nodes", name)),
            };
        }
        (method, ["nodes", ..]) => {
            return Err(ApiRefusal::method_not_allowed(method, path));
        }
        _ => {}
    }

    // The token subresource is the one request that reaches beyond this service's
    // own objects: the token is minted by the EKS Auth plane, which is what
    // answers for it afterwards.
    if let ["namespaces", namespace, "serviceaccounts", account, "token"] = rest {
        if method != ::http::Method::POST.as_str() {
            return Err(ApiRefusal::method_not_allowed(method, path));
        }
        let document = read_manifest(content_type, body)?;
        let plan = {
            let store = state.lock();
            plane(&store, key)?
                .objects
                .plan_token(namespace, account, &document, now)?
        };
        let token =
            super::pod_identity_plane::issue_service_account_token(auth, &key.1, &plan)
                .ok_or_else(|| {
                    ApiRefusal::internal("the service account token could not be issued")
                })?;
        return Ok((201, plan.to_response(&token)));
    }

    let mut store = state.lock();

    // The nodes of this cluster a pod can run on: the pod the request creates is
    // scheduled onto one, and so is any pod that was admitted while the cluster
    // had none. They are read from the control plane's own records before the
    // objects are borrowed to write them.
    let nodes = store.scheduler_nodes(&key.0, &key.1);

    let objects = &mut plane_mut(&mut store, key)?.objects;
    // The clock-owed moves of this cluster's objects, materialised before any of
    // them is reported: each was due at a stored instant whether or not anybody
    // called.
    objects.materialize_due_transitions(now, &nodes);
    match (method, rest) {
        (method, ["namespaces"]) if method == ::http::Method::POST.as_str() => {
            let document = read_manifest(content_type, body)?;
            Ok((201, objects.create_namespace(&document, now)?))
        }
        (method, ["namespaces"]) if method == ::http::Method::GET.as_str() => {
            let items: Vec<::serde_json::Value> = objects
                .namespaces
                .keys()
                .filter_map(|name| objects.namespace_json(name).ok())
                .collect();
            Ok((
                200,
                ::serde_json::json!({
                    "kind": "NamespaceList",
                    "apiVersion": super::kubernetes_objects::CORE_API_VERSION,
                    "metadata": {},
                    "items": items,
                }),
            ))
        }
        (method, ["namespaces", namespace]) if method == ::http::Method::GET.as_str() => {
            Ok((200, objects.namespace_json(namespace)?))
        }
        (method, ["namespaces", namespace, "pods"]) if method == ::http::Method::POST.as_str() => {
            let document = read_manifest(content_type, body)?;
            Ok((201, objects.create_pod(namespace, &document, now, &nodes)?))
        }
        (method, ["namespaces", namespace, "pods"]) if method == ::http::Method::GET.as_str() => {
            Ok((200, objects.pod_list_json(namespace)?))
        }
        (method, ["namespaces", namespace, "pods", name])
            if method == ::http::Method::GET.as_str() =>
        {
            Ok((200, objects.pod_json(namespace, name)?))
        }
        (method, ["namespaces", namespace, "serviceaccounts"])
            if method == ::http::Method::POST.as_str() =>
        {
            let document = read_manifest(content_type, body)?;
            Ok((
                201,
                objects.create_service_account(namespace, &document, now)?,
            ))
        }
        (method, ["namespaces", namespace, "serviceaccounts"])
            if method == ::http::Method::GET.as_str() =>
        {
            Ok((200, objects.service_account_list_json(namespace)?))
        }
        (method, ["namespaces", namespace, "serviceaccounts", name])
            if method == ::http::Method::GET.as_str() =>
        {
            Ok((200, objects.service_account_json(namespace, name)?))
        }
        // A path this API server does serve, asked for with a method that
        // addresses nothing, is told apart from a path it does not serve at all:
        // everything under a namespace is a resource it serves.
        (method, ["namespaces", ..]) => Err(ApiRefusal::method_not_allowed(method, path)),
        _ => Err(ApiRefusal::no_such_path(path)),
    }
}

/// The resource list of the core API group: every resource this API server serves
/// under `/api/v1`, the verbs it serves on each and whether the resource is
/// namespaced.
///
/// A Kubernetes client resolves the kind it was asked for against THIS document
/// before it addresses anything — a resource the list does not name is one the
/// client reports the server as not having — so the list names exactly the
/// resources the routing above serves, and no others.
fn core_api_resource_list() -> ::serde_json::Value {
    /// One row of the list: the resource's path, its kind, whether it lives in a
    /// namespace, the short name a client may address it by, and the verbs served.
    fn resource(
        name: &str,
        kind: &str,
        namespaced: bool,
        short_names: &[&str],
        verbs: &[&str],
    ) -> ::serde_json::Value {
        let mut row = ::serde_json::json!({
            "name": name,
            "singularName": kind.to_ascii_lowercase(),
            "namespaced": namespaced,
            "kind": kind,
            "verbs": verbs,
        });
        if !short_names.is_empty() {
            row["shortNames"] = ::serde_json::json!(short_names);
        }
        row
    }

    let read = ["get", "list"];
    let read_create = ["create", "get", "list"];
    ::serde_json::json!({
        "kind": "APIResourceList",
        "apiVersion": super::kubernetes_objects::CORE_API_VERSION,
        "groupVersion": super::kubernetes_objects::CORE_API_VERSION,
        "resources": [
            resource("namespaces", "Namespace", false, &["ns"], &read_create),
            resource("nodes", "Node", false, &["no"], &read),
            resource("pods", "Pod", true, &["po"], &read_create),
            resource(
                "serviceaccounts",
                "ServiceAccount",
                true,
                &["sa"],
                &read_create,
            ),
            // The one subresource this server serves, addressed under the service
            // account it mints a token for.
            resource(
                "serviceaccounts/token",
                "TokenRequest",
                true,
                &[],
                &["create"],
            ),
        ],
    })
}

/// The plane of the cluster keyed `key`, or the `Status` a request to a cluster
/// this store no longer holds is answered with — a cluster that has been deleted
/// while a client was dialling it.
fn plane<'store>(
    store: &'store ServiceStore,
    key: &(String, String),
) -> Result<&'store KubernetesApiPlane, ApiRefusal> {
    store
        .kubernetes_api
        .get(key)
        .ok_or_else(|| ApiRefusal::not_found("clusters", &key.1))
}

/// [`plane`], for the request that creates an object.
fn plane_mut<'store>(
    store: &'store mut ServiceStore,
    key: &(String, String),
) -> Result<&'store mut KubernetesApiPlane, ApiRefusal> {
    store
        .kubernetes_api
        .get_mut(key)
        .ok_or_else(|| ApiRefusal::not_found("clusters", &key.1))
}

/// The media types this API server reads a manifest in. A client states the one
/// it wrote the body in on `Content-Type`, and a body in any other media type is
/// one this server cannot read — the Kubernetes protobuf media type among them,
/// whose objects are encoded against the generated schema of each kind rather
/// than as a document.
pub const READ_MEDIA_TYPES: [&str; 1] = ["application/json"];

/// The manifest a write request carries, read as the JSON document a Kubernetes
/// client sends.
///
/// An empty body is the empty object, which is what a request that carries no
/// manifest sends. A body in a media type this server does not read is the API
/// server's own `415`, naming the media types it does; one in a media type it
/// does read but cannot parse is its `400`, reporting that the kind could not be
/// read out of it — as opposed to a manifest it read and found invalid, which is
/// the `422` above.
fn read_manifest(content_type: &str, body: &[u8]) -> Result<::serde_json::Value, ApiRefusal> {
    // A media type carries parameters (`application/json; charset=utf-8`); the
    // type itself is what says how the body is encoded.
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim();
    if !media_type.is_empty()
        && !READ_MEDIA_TYPES
            .iter()
            .any(|read| media_type.eq_ignore_ascii_case(read))
    {
        return Err(ApiRefusal::unsupported_media_type(&READ_MEDIA_TYPES));
    }
    if body.is_empty() {
        return Ok(::serde_json::json!({}));
    }
    match ::serde_json::from_slice::<::serde_json::Value>(body) {
        Ok(document @ ::serde_json::Value::Object(_)) => Ok(document),
        Ok(_) => Err(ApiRefusal::bad_request(
            "couldn't get version/kind; json parse error: the body is not a JSON object",
        )),
        Err(error) => Err(ApiRefusal::bad_request(format!(
            "couldn't get version/kind; json parse error: {error}"
        ))),
    }
}

/// The `/version` document, naming the Kubernetes version of the CLUSTER this plane
/// serves — the `version` its own record carries, which `DescribeCluster` reports, so the
/// two answers about one cluster cannot disagree.
///
/// A record that carries none (or a value that is not `<major>.<minor>`) is reported as
/// the version the cluster was created at by default
/// ([`super::DEFAULT_KUBERNETES_VERSION`]), which is what the control plane would have
/// stored.
fn version_document(
    state: &::engine::ServiceState<ServiceStore>,
    key: &(String, String),
) -> ::serde_json::Value {
    let version = {
        let store = state.lock();
        store
            .clusters
            .get(key)
            .and_then(|record| record.version.clone())
            .unwrap_or_else(|| super::DEFAULT_KUBERNETES_VERSION.to_string())
    };
    let mut parts = version.split('.');
    let major = parts.next().unwrap_or_default().to_string();
    let minor = parts.next().unwrap_or_default().to_string();
    ::serde_json::json!({
        "major": major,
        "minor": minor,
        "gitVersion": format!("v{version}"),
        "platform": "emulator",
    })
}

/// One object document as the HTTP answer a Kubernetes client reads.
fn json_response(status: u16, document: &::serde_json::Value) -> ::http::Response<Full<Bytes>> {
    let body = Bytes::from(document.to_string().into_bytes());
    ::http::Response::builder()
        .status(status)
        .header(::http::header::CONTENT_TYPE, "application/json")
        .body(Full::new(body.clone()))
        .unwrap_or_else(|_| ::http::Response::new(Full::new(body)))
}

/// One refusal as the HTTP answer a Kubernetes client reads: the status it names,
/// carrying its own `Status` object.
fn rendered(refusal: &ApiRefusal) -> ::http::Response<Full<Bytes>> {
    json_response(refusal.status, &refusal.to_status_json())
}
