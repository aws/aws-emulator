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
//! The cluster's own state and nothing else. A request is served the way an API
//! server serves one, in that order:
//!
//! 1. the credential the request presents names an IAM principal, and the cluster's
//!    own mapping for that principal — its ACCESS ENTRY, else the `aws-auth`
//!    ConfigMap of a cluster whose mode includes `CONFIG_MAP` — is the Kubernetes
//!    identity the request is served as ([`super::kubernetes_authentication`]); a
//!    principal neither mapping names authenticates as nobody;
//! 2. the cluster's RBAC objects and the access policies its entry associates decide
//!    whether that identity may do what the request addresses
//!    ([`super::kubernetes_rbac`]); one that may not is refused before anything is
//!    read or written;
//! 3. the objects ([`super::kubernetes_objects`]) answer it: every answer is computed
//!    from the namespaces, service accounts, pods and RBAC objects the cluster holds,
//!    and the one write a request makes is the object it creates. A projected service
//!    account token is minted by the EKS Auth plane
//!    ([`super::pod_identity_plane`]), because that plane is what answers for the
//!    token afterwards — the cluster's record of what it issued is what
//!    `AssumeRoleForPodIdentity` resolves a presented token against, so there is one
//!    registry rather than two;
//! 4. the request is RECORDED in the cluster's audit log
//!    ([`super::kubernetes_audit`]) while the `audit` control plane log type is
//!    enabled, naming the identity it was served as and the decision taken on it.
//!
//! Each of those steps takes this service's store for its own read and GIVES IT UP
//! again, deliberately: resolving the credential asks the other services' stores and
//! the audit record is delivered over the in-process callback, and neither may run
//! with this store held. The authorizer therefore decides on the state it read and the
//! routing re-reads it, which is what an API server does with two sequential reads
//! anyway.
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

use super::kubernetes_audit::{AuditDelivery, AuditedRequest, ClusterAuditLog};
use super::kubernetes_authentication::{self as authentication, ApiServerPrincipals};
use super::kubernetes_container_run;
use super::kubernetes_events::EVENT_FIELD_SELECTORS;
use super::kubernetes_image_pull;
use super::kubernetes_lists::ListRequest;
use super::kubernetes_objects::{
    NAMESPACED_FIELD_SELECTORS, NAMESPACE_FIELD_SELECTORS, POD_FIELD_SELECTORS,
    SERVICE_ACCOUNT_FIELD_SELECTORS,
};
use super::kubernetes_objects::{ApiRefusal, ClusterObjects};
use super::kubernetes_openapi;
use super::kubernetes_pod_addressing;
use super::kubernetes_protobuf;
use super::kubernetes_rbac::{self as rbac, RbacResource};
use super::kubernetes_self_subject_review as self_review;
use super::kubernetes_services::SERVICE_FIELD_SELECTORS;
use super::kubernetes_subject_access_review as access_review;
use super::kubernetes_version_info;
use super::kubernetes_workloads::{self as workloads, WorkloadResource};
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

/// The most bytes of ONE request a Kubernetes API server reads: the limit the
/// API server itself puts on a request body ("Request entity too large: limit is
/// 3145728"), under which every object it stores therefore fits.
///
/// It is what bounds the size of an object a create writes into this service's
/// store, beside the per-namespace counts the objects are bounded by
/// ([`super::kubernetes_objects::MAX_PODS_PER_NAMESPACE`] and its neighbours).
pub(super) const MAX_REQUEST_BYTES: usize = 3 * 1024 * 1024;

/// How many bytes of one request this address reads: the API server's own
/// request ceiling, never more than the core's per-connection ceiling.
fn body_ceiling() -> usize {
    usize::try_from(listener_caps::MAX_BYTES_PER_CONN)
        .unwrap_or(usize::MAX)
        .min(MAX_REQUEST_BYTES)
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
    /// The registries that answer which IAM principal a credential presented at
    /// this address speaks for.
    principals: ApiServerPrincipals,
    /// Where this cluster's audit records go, which is the cluster's own control
    /// plane log group. Every request is recorded there while the `audit` log type
    /// is enabled, and none while it is not.
    audit: Option<ClusterAuditLog>,
    /// The Amazon ECR registries the kubelet of a node of this cluster pulls a
    /// pod's images from, as the stores that decide a pull
    /// ([`kubernetes_image_pull::settle_due_pulls`]).
    registry: kubernetes_image_pull::PullRegistries,
    /// The Amazon EC2 records this cluster's networking plugin addresses its pods
    /// out of: a pod's address is a secondary private address of its node's own
    /// elastic network interface
    /// ([`kubernetes_pod_addressing::settle_due_pod_addresses`]).
    addressing: kubernetes_pod_addressing::PodAddressPlane,
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
    callback: ::smithy_server_runtime::EmbeddedEmulator,
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
        // The identities of the account, as the other services of this same
        // emulator hold them: a credential presented here is resolved against the
        // very records they wrote.
        principals: ApiServerPrincipals::of(engine),
        // The audit log of this cluster, reached over the callback into the emulator
        // the create is running within — the same seam every other cross-service
        // call of this service goes over.
        audit: Some(ClusterAuditLog::new(callback, &key.0, &key.1)),
        registry: kubernetes_image_pull::PullRegistries::of(engine),
        // The records the cluster's networking plugin takes a pod's address out
        // of, taken over the same seam and held for the same reason.
        addressing: kubernetes_pod_addressing::PodAddressPlane::of(engine),
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
    // The credential the request presents, the agent that sent it, and the media type
    // it wrote its body in: all three are read off the headers before the body,
    // because the answer is decided by who is asking and how they wrote it as much as
    // by what was asked.
    let header_value = |name: ::http::header::HeaderName| {
        request
            .headers()
            .get(&name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    let authorization = header_value(::http::header::AUTHORIZATION);
    let cluster_id = ::http::header::HeaderName::from_static(
        super::kubernetes_authentication::CLUSTER_ID_HEADER,
    );
    let cluster_id = header_value(cluster_id);
    let user_agent = header_value(::http::header::USER_AGENT);
    let accept = header_value(::http::header::ACCEPT);
    let query = request.uri().query().map(str::to_string);
    // The media type the client wrote its body in, which is what says how the
    // body is read ([`read_manifest`]).
    let content_type = header_value(::http::header::CONTENT_TYPE).unwrap_or_default();
    // The in-flight permit is WAITED for rather than demanded: an API server queues a
    // request that arrives while it is already serving its maximum and answers
    // `429 Too Many Requests` only when it could not take it at all, which is what
    // this answers when the wait runs out. The cap itself is the core's
    // ([`in_flight`]), so what the bodies in flight hold at once is unchanged.
    let acquired =
        ::tokio::time::timeout(request_deadline(), Arc::clone(&in_flight()).acquire_owned()).await;
    let Ok(Ok(permit)) = acquired else {
        return rendered(&ApiRefusal {
            status: 429,
            reason: "TooManyRequests",
            message: "Too many requests, please try again later.".to_string(),
            resource: None,
        });
    };
    let limited = Limited::new(request.into_body(), body_ceiling());
    let served = ::tokio::time::timeout(request_deadline(), async move {
        // Held for the whole read and dispatch, and released with it.
        let _permit: OwnedSemaphorePermit = permit;
        let Ok(collected) = limited.collect().await else {
            // The ceiling is the API server's own, so the refusal is the one the API
            // server answers a body past it with, naming that limit.
            return rendered(&ApiRefusal {
                status: 413,
                reason: "RequestEntityTooLarge",
                message: format!("Request entity too large: limit is {}", body_ceiling()),
                resource: None,
            });
        };
        let body = collected.to_bytes();
        // The containers the clock already owes their start: once the kubelet has the
        // images, it creates the containers and runs the programs they declare
        // ([`kubernetes_container_run`]). It is settled HERE rather than inside `serve`
        // because performing a caller's own program is synchronous, CPU-bound work: it
        // goes to the blocking pool, so this worker is released while a program runs and
        // the deadline around this block still has an await point to land on. Every run
        // it settles was due at a stored instant whether or not anybody called.
        kubernetes_container_run::settle_due_runs(&state, &key, SystemTime::now()).await;
        let served = serve(
            &state,
            auth.as_ref(),
            &key,
            &ApiRequest {
                method: method.as_str(),
                path: &path,
                authorization: authorization.as_deref(),
                cluster_id: cluster_id.as_deref(),
                user_agent: user_agent.as_deref(),
                query: query.as_deref(),
                accept: accept.as_deref(),
                content_type: &content_type,
                body: &body,
            },
        );
        // The audit record goes out once the answer is decided and this service's
        // store is no longer held, which is why it is delivered here rather than
        // inside the routing.
        if let Some(delivery) = served.delivery {
            delivery.deliver().await;
        }
        match served.body {
            Some(negotiated) => {
                media_response(served.status, negotiated.content_type, negotiated.bytes)
            }
            None => json_response(served.status, &served.document),
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

/// One request to a cluster's Kubernetes API address, reduced to what deciding it
/// needs: what it addresses, the credential it presents, the agent that sent it, and
/// the media type its body is written in.
///
/// Carried as one value rather than as a parameter each, because every step of the
/// decision reads some of it and an API request is one thing, not six.
#[derive(Debug, Clone, Copy)]
pub struct ApiRequest<'request> {
    pub method: &'request str,
    pub path: &'request str,
    /// The `Authorization` header, which is where the credential travels.
    pub authorization: Option<&'request str>,
    /// The `x-k8s-aws-id` header, which is where a client names the CLUSTER its
    /// bearer token was built for
    /// ([`super::kubernetes_authentication::CLUSTER_ID_HEADER`]).
    pub cluster_id: Option<&'request str>,
    /// The `User-Agent`, which the audit record names.
    pub user_agent: Option<&'request str>,
    /// The query string, which is where a list states the chunk it asks for.
    pub query: Option<&'request str>,
    /// The `Accept` header: the encodings the client can read, which is what the
    /// published OpenAPI documents are negotiated by
    /// ([`super::kubernetes_openapi::negotiated`]).
    pub accept: Option<&'request str>,
    /// The media type the client wrote its body in, which is what says how the body is
    /// read.
    pub content_type: &'request str,
    pub body: &'request [u8],
}

/// One answered Kubernetes API request: what the client reads, and the record the
/// cluster's audit log owes for it.
#[derive(Debug)]
pub struct Served {
    pub status: u16,
    pub document: ::serde_json::Value,
    /// The same answer written in the encoding the client NEGOTIATED, for the one
    /// surface of this API that is negotiated — the published OpenAPI documents
    /// ([`kubernetes_openapi::negotiated_body`]) — and `None` for every answer
    /// written in the JSON a Kubernetes client reads an object in.
    pub body: Option<NegotiatedBody>,
    /// The audit record to deliver once this service's store is released, and `None`
    /// for a cluster that does not have the `audit` log type enabled.
    pub delivery: Option<AuditDelivery>,
}

/// One answer written in an encoding the client chose: the content type it is
/// written with, and the bytes of it.
#[derive(Debug)]
pub struct NegotiatedBody {
    pub content_type: &'static str,
    pub bytes: Vec<u8>,
}

/// Serve one Kubernetes API request against the cluster keyed `key`.
///
/// The ONE place a Kubernetes request is served, so the published address and any
/// later in-process caller answer identically. It is the API server's own sequence:
/// the credential names an IAM principal, the cluster's own mapping for that
/// principal — its access entry, else its `aws-auth` ConfigMap — is the Kubernetes
/// identity the request is served as ([`authentication::authenticate`]), the RBAC
/// objects and access policies decide whether that identity may do what the request
/// addresses ([`rbac::admits`]), the request is routed only then, and the whole thing
/// is recorded in the cluster's audit log.
///
/// Nothing but the cluster's own state decides an answer; the only write is the
/// object a create makes.
pub fn serve(
    state: &::engine::ServiceState<ServiceStore>,
    auth: Option<&AuthPlane>,
    key: &(String, String),
    request: &ApiRequest<'_>,
) -> Served {
    let received_at = SystemTime::now();
    let attributes = rbac::request_attributes(request.method, request.path);
    // The pulls the clock already owes this cluster's pods: the kubelet of the node
    // that took a pod fetches the images its containers run, which the registry each
    // reference names decides ([`kubernetes_image_pull`]). It runs here, before the
    // request is routed, because the verdict is taken with no store of this service
    // held — and every pull it settles was due at a stored instant whether or not
    // anybody called.
    let registry = {
        let store = state.lock();
        store
            .kubernetes_api
            .get(key)
            .map(|plane| (plane.registry.clone(), plane.addressing.clone()))
    };
    if let Some((registry, addressing)) = registry {
        kubernetes_image_pull::settle_due_pulls(state, &registry, key, received_at);
        // The addresses the cluster's networking plugin owes the pods it has
        // placed, and the ones it has to give back: taken on the node's own
        // elastic network interface, with no store of this service held, for the
        // same reason the pulls above are
        // ([`kubernetes_pod_addressing::settle_due_pod_addresses`]).
        kubernetes_pod_addressing::settle_due_pod_addresses(state, &addressing, key);
    }
    // The plane's own handles, read and RELEASED: resolving the credential asks the
    // other services' stores, which is never done with this one's lock held.
    let plane_handles = {
        let store = state.lock();
        store
            .kubernetes_api
            .get(key)
            .map(|plane| (plane.principals.clone(), plane.audit.clone()))
    };
    let Some((principals, audit)) = plane_handles else {
        // The cluster has been deleted while a client was dialling it: there is no
        // API server behind this address any more, and no audit log to record in.
        let refusal = ApiRefusal::not_found("clusters", &key.1);
        return Served {
            status: refusal.status,
            document: refusal.to_status_json(),
            body: None,
            delivery: None,
        };
    };
    // WHO the request speaks for: the credential its bearer token was built from,
    // resolved against the registries the other services of this emulator wrote.
    //
    // A token is bound to ONE cluster — the plugin signs the cluster's name into the
    // presigned request it carries, and a client that names the cluster beside the
    // token sends it under the same name — so a token built for ANOTHER cluster
    // speaks for nobody here, however good the credential behind it is.
    let caller = authentication::presented_token(request.authorization).and_then(|presented| {
        let names_another_cluster = presented
            .cluster_name
            .as_deref()
            .or(request.cluster_id)
            .is_some_and(|named| named != key.1);
        if names_another_cluster {
            return None;
        }
        principals.caller(&presented.access_key_id, received_at)
    });

    let authenticated = match caller {
        Some(caller) => {
            let store = state.lock();
            authentication::authenticate(&store, key, &caller)
        }
        // A credential this server cannot read, and one naming a session that has run
        // out, both speak for nobody.
        None => Err(ApiRefusal::unauthorized()),
    };
    let (identity, admitted, answer) = match authenticated {
        Err(refusal) => (None, false, Err(refusal)),
        Ok(identity) => {
            // The decision, or the `Status` a cluster this store no longer holds is
            // answered with — the same refusal the handles read above answers, so a
            // cluster deleted at either instant is answered alike rather than as a
            // request the authorizer refused.
            //
            // What the decision COSTS is bounded by the store it walks rather than by
            // the request: at most
            // [`rbac::MAX_RBAC_OBJECTS_PER_SCOPE`] bindings of a scope, each with at
            // most [`rbac::MAX_SUBJECTS_PER_BINDING`] subjects, resolving to a role of
            // at most [`rbac::MAX_RULES_PER_ROLE`] rules whose lists hold at most
            // [`rbac::MAX_ENTRIES_PER_RULE`] entries each — every one of them refused
            // at the create (`rbac::stored_objects`) rather than trimmed here. A
            // subject is matched against the identity's groups through the SET those
            // groups are held in (`rbac::SubjectIdentity::groups`), so however many
            // groups the access entry was created with they are walked once when the
            // identity is built and never multiply the product above. The walk matches
            // strings and indexes nothing, which is also why this short read may hold
            // the store: it cannot grow with what a caller sends.
            let decided = {
                let store = state.lock();
                plane(&store, key)
                    .map(|plane| rbac::admits(&plane.objects, &identity, &attributes))
            };
            match decided {
                Ok(true) => {
                    let answer = route(state, auth, key, request, &identity);
                    (Some(identity), true, answer)
                }
                Ok(false) => {
                    let refusal = attributes.forbidden(&identity.username);
                    (Some(identity), false, Err(refusal))
                }
                Err(refusal) => (Some(identity), false, Err(refusal)),
            }
        }
    };
    let (status, document) = match answer {
        Ok((status, document)) => (status, document),
        Err(refusal) => (refusal.status, refusal.to_status_json()),
    };
    let answered_at = SystemTime::now();
    // A create names its object in the manifest rather than in the path, and a
    // `generateName` is named by the server; the object the answer carries is the one
    // the request really acted on, so that is the name the record names.
    let answered_object_name = document
        .get("metadata")
        .and_then(|metadata| metadata.get("name"))
        .and_then(::serde_json::Value::as_str)
        .map(str::to_string);
    let delivery = audit.and_then(|log| {
        let store = state.lock();
        log.delivery(
            &store,
            key,
            &AuditedRequest {
                identity,
                attributes,
                answered_object_name,
                status,
                admitted,
                user_agent: request.user_agent.map(str::to_string),
                received_at,
                answered_at,
            },
        )
    });
    // The one surface of this API whose encoding the client chooses: a published
    // OpenAPI document the request asked for in another encoding is written in that
    // encoding rather than in JSON.
    let body = kubernetes_openapi::negotiated_body(request.path, request.accept, status, &document)
        .map(|(content_type, bytes)| NegotiatedBody {
            content_type,
            bytes,
        });
    Served {
        status,
        document,
        body,
        delivery,
    }
}

/// Route one request the authorizer admitted to the objects that answer it: the HTTP
/// status and the document, or the `Status` the API server refuses it with.
///
/// `identity` is who the request is served as, which one resource answers WITH:
/// a `SelfSubjectReview` is the user information the API server has about the
/// client that asked ([`super::kubernetes_self_subject_review`]).
fn route(
    state: &::engine::ServiceState<ServiceStore>,
    auth: Option<&AuthPlane>,
    key: &(String, String),
    request: &ApiRequest<'_>,
    identity: &rbac::SubjectIdentity,
) -> Result<(u16, ::serde_json::Value), ApiRefusal> {
    let ApiRequest {
        method,
        path,
        query,
        content_type,
        body,
        ..
    } = *request;
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
                ::serde_json::json!({
                    "kind": "APIGroupList",
                    "apiVersion": super::kubernetes_objects::CORE_API_VERSION,
                    "groups": [
                        workloads::api_group_entry(),
                        rbac::api_group_entry(),
                        self_review::api_group_entry(),
                        access_review::api_group_entry(),
                    ],
                }),
            ));
        }
        // The OpenAPI documents a client validates a manifest against before it
        // sends one ([`super::kubernetes_openapi`]). A client that cannot read them
        // sends nothing, whatever this server serves.
        [segment, version] if *segment == kubernetes_openapi::OPENAPI_V3_SEGMENT => {
            if method != ::http::Method::GET.as_str() {
                return Err(ApiRefusal::method_not_allowed(method, path));
            }
            // A path under the OpenAPI segment that addresses no published document
            // is not found, whatever the client can read.
            let Some(published) = kubernetes_openapi::published_at(path) else {
                return Err(ApiRefusal::no_such_path(path));
            };
            unacceptable(published, request.accept)?;
            let kubernetes_version = cluster_kubernetes_version(state, key);
            return match *version {
                kubernetes_openapi::OPENAPI_V3_VERSION => {
                    Ok((200, kubernetes_openapi::v3_index(&kubernetes_version)))
                }
                kubernetes_openapi::OPENAPI_V2_VERSION => {
                    Ok((200, kubernetes_openapi::v2_document(&kubernetes_version)))
                }
                // A version `published_at` publishes a document for and this
                // routing has none for is not found rather than answered with
                // another version's document.
                _ => Err(ApiRefusal::no_such_path(path)),
            };
        }
        [segment, version, document @ ..]
            if *segment == kubernetes_openapi::OPENAPI_V3_SEGMENT
                && *version == kubernetes_openapi::OPENAPI_V3_VERSION =>
        {
            if method != ::http::Method::GET.as_str() {
                return Err(ApiRefusal::method_not_allowed(method, path));
            }
            unacceptable(kubernetes_openapi::V3_MEDIA_TYPES, request.accept)?;
            let kubernetes_version = cluster_kubernetes_version(state, key);
            return kubernetes_openapi::v3_document(&kubernetes_version, &document.join("/"))
                .map(|document| (200, document))
                .ok_or_else(|| ApiRefusal::no_such_path(path));
        }
        // The discovery documents of the groups beside the core API this server
        // serves: a client reads them before it addresses a group at all.
        ["apis", group] if *group == workloads::APPS_API_GROUP => {
            return Ok((200, workloads::api_group_document()));
        }
        ["apis", group, version]
            if *group == workloads::APPS_API_GROUP
                && *version == workloads::APPS_GROUP_VERSION =>
        {
            return Ok((200, workloads::api_resource_list_document()));
        }
        ["apis", group, version, rest @ ..]
            if *group == workloads::APPS_API_GROUP
                && *version == workloads::APPS_GROUP_VERSION =>
        {
            return route_apps(state, key, request, rest, now);
        }
        ["apis", group] if *group == rbac::RBAC_API_GROUP => {
            return Ok((200, rbac::api_group_document()));
        }
        ["apis", group, version]
            if *group == rbac::RBAC_API_GROUP && *version == rbac::RBAC_GROUP_VERSION =>
        {
            return Ok((200, rbac::api_resource_list_document()));
        }
        ["apis", group, version, rest @ ..]
            if *group == rbac::RBAC_API_GROUP && *version == rbac::RBAC_GROUP_VERSION =>
        {
            return route_rbac(state, key, request, rest, now);
        }
        ["apis", group] if *group == self_review::AUTHENTICATION_API_GROUP => {
            return Ok((200, self_review::api_group_document()));
        }
        ["apis", group, version]
            if *group == self_review::AUTHENTICATION_API_GROUP
                && *version == self_review::AUTHENTICATION_GROUP_VERSION =>
        {
            return Ok((200, self_review::api_resource_list_document()));
        }
        // A client asks the API server which identity it is being served as by
        // CREATING a self subject review, and the answer is that identity.
        ["apis", group, version, resource]
            if *group == self_review::AUTHENTICATION_API_GROUP
                && *version == self_review::AUTHENTICATION_GROUP_VERSION
                && *resource == self_review::SELF_SUBJECT_REVIEWS_RESOURCE =>
        {
            if method != ::http::Method::POST.as_str() {
                return Err(ApiRefusal::method_not_allowed(method, path));
            }
            // The body is read as a manifest like every other create's, so a body
            // this server cannot read is refused the way one is anywhere else;
            // nothing in it decides the answer.
            read_manifest(content_type, body)?;
            let store = state.lock();
            plane(&store, key)?;
            return Ok((201, self_review::self_subject_review(identity)));
        }
        ["apis", group] if *group == access_review::AUTHORIZATION_API_GROUP => {
            return Ok((200, access_review::api_group_document()));
        }
        ["apis", group, version]
            if *group == access_review::AUTHORIZATION_API_GROUP
                && *version == access_review::AUTHORIZATION_GROUP_VERSION =>
        {
            return Ok((200, access_review::api_resource_list_document()));
        }
        // A client asks the API server whether it MAY do something by creating a
        // self subject access review, and the answer is the decision the same
        // authorizer takes on the request it describes.
        ["apis", group, version, resource]
            if *group == access_review::AUTHORIZATION_API_GROUP
                && *version == access_review::AUTHORIZATION_GROUP_VERSION
                && *resource == access_review::SELF_SUBJECT_ACCESS_REVIEWS_RESOURCE =>
        {
            if method != ::http::Method::POST.as_str() {
                return Err(ApiRefusal::method_not_allowed(method, path));
            }
            let review = read_manifest(content_type, body)?;
            let reviewed = access_review::reviewed_attributes(&review)
                .ok_or_else(|| ApiRefusal::invalid(access_review::NO_ATTRIBUTES_MESSAGE))?;
            let store = state.lock();
            let objects = &plane(&store, key)?.objects;
            return Ok((
                201,
                access_review::self_subject_access_review(
                    objects, identity, &review, &reviewed,
                ),
            ));
        }
        // And it asks WHAT it may do in a namespace by creating a self subject
        // rules review, whose answer is the rules the same two sources of
        // permissions put in force for it there.
        ["apis", group, version, resource]
            if *group == access_review::AUTHORIZATION_API_GROUP
                && *version == access_review::AUTHORIZATION_GROUP_VERSION
                && *resource == access_review::SELF_SUBJECT_RULES_REVIEWS_RESOURCE =>
        {
            if method != ::http::Method::POST.as_str() {
                return Err(ApiRefusal::method_not_allowed(method, path));
            }
            let review = read_manifest(content_type, body)?;
            let namespace = access_review::reviewed_namespace(&review)
                .ok_or_else(|| ApiRefusal::bad_request(access_review::NO_NAMESPACE_MESSAGE))?
                .to_string();
            let store = state.lock();
            // Whether the authorizer Amazon EKS serves this cluster's access
            // entries by is in its chain, which is what decides whether the
            // listing can be the whole of what the identity may do.
            let access_entry_authorizer = store
                .clusters
                .get(key)
                .is_some_and(super::ClusterRecord::serves_access_entries);
            let objects = &plane(&store, key)?.objects;
            return Ok((
                201,
                access_review::self_subject_rules_review(
                    objects,
                    identity,
                    &review,
                    &namespace,
                    access_entry_authorizer,
                ),
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
            // The collection is chunked and label-selected like every other one
            // this server lists ([`super::kubernetes_lists`]), which is how a
            // client asks for the nodes of ONE node group.
            let nodes = store.cluster_nodes(&key.0, &key.1);
            let names: Vec<String> = nodes
                .iter()
                .filter_map(|node| {
                    node.get("metadata")
                        .and_then(|metadata| metadata.get("name"))
                        .and_then(::serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .collect();
            let listing = selected(
                ListRequest::from_query(query)?,
                super::kubernetes_nodes::NODE_FIELD_SELECTORS,
            )?;
            return Ok((
                200,
                super::kubernetes_lists::listed("NodeList", &names, &listing, |name| {
                    nodes
                        .iter()
                        .find(|node| {
                            node.get("metadata")
                                .and_then(|metadata| metadata.get("name"))
                                .and_then(::serde_json::Value::as_str)
                                == Some(name)
                        })
                        .cloned()
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

    // Everywhere this cluster can run a pod: the pod the request creates is placed
    // on it, and so is any pod that was admitted while the cluster had nowhere to
    // run it. It is read from the control plane's own records before the objects
    // are borrowed to write them.
    let capacity = settled(&mut store, key, now)?;
    // The block this cluster allocates its Service addresses out of, read from the
    // record the create wrote ([`super::kubernetes_services`]) — so an address a
    // Service reports is one inside the CIDR `DescribeCluster` publishes.
    let network = store.service_network(&key.0, &key.1);

    let objects = &mut plane_mut(&mut store, key)?.objects;
    // The chunk a list asks for, read once: every collection below answers one
    // chunk of itself ([`super::kubernetes_lists`]).
    let list_request = ListRequest::from_query(query);
    match (method, rest) {
        (method, ["namespaces"]) if method == ::http::Method::POST.as_str() => {
            let document = read_manifest(content_type, body)?;
            Ok((201, objects.create_namespace(&document, now)?))
        }
        (method, ["namespaces"]) if method == ::http::Method::GET.as_str() => {
            Ok((
                200,
                objects.namespace_list_json(&selected(list_request?, NAMESPACE_FIELD_SELECTORS)?),
            ))
        }
        (method, ["namespaces", namespace]) if method == ::http::Method::GET.as_str() => {
            Ok((200, objects.namespace_json(namespace)?))
        }
        (method, ["namespaces", namespace, "pods"]) if method == ::http::Method::POST.as_str() => {
            let document = read_manifest(content_type, body)?;
            Ok((201, objects.create_pod(namespace, &document, now, &capacity)?))
        }
        (method, ["namespaces", namespace, "pods"]) if method == ::http::Method::GET.as_str() => {
            Ok((
                200,
                objects.pod_list_json(
                    namespace,
                    &selected(list_request?, POD_FIELD_SELECTORS)?,
                )?,
            ))
        }
        (method, ["namespaces", namespace, "pods", name])
            if method == ::http::Method::GET.as_str() =>
        {
            Ok((200, objects.pod_json(namespace, name)?))
        }
        (method, ["namespaces", namespace, "configmaps"])
            if method == ::http::Method::POST.as_str() =>
        {
            let document = read_manifest(content_type, body)?;
            Ok((201, objects.create_config_map(namespace, &document, now)?))
        }
        (method, ["namespaces", namespace, "configmaps"])
            if method == ::http::Method::GET.as_str() =>
        {
            Ok((
                200,
                objects.config_map_list_json(
                    namespace,
                    &selected(list_request?, &NAMESPACED_FIELD_SELECTORS)?,
                )?,
            ))
        }
        (method, ["namespaces", namespace, "configmaps", name])
            if method == ::http::Method::GET.as_str() =>
        {
            Ok((200, objects.config_map_json(namespace, name)?))
        }
        (method, ["namespaces", namespace, "configmaps", name])
            if method == ::http::Method::PUT.as_str() =>
        {
            let document = read_manifest(content_type, body)?;
            Ok((
                200,
                objects.replace_config_map(namespace, name, &document)?,
            ))
        }
        // The SERVICES of a namespace: the objects that name a workload inside the
        // cluster, each on an address allocated out of the cluster's own service
        // CIDR ([`super::kubernetes_services`]).
        (method, ["namespaces", namespace, "services"])
            if method == ::http::Method::POST.as_str() =>
        {
            let document = read_manifest(content_type, body)?;
            Ok((
                201,
                objects.create_service(namespace, &document, now, &network)?,
            ))
        }
        (method, ["namespaces", namespace, "services"])
            if method == ::http::Method::GET.as_str() =>
        {
            Ok((
                200,
                objects.service_list_json(
                    namespace,
                    &selected(list_request?, SERVICE_FIELD_SELECTORS)?,
                    &network,
                )?,
            ))
        }
        (method, ["namespaces", namespace, "services", name])
            if method == ::http::Method::GET.as_str() =>
        {
            Ok((200, objects.service_json(namespace, name, &network)?))
        }
        (method, ["namespaces", namespace, "services", name])
            if method == ::http::Method::DELETE.as_str() =>
        {
            Ok((200, objects.delete_service(namespace, name)?))
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
            Ok((
                200,
                objects.service_account_list_json(
                    namespace,
                    &selected(list_request?, SERVICE_ACCOUNT_FIELD_SELECTORS)?,
                )?,
            ))
        }
        (method, ["namespaces", namespace, "serviceaccounts", name])
            if method == ::http::Method::GET.as_str() =>
        {
            Ok((200, objects.service_account_json(namespace, name)?))
        }
        // The EVENTS of a namespace: what the cluster's own components recorded
        // about its objects ([`super::kubernetes_events`]). They are not objects a
        // client creates — the cluster records them — so the collection is read.
        (method, ["namespaces", namespace, "events"]) if method == ::http::Method::GET.as_str() => {
            Ok((
                200,
                objects.event_list_json(
                    namespace,
                    &selected(list_request?, EVENT_FIELD_SELECTORS)?,
                )?,
            ))
        }
        (method, ["namespaces", namespace, "events", name])
            if method == ::http::Method::GET.as_str() =>
        {
            Ok((200, objects.event_json(namespace, name)?))
        }
        // A path this API server does serve, asked for with a method that
        // addresses nothing, is told apart from a path it does not serve at all:
        // everything under a namespace is a resource it serves.
        (method, ["namespaces", ..]) => Err(ApiRefusal::method_not_allowed(method, path)),
        _ => Err(ApiRefusal::no_such_path(path)),
    }
}

/// Settle everything this cluster is owed before any of its objects is reported,
/// and hand back everywhere a pod of it can run.
///
/// Two kinds of work are owed here, and both were owed at this instant whether or
/// not anybody called: the clock-owed moves of the cluster's own objects (a pending
/// pod placed on capacity that has appeared, a pod whose containers were owed their
/// start), and the workloads its installed add-ons own
/// ([`super::kubernetes_workloads::managed_workloads`]) — one per add-on the
/// control plane reports as installed, converged against the nodes the cluster
/// holds. So two callers read the same cluster, and a read changes nothing a later
/// caller would not have seen.
fn settled(
    store: &mut ServiceStore,
    key: &(String, String),
    now: SystemTime,
) -> Result<super::kubernetes_fargate::ClusterCapacity, ApiRefusal> {
    let capacity = store.cluster_capacity(&key.0, &key.1);
    let managed = workloads::managed_workloads(store, &key.0, &key.1, now);
    let network = store.service_network(&key.0, &key.1);
    let objects = &mut plane_mut(store, key)?.objects;
    objects.materialize_due_transitions(now, &capacity);
    objects.materialize_managed_workloads(&managed, now);
    objects.reconcile_workloads(now, &capacity);
    // The Service the cluster's own API server is reached by inside the cluster,
    // which every Kubernetes cluster holds from the instant it was created
    // ([`super::kubernetes_services`]).
    objects.materialize_api_server_service(&network, now);
    // The capacity is re-read: placing the cluster's pods may have provisioned
    // Fargate capacity for them, which is capacity the cluster now holds.
    Ok(store.cluster_capacity(&key.0, &key.1))
}

/// Route one request to `apps/v1`: the workload resources the group serves, each of
/// them in a namespace.
///
/// `rest` is the path under the group version, so a workload is addressed as
/// `["namespaces", ns, resource]` or `["namespaces", ns, resource, name]`. A
/// workload resource addressed outside a namespace is a path the group does not
/// serve, because both its resources are namespaced.
fn route_apps(
    state: &::engine::ServiceState<ServiceStore>,
    key: &(String, String),
    request: &ApiRequest<'_>,
    rest: &[&str],
    now: SystemTime,
) -> Result<(u16, ::serde_json::Value), ApiRefusal> {
    let (method, path, body) = (request.method, request.path, request.body);
    let (namespace, addressed) = match rest {
        ["namespaces", namespace, addressed @ ..] if !addressed.is_empty() => {
            (*namespace, addressed)
        }
        _ => return Err(ApiRefusal::no_such_path(path)),
    };
    let (Some(resource), name) = (
        addressed
            .first()
            .and_then(|segment| WorkloadResource::from_segment(segment)),
        addressed.get(1).copied(),
    ) else {
        return Err(ApiRefusal::no_such_path(path));
    };
    if addressed.len() > 2 {
        return Err(ApiRefusal::no_such_path(path));
    }
    let mut store = state.lock();
    let capacity = settled(&mut store, key, now)?;
    let list_request = ListRequest::from_query(request.query);
    let objects = &mut plane_mut(&mut store, key)?.objects;
    match (method, name) {
        (method, None) if method == ::http::Method::POST.as_str() => {
            let manifest = read_manifest(request.content_type, body)?;
            Ok((
                201,
                objects.create_workload(resource, namespace, &manifest, now, &capacity)?,
            ))
        }
        (method, None) if method == ::http::Method::GET.as_str() => Ok((
            200,
            objects.workload_list_json(
                resource,
                namespace,
                &selected(list_request?, &NAMESPACED_FIELD_SELECTORS)?,
                &capacity,
            )?,
        )),
        (method, Some(name)) if method == ::http::Method::GET.as_str() => Ok((
            200,
            objects.workload_json(resource, namespace, name, &capacity)?,
        )),
        (method, Some(name)) if method == ::http::Method::PUT.as_str() => {
            let manifest = read_manifest(request.content_type, body)?;
            Ok((
                200,
                objects.replace_workload(resource, namespace, name, &manifest, now, &capacity)?,
            ))
        }
        (method, Some(name)) if method == ::http::Method::DELETE.as_str() => Ok((
            200,
            objects.delete_workload(resource, namespace, name, &capacity)?,
        )),
        (method, _) => Err(ApiRefusal::method_not_allowed(method, path)),
    }
}

/// Route one request to `rbac.authorization.k8s.io/v1`: the four resources the group
/// serves, namespaced or cluster-scoped as each of them is.
///
/// `rest` is the path under the group version, so `["namespaces", ns, resource, ..]`
/// addresses a namespaced object and `[resource, ..]` a cluster-scoped one. A
/// resource addressed in the scope it does not belong to — a `Role` without a
/// namespace, a `ClusterRole` under one — is a path the group does not serve.
fn route_rbac(
    state: &::engine::ServiceState<ServiceStore>,
    key: &(String, String),
    request: &ApiRequest<'_>,
    rest: &[&str],
    now: SystemTime,
) -> Result<(u16, ::serde_json::Value), ApiRefusal> {
    let (method, path, body) = (request.method, request.path, request.body);
    let (namespace, addressed) = match rest {
        ["namespaces", namespace, addressed @ ..] if !addressed.is_empty() => {
            (Some(*namespace), addressed)
        }
        _ => (None, rest),
    };
    let (Some(resource), name) = (
        addressed.first().and_then(|segment| RbacResource::from_segment(segment)),
        addressed.get(1).copied(),
    ) else {
        return Err(ApiRefusal::no_such_path(path));
    };
    if addressed.len() > 2 || resource.is_namespaced() != namespace.is_some() {
        return Err(ApiRefusal::no_such_path(path));
    }
    let mut store = state.lock();
    let objects = &mut plane_mut(&mut store, key)?.objects;
    // What a write RETAINS is bounded by the creating call itself, before anything is
    // stored: `rbac::MAX_RBAC_OBJECTS_PER_SCOPE` objects of one resource per scope,
    // `rbac::MAX_RULES_PER_ROLE` rules, `rbac::MAX_SUBJECTS_PER_BINDING` subjects and
    // `rbac::MAX_ENTRIES_PER_RULE` entries per rule list — each refused with the
    // validation `Status` past the bound by `rbac::stored_objects`, which these calls
    // enter.
    match (method, name) {
        (method, None) if method == ::http::Method::POST.as_str() => {
            let manifest = read_manifest(request.content_type, body)?;
            Ok((
                201,
                objects.create_rbac_object(resource, namespace, &manifest, now)?,
            ))
        }
        (method, None) if method == ::http::Method::GET.as_str() => {
            Ok((200, objects.rbac_list_json(resource, namespace)?))
        }
        (method, Some(name)) if method == ::http::Method::GET.as_str() => {
            Ok((200, objects.rbac_object_json(resource, namespace, name)?))
        }
        (method, Some(name)) if method == ::http::Method::PUT.as_str() => {
            let manifest = read_manifest(request.content_type, body)?;
            Ok((
                200,
                objects.replace_rbac_object(resource, namespace, name, &manifest)?,
            ))
        }
        (method, Some(name)) if method == ::http::Method::DELETE.as_str() => {
            Ok((200, objects.delete_rbac_object(resource, namespace, name)?))
        }
        (method, _) => Err(ApiRefusal::method_not_allowed(method, path)),
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
    // A Service is also DELETED through this server: its address goes back to the
    // cluster's block when it is ([`super::kubernetes_services`]).
    let read_create_delete = ["create", "delete", "get", "list"];
    // A ConfigMap is also UPDATED through this server: the mapping a cluster
    // authenticates through is kept in one (`super::kubernetes_aws_auth`), and a
    // client maintains it by replacing the object.
    let read_create_update = ["create", "get", "list", "update"];
    ::serde_json::json!({
        "kind": "APIResourceList",
        "apiVersion": super::kubernetes_objects::CORE_API_VERSION,
        "groupVersion": super::kubernetes_objects::CORE_API_VERSION,
        "resources": [
            resource("namespaces", "Namespace", false, &["ns"], &read_create),
            resource("nodes", "Node", false, &["no"], &read),
            resource("pods", "Pod", true, &["po"], &read_create),
            resource("configmaps", "ConfigMap", true, &["cm"], &read_create_update),
            resource("services", "Service", true, &["svc"], &read_create_delete),
            // The events the cluster's own components record about its objects:
            // a client READS them, so the row names the verbs this server serves.
            resource("events", "Event", true, &["ev"], &read),
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

/// The media type a client states when it wrote the body as a JSON document,
/// which is also the one a request that states none is read as.
pub const JSON_MEDIA_TYPE: &str = "application/json";

/// The media type a client states when it wrote the body as a YAML document.
pub const YAML_MEDIA_TYPE: &str = "application/yaml";

/// The media types this API server reads a manifest in: a client states the one it
/// wrote the body in on `Content-Type`, and a body in any other media type is one
/// this server cannot read.
///
/// The three encodings a Kubernetes object travels in — the JSON document, the
/// YAML document that is the same object written as YAML, and the Kubernetes
/// protobuf encoding, in which the object is the bytes of its own generated
/// message behind the envelope that names its kind
/// ([`super::kubernetes_protobuf`]).
pub const READ_MEDIA_TYPES: [&str; 3] = [
    JSON_MEDIA_TYPE,
    YAML_MEDIA_TYPE,
    kubernetes_protobuf::PROTOBUF_MEDIA_TYPE,
];

/// How deeply a manifest written in YAML may nest before this server stops
/// reading it.
///
/// CHOSEN, not published: the bound is on the loader rather than on the object,
/// because a YAML loader builds the whole tree before anything inspects it. It is
/// far above what any Kubernetes manifest nests, and it is the one pre-parse
/// nesting bound this tree states
/// ([`::service_common::yaml_nesting::nesting_depth_bound`]), the same reading the
/// other YAML a caller hands this service is bounded by.
const MAX_MANIFEST_DEPTH: usize = 64;

/// The manifest a write request carries, read out of the encoding the client
/// stated it wrote the body in.
///
/// An empty body is the empty object, which is what a request that carries no
/// manifest sends. A client that states no media type at all is read as the JSON
/// one, which is the encoding a Kubernetes client writes by default. A body in a
/// media type this server does not read is the API server's own `415`, naming the
/// media types it does; one in a media type it does read but cannot decode an
/// object out of is its `400`, reporting that the kind could not be read out of it
/// — as opposed to a manifest it read and found invalid, which is the `422` above.
fn read_manifest(content_type: &str, body: &[u8]) -> Result<::serde_json::Value, ApiRefusal> {
    // A media type carries parameters (`application/json; charset=utf-8`); the
    // type itself is what says how the body is encoded.
    let stated = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim();
    let media_type = if stated.is_empty() {
        JSON_MEDIA_TYPE
    } else {
        stated
    };
    let Some(read_as) = READ_MEDIA_TYPES
        .into_iter()
        .find(|read| media_type.eq_ignore_ascii_case(read))
    else {
        return Err(ApiRefusal::unsupported_media_type(&READ_MEDIA_TYPES));
    };
    if body.is_empty() {
        return Ok(::serde_json::json!({}));
    }
    match read_as {
        kubernetes_protobuf::PROTOBUF_MEDIA_TYPE => kubernetes_protobuf::read_object(body),
        YAML_MEDIA_TYPE => read_yaml_manifest(body),
        _ => match ::serde_json::from_slice::<::serde_json::Value>(body) {
            Ok(document @ ::serde_json::Value::Object(_)) => Ok(document),
            Ok(_) => Err(ApiRefusal::bad_request(
                "couldn't get version/kind; json parse error: the body is not a JSON object",
            )),
            Err(error) => Err(ApiRefusal::bad_request(format!(
                "couldn't get version/kind; json parse error: {error}"
            ))),
        },
    }
}

/// The manifest a body written as a YAML document carries, as the same object
/// document the JSON form of it reads as — YAML's scalars, mappings and sequences
/// are JSON's ([`super::addon_catalog::yaml_as_json`], the one reading of
/// caller-supplied YAML this service has).
///
/// BOUNDED before the loader is handed the text, as every other reading of
/// caller-supplied YAML here is: a document past [`MAX_MANIFEST_DEPTH`] and one
/// that references a YAML anchor — which a loader resolves by COPYING the anchored
/// node into every alias position, so a shallow document can expand
/// multiplicatively while it is being loaded — are bodies this server does not
/// read at all.
fn read_yaml_manifest(body: &[u8]) -> Result<::serde_json::Value, ApiRefusal> {
    let unreadable = |detail: &str| {
        ApiRefusal::bad_request(format!(
            "couldn't get version/kind; yaml parse error: {detail}"
        ))
    };
    let text = ::std::str::from_utf8(body).map_err(|_| unreadable("the body is not text"))?;
    if ::service_common::yaml_nesting::nesting_depth_bound(text, MAX_MANIFEST_DEPTH)
        > MAX_MANIFEST_DEPTH
    {
        return Err(unreadable("the document nests deeper than the server reads"));
    }
    if super::addon_catalog::references_an_anchor(text) {
        return Err(unreadable("the document references a YAML anchor"));
    }
    let documents = ::yaml_rust2::YamlLoader::load_from_str(text)
        .map_err(|error| unreadable(&error.to_string()))?;
    // A stream with nothing in it (comments alone) carries no manifest, which is
    // what a body that carries none is.
    let Some(first) = documents.first() else {
        return Ok(::serde_json::json!({}));
    };
    match super::addon_catalog::yaml_as_json(first, MAX_MANIFEST_DEPTH) {
        document @ ::serde_json::Value::Object(_) => Ok(document),
        _ => Err(unreadable("the document is not a mapping")),
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
///
/// The patch level and the release day are the published calendar's for that version
/// — the same values `DescribeClusterVersions` reports — so the build this document
/// names is the build of the version the control plane says the cluster runs
/// ([`super::kubernetes_version_info`]).
fn version_document(
    state: &::engine::ServiceState<ServiceStore>,
    key: &(String, String),
) -> ::serde_json::Value {
    let (version, patch_version, released_at) = {
        let store = state.lock();
        let version = store
            .clusters
            .get(key)
            .and_then(|record| record.version.clone())
            .unwrap_or_else(|| super::DEFAULT_KUBERNETES_VERSION.to_string());
        let row = store
            .cluster_versions
            .iter()
            .find(|row| row.cluster_version == version);
        let patch_version = row.map_or_else(
            // A version the published calendar does not carry still reports a
            // patch-level build, at the initial patch level of its own minor
            // version — the level the calendar publishes every version it carries
            // at.
            || format!("{version}.0"),
            super::ClusterVersionRecord::kubernetes_patch_version,
        );
        (version, patch_version, row.map(|row| row.release_date))
    };
    kubernetes_version_info::version_document(&version, &patch_version, released_at)
}

/// `Ok(())` when `accept` admits one of the encodings the document the request
/// addresses is `published` in, else the `Status` the API server refuses an
/// unacceptable `Accept` with: a client that can read only an encoding this server
/// does not write is told so, naming the encodings that document IS published in,
/// rather than handed a body it cannot decode ([`kubernetes_openapi::negotiated`]).
fn unacceptable(
    published: &'static [&'static str],
    accept: Option<&str>,
) -> Result<(), ApiRefusal> {
    if kubernetes_openapi::negotiated(accept, published).is_some() {
        return Ok(());
    }
    Err(ApiRefusal {
        status: 406,
        reason: "NotAcceptable",
        message: kubernetes_openapi::unacceptable_message(published),
        resource: None,
    })
}

/// One list request held to the fields the collection it addresses is SELECTABLE by
/// ([`ListRequest::fields_limited_to`]): a `fieldSelector` naming a field that
/// collection's registry does not index is refused rather than answered with a list
/// narrowed by a field nothing indexes.
fn selected(
    mut listing: ListRequest,
    served: &[(&str, &str)],
) -> Result<ListRequest, ApiRefusal> {
    listing.fields_limited_to(served)?;
    Ok(listing)
}

/// The Kubernetes version the cluster keyed `key` runs, which every document this
/// server publishes about itself reports — the record's own version, so the version
/// a client reads from the API and from `DescribeCluster` is one value.
fn cluster_kubernetes_version(
    state: &::engine::ServiceState<ServiceStore>,
    key: &(String, String),
) -> String {
    let store = state.lock();
    store
        .clusters
        .get(key)
        .and_then(|record| record.version.clone())
        .unwrap_or_else(|| super::DEFAULT_KUBERNETES_VERSION.to_string())
}

/// One object document as the HTTP answer a Kubernetes client reads.
fn json_response(status: u16, document: &::serde_json::Value) -> ::http::Response<Full<Bytes>> {
    media_response(
        status,
        kubernetes_openapi::DOCUMENT_MEDIA_TYPE,
        document.to_string().into_bytes(),
    )
}

/// One answer as the HTTP response a client reads it from, stating the media type it
/// is written in — which is what tells a client that negotiated an encoding that it
/// got the one it asked for.
fn media_response(
    status: u16,
    media_type: &str,
    bytes: Vec<u8>,
) -> ::http::Response<Full<Bytes>> {
    let body = Bytes::from(bytes);
    ::http::Response::builder()
        .status(status)
        .header(::http::header::CONTENT_TYPE, media_type)
        .body(Full::new(body.clone()))
        .unwrap_or_else(|_| ::http::Response::new(Full::new(body)))
}

/// One refusal as the HTTP answer a Kubernetes client reads: the status it names,
/// carrying its own `Status` object.
fn rendered(refusal: &ApiRefusal) -> ::http::Response<Full<Bytes>> {
    json_response(refusal.status, &refusal.to_status_json())
}
