// SPDX-License-Identifier: CC0-1.0

//! The cluster's KUBERNETES API, end to end over the address the cluster itself
//! publishes.
//!
//! The RULE under test: a value this service publishes for a client to DIAL is an
//! address this emulator answers on, and what answers there is the API the member
//! documents. `Cluster$endpoint` is "the endpoint for your Kubernetes API server"
//! and `Certificate$data` is "the Base64-encoded certificate data required to
//! communicate with your cluster. Add this to the `certificate-authority-data`
//! section of the kubeconfig file for your cluster" — so a client that does the
//! documented thing (build a kubeconfig out of the pair, trust nothing but that
//! certificate, and talk to the cluster) reaches a Kubernetes API server, creates
//! a Pod object, and mints a `TokenRequest` bound to it. The token that comes back
//! is then exchangeable at `eks-auth`'s `AssumeRoleForPodIdentity` for the
//! credentials of the role the cluster's pod identity association binds that
//! service account to, which is the only token class that operation accepts.
//!
//! Every suite here drives that over a TABLE whose rows differ along what the rule
//! generalises over: the region and the name of the cluster (so the address is
//! shown to be per cluster rather than per service), and — for the token — the
//! namespace, the service account, whether the client created them or used the
//! ones Kubernetes creates with a cluster, the lifetime it asked for (absent,
//! below the server's floor, in range, past its ceiling) and whether the request
//! bound the token to a pod at all.
//!
//! The client here is deliberately a RAW one: a TCP connection, the TLS the
//! published certificate authority is the only trust anchor for, and an HTTP/1.1
//! request written by hand — so nothing but what the emulator published is used to
//! reach it, exactly as a kubeconfig-built client would.

use std::sync::Arc;
use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::kubernetes_objects;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_state;
use smithy_server_runtime::MultiServiceRouter;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::certificate_authority::{create_cluster, describe_cluster, str_member, REGION};
use crate::pod_identity_associations::{
    association_id_of, create_association, delete_association,
};
use crate::wire::emulator_with_engine;

/// The IAM role an association in this suite binds a service account to — a
/// well-formed role ARN of the account this emulator serves, which is what
/// `CreatePodIdentityAssociation` requires of the member.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/PodIdentityRole";

/// The audience an EKS Pod Identity token is projected for, as the EKS Auth
/// service documents it ("This value is always pods.eks.amazonaws.com").
pub(crate) const PODS_AUDIENCE: &str = "pods.eks.amazonaws.com";

/// The sentence `AssumeRoleForPodIdentity` refuses a token whose `(namespace, service
/// account)` identity the cluster holds no pod identity association for with — one
/// fixed sentence, naming nothing of the request.
///
/// Here rather than in each suite that drives an exchange, so the two cannot assert
/// different wording for one refusal.
pub(crate) const NO_ASSOCIATION_REFUSAL: &str =
    "The token included in the request has no service account role association for it.";

/// The sentence `AssumeRoleForPodIdentity` refuses an exchange whose association names a
/// TARGET role that role's own trust policy does not let the association's role assume —
/// one fixed sentence, naming nothing of the request.
///
/// Beside [`NO_ASSOCIATION_REFUSAL`] for the same reason: one refusal, one wording.
pub(crate) const TARGET_ROLE_REFUSAL: &str =
    "The target role of the EKS Pod Identity association cannot be assumed by the \
     association's role with the association's external ID.";

/// One answer from a cluster's Kubernetes API address.
#[derive(Debug)]
pub(crate) struct ApiAnswer {
    pub(crate) status: u16,
    pub(crate) body: serde_json::Value,
}

/// The emulator under test: the EKS control plane and the EKS Auth plane over ONE
/// shared engine, which is how a token a cluster's API server issues is
/// exchangeable at `AssumeRoleForPodIdentity`. The engine handle comes back with it,
/// so a suite can read the SESSION the exchange vended where every service resolves
/// a temporary credential's principal.
pub(crate) fn emulator_with_pod_identity() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (mut router, engine) = emulator_with_engine();
    router.register_service(
        service_com_amazonaws_eksauth_eksauthfrontend::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// What a client reads out of `DescribeCluster` to build a kubeconfig: the address
/// the cluster publishes for its API server, and the certificate authority it
/// publishes beside it.
#[derive(Debug, Clone)]
pub(crate) struct Kubeconfig {
    pub(crate) endpoint: String,
    pub(crate) certificate_authority_pem: String,
}

/// Create the cluster `name` in `region`, wait for the clock-owed provisioning to be
/// MATERIALISED (the cluster `ACTIVE` and its API server address published), and read
/// the kubeconfig members back the way a client does.
///
/// The wait is for the CONDITION, re-read until it holds or a generous deadline passes,
/// so a loaded host only makes it slower rather than failing it.
pub(crate) async fn provisioned_cluster(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> Kubeconfig {
    let created = create_cluster(router, region, name).await;
    assert_eq!(
        created.status, 200,
        "cluster {name:?}: expected 200 from CreateCluster, got {created:?}"
    );
    let described = poll_until_active(router, region, name).await;
    kubeconfig_of(name, &described)
}

/// The kubeconfig members of the cluster `name`, read out of one `DescribeCluster`
/// answer the way a client does: the address it published for its API server, and
/// the certificate authority published beside it, decoded as the member documents
/// ("the Base64-encoded certificate data required to communicate with your
/// cluster").
///
/// Separate from [`provisioned_cluster`] so a suite that creates several clusters
/// before waiting for any of them reads the pair the same way.
pub(crate) fn kubeconfig_of(name: &str, described: &crate::wire::Answer) -> Kubeconfig {
    let endpoint = str_member(name, "cluster.endpoint", &described.cluster()["endpoint"]);
    let data = str_member(
        name,
        "cluster.certificateAuthority.data",
        &described.cluster()["certificateAuthority"]["data"],
    );
    let decoded = aws_smithy_types::base64::decode(&data)
        .unwrap_or_else(|error| panic!("cluster {name:?}: the CA data must be Base64: {error}"));
    let certificate_authority_pem = String::from_utf8(decoded)
        .unwrap_or_else(|error| panic!("cluster {name:?}: the CA data must be text: {error}"));
    Kubeconfig {
        endpoint,
        certificate_authority_pem,
    }
}

/// How long a suite waits for a clock-owed move the emulator owes in
/// [`CLUSTER_ACTIVATION_DELAY`]: far above that span, so the deadline is reached only
/// when the move is not coming at all.
const PROVISIONING_DEADLINE: Duration = Duration::from_secs(60);

/// How often the condition is looked at again while waiting for it.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// `DescribeCluster` until the cluster reports `ACTIVE` with its API server address
/// published, or the deadline passes.
pub(crate) async fn poll_until_active(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> crate::wire::Answer {
    let deadline = std::time::Instant::now() + PROVISIONING_DEADLINE;
    loop {
        let described = describe_cluster(router, region, name).await;
        assert_eq!(
            described.status, 200,
            "cluster {name:?}: expected 200 from DescribeCluster, got {described:?}"
        );
        let active = described.cluster()["status"] == serde_json::json!("ACTIVE")
            && described.cluster()["endpoint"].is_string();
        if active {
            return described;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "cluster {name:?}: the clock owes it ACTIVE with a published endpoint, got \
             {described:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One request to a cluster's published Kubernetes API address, as a
/// kubeconfig-built client makes it: TLS over the endpoint's own host and port,
/// verified against the published certificate authority and NOTHING else, then one
/// HTTP/1.1 exchange on it.
pub(crate) async fn dial(
    config: &Kubeconfig,
    case: &str,
    method: &str,
    path: &str,
    body: Option<&serde_json::Value>,
) -> ApiAnswer {
    dial_as(config, case, method, path, "application/json", body).await
}

/// The same request, with the client stating the media type it wrote the body in
/// — what a client that writes its body in another encoding sends.
pub(crate) async fn dial_as(
    config: &Kubeconfig,
    case: &str,
    method: &str,
    path: &str,
    content_type: &str,
    body: Option<&serde_json::Value>,
) -> ApiAnswer {
    dial_with(
        config,
        case,
        method,
        path,
        content_type,
        Some(&account_bearer_token()),
        body,
    )
    .await
}

/// The bearer token a kubeconfig's credential plugin builds for the credential
/// `access_key_id`: `k8s-aws-v1.` followed by the base64url form of a presigned
/// `sts:GetCallerIdentity` request, whose SigV4 credential scope names that key.
///
/// The base64url form is written out here (the two alphabet substitutions and the
/// dropped padding of RFC 7515 §2) rather than taken from the crate's own codec,
/// so the token this client presents is built the way a credential plugin builds
/// one rather than by the code that reads it.
pub(crate) fn bearer_token(access_key_id: &str) -> String {
    token_of(&presigned_caller_identity(access_key_id, None))
}

/// The same token, BOUND to the cluster `cluster`: the plugin names the cluster the
/// token is built for in the presigned request, which is what makes a token usable
/// at one cluster.
pub(crate) fn bearer_token_for_cluster(access_key_id: &str, cluster: &str) -> String {
    token_of(&presigned_caller_identity(access_key_id, Some(cluster)))
}

/// The presigned `sts:GetCallerIdentity` request a token carries, scoped to
/// `access_key_id` and naming `cluster` when the plugin bound one into it.
fn presigned_caller_identity(access_key_id: &str, cluster: Option<&str>) -> String {
    let bound = cluster
        .map(|cluster| format!("&x-k8s-aws-id={cluster}"))
        .unwrap_or_default();
    format!(
        "https://sts.{REGION}.amazonaws.com/?Action=GetCallerIdentity&Version=2011-06-15\
         &X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential={access_key_id}%2F20260101%2F\
         {REGION}%2Fsts%2Faws4_request&X-Amz-Date=20260101T000000Z&X-Amz-Expires=60\
         &X-Amz-SignedHeaders=host%3Bx-k8s-aws-id&X-Amz-Signature=0000{bound}"
    )
}

/// `url` as the token a client presents it in.
fn token_of(url: &str) -> String {
    let encoded = aws_smithy_types::base64::encode(url.as_bytes())
        .replace('+', "-")
        .replace('/', "_")
        .replace('=', "");
    format!("Bearer k8s-aws-v1.{encoded}")
}

/// The token a client built from the ACCOUNT's own published credential presents,
/// which is the caller every dialler here is unless it says otherwise.
///
/// A Kubernetes API server authenticates every request from the bearer token it
/// carries, so a client of one presents a token; a request with none is anonymous
/// and is refused ([`crate::kubernetes_anonymous_requests`]).
pub(crate) fn account_bearer_token() -> String {
    bearer_token(crate::wire::DEFAULT_ACCESS_KEY_ID)
}

/// The same request again, PRESENTING a credential: the `Authorization` header a
/// kubeconfig's credential plugin sets, which is what tells the API server which
/// IAM principal the request speaks for. `None` presents none, which is the
/// account's own published credential — the caller every other dialler here is.
pub(crate) async fn dial_with(
    config: &Kubeconfig,
    case: &str,
    method: &str,
    path: &str,
    content_type: &str,
    authorization: Option<&str>,
    body: Option<&serde_json::Value>,
) -> ApiAnswer {
    let payload = body.map(serde_json::Value::to_string).unwrap_or_default();
    dial_bytes(
        config,
        case,
        method,
        path,
        content_type,
        authorization,
        payload.as_bytes(),
    )
    .await
}

/// The TLS connection a kubeconfig-built client opens to `endpoint`, trusting
/// NOTHING but the certificates `trusted_pem` carries — and the error the handshake
/// was refused with instead.
///
/// The ONE place this test crate builds that client, so every suite dials a published
/// endpoint the same way: the trust anchors are exactly the published authority, the
/// address dialled is the emulator's own loopback interface on the port the endpoint
/// names, and the server name presented is the host it names. The failure is ANSWERED
/// rather than panicked, because a suite that asks whether the authority of one
/// cluster verifies the endpoint of another needs the handshake that must be refused
/// (`crate::kubeconfig_authority_is_per_cluster`); [`dial_bytes`] unwraps it, naming
/// the row that dialled.
pub(crate) async fn tls_connection(
    endpoint: &str,
    trusted_pem: &str,
) -> Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>, String> {
    let authority = endpoint
        .strip_prefix("https://")
        .ok_or_else(|| format!("the endpoint must be an https URL, got {endpoint:?}"))?
        .to_string();
    let (host, port) = authority
        .rsplit_once(':')
        .ok_or_else(|| format!("the endpoint must carry a port, got {authority:?}"))?;
    let port: u16 = port
        .parse()
        .map_err(|error| format!("the endpoint's port must be a port, got {port:?}: {error}"))?;
    let mut roots = rustls::RootCertStore::empty();
    for certificate in rustls_pemfile::certs(&mut trusted_pem.as_bytes()) {
        let certificate = certificate
            .map_err(|error| format!("the trusted authority must be PEM certificates: {error}"))?;
        roots.add(certificate).map_err(|error| {
            format!("the trusted authority must load as a trust anchor: {error}")
        })?;
    }
    let client_config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| format!("the TLS client must configure: {error}"))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));
    // The emulator's OWN listener, on the port it published. The address is a loopback
    // literal here rather than the published host, so this crate dials nothing else.
    let stream = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|error| format!("the published address {authority:?} must accept: {error}"))?;
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|error| format!("{host:?} must be a server name: {error}"))?;
    connector
        .connect(server_name, stream)
        .await
        .map_err(|error| error.to_string())
}

/// The same request once more, with the body written as the BYTES the client
/// encoded it to — what a client that writes its object in an encoding other than
/// a JSON document sends.
pub(crate) async fn dial_bytes(
    config: &Kubeconfig,
    case: &str,
    method: &str,
    path: &str,
    content_type: &str,
    authorization: Option<&str>,
    payload: &[u8],
) -> ApiAnswer {
    dial_bytes_with_headers(
        config,
        case,
        method,
        path,
        content_type,
        authorization,
        &[],
        payload,
    )
    .await
}

/// The same request once more, carrying the extra `headers` the client set beside
/// the ones every request carries — what a client that names the identity it asks to
/// be served as sends (`Impersonate-User` and the `Impersonate-Group` headers beside
/// it).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn dial_bytes_with_headers(
    config: &Kubeconfig,
    case: &str,
    method: &str,
    path: &str,
    content_type: &str,
    authorization: Option<&str>,
    headers: &[(&str, &str)],
    payload: &[u8],
) -> ApiAnswer {
    let authority = config
        .endpoint
        .strip_prefix("https://")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the endpoint must be an https URL, got {:?}",
                config.endpoint
            )
        })
        .to_string();
    let (host, _port) = authority.rsplit_once(':').unwrap_or_else(|| {
        panic!("case {case:?}: the endpoint must carry a port, got {authority:?}")
    });
    assert!(
        matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1"),
        "case {case:?}: the published endpoint must be the emulator's own loopback \
         address, got {host:?}"
    );

    // The ONLY trust anchor is the certificate authority the cluster published, so
    // a handshake succeeds here exactly when the certificate served at the endpoint
    // was issued by it ([`tls_connection`]).
    let mut tls = tls_connection(&config.endpoint, &config.certificate_authority_pem)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the certificate served at {authority:?} must verify against the \
                 published certificate authority: {error}"
            )
        });

    let credential = authorization
        .map(|header| format!("Authorization: {header}\r\n"))
        .unwrap_or_default();
    let extra: String = headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {authority}\r\n{credential}{extra}Content-Type: \
         {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    let mut request = head.into_bytes();
    request.extend_from_slice(payload);
    tls.write_all(&request)
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the request must be written: {error}"));
    let mut raw = Vec::new();
    tls.read_to_end(&mut raw)
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the answer must be read: {error}"));
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, answered) = text
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("case {case:?}: expected an HTTP answer, got {text:?}"));
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("case {case:?}: expected a status line, got {head:?}"));
    ApiAnswer {
        status,
        body: serde_json::from_str(answered).unwrap_or(serde_json::Value::Null),
    }
}

/// The claim set of a JSON Web Token, read the way any client reads one: the
/// middle segment, base64url-decoded.
fn token_claims(case: &str, token: &str) -> serde_json::Value {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "case {case:?}: a projected token is three segments, got {token:?}"
    );
    let mut standard = segments[1].replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    let decoded = aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|error| panic!("case {case:?}: the claim segment must be base64url: {error}"));
    serde_json::from_slice(&decoded)
        .unwrap_or_else(|error| panic!("case {case:?}: the claim set must be JSON: {error}"))
}

/// `POST /clusters/{cluster}/assume-role-for-pod-identity` on the EKS Auth plane of
/// the same emulator, as the EKS Pod Identity Agent calls it: the `host` header names
/// that service's own signing name, which is the routing key, and the `Authorization`
/// header names the credential the request speaks for.
///
/// It answers the crate's own [`crate::wire::Answer`], so a suite can read the error
/// SHAPE and the refusal sentence of a refused exchange as well as its status.
pub(crate) async fn assume_role_for_pod_identity(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    token: &str,
) -> crate::wire::Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/clusters/{cluster}/assume-role-for-pod-identity"))
        // The routing key: the endpoint host names the `eks-auth` signing name.
        .header("host", format!("eks-auth.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={}/20260101/{region}/eks-auth/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef",
                crate::wire::DEFAULT_ACCESS_KEY_ID
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::json!({ "token": token }).to_string(),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body: serde_json::Value = serde_json::from_slice(response.body()).unwrap_or_default();
    let error_type = error_type.or_else(|| {
        body.get("__type")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    });
    crate::wire::Answer {
        status,
        error_type,
        body,
    }
}

// ---------------------------------------------------------------------------
// RULE: the endpoint a cluster publishes is an address this emulator serves the
// Kubernetes API on, under a certificate the certificate authority published
// beside it issued.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_published_endpoint_is_served_under_the_published_certificate_authority() {
    let (router, _engine) = emulator_with_pod_identity();
    // The rows differ along what the rule generalises over: the region the cluster
    // was created in, the form of its name, and two clusters of the same name in
    // different regions — each of which is a separate cluster and so a separate
    // address.
    let clusters = [
        ("a cluster in the suite's region", REGION, "k8s-api-one"),
        ("a second cluster beside it", REGION, "k8s_api_two"),
        ("a cluster of the same name elsewhere", "eu-central-1", "k8s-api-one"),
    ];
    let mut addresses: Vec<String> = Vec::new();
    for (case, region, name) in clusters {
        let config = provisioned_cluster(&router, region, name).await;
        let authority = config
            .endpoint
            .strip_prefix("https://")
            .unwrap_or_default()
            .to_string();
        assert!(
            authority
                .rsplit_once(':')
                .is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok()),
            "case {case:?}: the endpoint must name the host and port it is served at, got {:?}",
            config.endpoint
        );
        assert!(
            !authority.ends_with("amazonaws.com"),
            "case {case:?}: the endpoint must be an address this emulator serves, not a name \
             under AWS's own zone, got {:?}",
            config.endpoint
        );
        // The handshake inside `dial` is the assertion that the published CA
        // verifies the published endpoint: it trusts nothing else.
        let version = dial(&config, case, "GET", "/version", None).await;
        assert_eq!(
            version.status, 200,
            "case {case:?}: the published endpoint must serve the Kubernetes API, got {version:?}"
        );
        assert!(
            version.body["major"].is_string(),
            "case {case:?}: expected the API server's version document, got {:?}",
            version.body
        );
        addresses.push(authority);
    }
    let mut distinct = addresses.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        addresses.len(),
        "each cluster publishes an address of its own, got {addresses:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: a path the API server does not serve, and an object it does not hold, are
// answered with the Kubernetes `Status` object for that failure — never with a
// success.
// ---------------------------------------------------------------------------

/// One row of the refusal table: the request made to the published endpoint, and the
/// `Status` the API server answers it with.
struct RefusalCase {
    label: &'static str,
    method: &'static str,
    path: &'static str,
    /// The manifest the request carries, for the rows that write.
    body: Option<serde_json::Value>,
    status: u16,
    /// The machine-readable `reason` of the `Status` object.
    reason: &'static str,
}

#[tokio::test]
async fn the_api_server_refuses_what_it_does_not_hold() {
    let (router, _engine) = emulator_with_pod_identity();
    let config = provisioned_cluster(&router, REGION, "k8s-refusals").await;

    // The rows span the refusals this API serves: a read of each kind of object
    // that was never created, a path outside the API's own space, a method the
    // addressed resource does not serve, a pod naming a service account that does
    // not exist, and a token bound to a pod that does not exist.
    let cases = [
        RefusalCase {
            label: "a pod that was never created",
            method: "GET",
            path: "/api/v1/namespaces/default/pods/absent",
            body: None,
            status: 404,
            reason: "NotFound",
        },
        RefusalCase {
            label: "a namespace that was never created",
            method: "GET",
            path: "/api/v1/namespaces/absent",
            body: None,
            status: 404,
            reason: "NotFound",
        },
        RefusalCase {
            label: "a service account that was never created",
            method: "GET",
            path: "/api/v1/namespaces/default/serviceaccounts/absent",
            body: None,
            status: 404,
            reason: "NotFound",
        },
        RefusalCase {
            label: "a path outside the API's own space",
            method: "GET",
            path: "/healthz/etcd",
            body: None,
            status: 404,
            reason: "NotFound",
        },
        RefusalCase {
            label: "a method the addressed resource does not serve",
            method: "DELETE",
            path: "/api/v1/namespaces/default/pods",
            body: None,
            status: 405,
            reason: "MethodNotAllowed",
        },
        RefusalCase {
            label: "a pod naming a service account that does not exist",
            method: "POST",
            path: "/api/v1/namespaces/default/pods",
            body: Some(serde_json::json!({
                "metadata": { "name": "orphan" },
                "spec": { "serviceAccountName": "no-such-account" },
            })),
            status: 404,
            reason: "NotFound",
        },
        RefusalCase {
            label: "a manifest that names nothing",
            method: "POST",
            path: "/api/v1/namespaces/default/pods",
            body: Some(serde_json::json!({ "spec": {} })),
            status: 422,
            reason: "Invalid",
        },
    ];
    for case in &cases {
        let label = case.label;
        let answered = dial(&config, label, case.method, case.path, case.body.as_ref()).await;
        assert_eq!(
            answered.status, case.status,
            "case {label:?}: expected HTTP {}, got {answered:?}",
            case.status
        );
        assert_eq!(
            answered.body["kind"],
            serde_json::json!("Status"),
            "case {label:?}: a refusal is a Kubernetes Status object, got {:?}",
            answered.body
        );
        assert_eq!(
            answered.body["reason"],
            serde_json::json!(case.reason),
            "case {label:?}: expected the reason {:?}, got {:?}",
            case.reason,
            answered.body
        );
    }

    // A token bound to a pod the cluster does not hold binds to nothing, so it is
    // not issued.
    let unbound = dial(
        &config,
        "a token bound to a pod that does not exist",
        "POST",
        "/api/v1/namespaces/default/serviceaccounts/default/token",
        Some(&serde_json::json!({
            "spec": {
                "audiences": [PODS_AUDIENCE],
                "boundObjectRef": { "kind": "Pod", "name": "absent" },
            }
        })),
    )
    .await;
    assert_eq!(
        unbound.status, 404,
        "a token bound to an absent pod must not be issued, got {unbound:?}"
    );

    // A lifetime under the floor the API server validates is refused rather than
    // served for longer than it asked for.
    let too_short = dial(
        &config,
        "a lifetime under the server's floor",
        "POST",
        "/api/v1/namespaces/default/serviceaccounts/default/token",
        Some(&serde_json::json!({
            "spec": {
                "audiences": [PODS_AUDIENCE],
                "expirationSeconds": kubernetes_objects::MIN_TOKEN_LIFETIME.as_secs() - 1,
            }
        })),
    )
    .await;
    assert_eq!(
        too_short.status, 422,
        "a lifetime under the floor must be refused, got {too_short:?}"
    );
    assert_eq!(
        too_short.body["reason"],
        serde_json::json!("Invalid"),
        "the refusal states the value is invalid, got {:?}",
        too_short.body
    );
}

// ---------------------------------------------------------------------------
// RULE: a Pod created through the published endpoint reads back with the uid the
// create minted, a `TokenRequest` bound to it mints a token carrying that pod in
// its claims, and that token exchanges at `AssumeRoleForPodIdentity` for the
// credentials of the role the cluster's association binds the service account to —
// until the association is deleted, after which the same token resolves to nothing.
// ---------------------------------------------------------------------------

/// One row of the token table.
struct TokenCase {
    label: &'static str,
    namespace: &'static str,
    /// Whether the client creates the namespace, or uses one Kubernetes created
    /// with the cluster.
    create_namespace: bool,
    service_account: &'static str,
    /// Whether the client creates the service account, or uses the `default` one
    /// Kubernetes creates in every namespace.
    create_service_account: bool,
    pod: &'static str,
    /// The lifetime the `TokenRequest` asks for, and the lifetime the API server
    /// serves it at — the request's own value, capped at the ceiling the server
    /// issues within ([`kubernetes_objects::MAX_TOKEN_LIFETIME`]), or its default
    /// when the request asks for none. A value under the FLOOR is refused instead,
    /// which the refusal suite covers.
    expiration_seconds: Option<u64>,
    expected_seconds: u64,
}

#[tokio::test]
async fn a_pod_bound_token_is_exchanged_for_the_associated_role() {
    let (router, engine) = emulator_with_pod_identity();
    let cluster = "k8s-pod-identity";
    let config = provisioned_cluster(&router, REGION, cluster).await;

    // The rows differ along what the rule generalises over: whose identity the
    // token names (the namespace and service account Kubernetes creates with a
    // cluster, a service account the client created, a namespace AND service
    // account the client created) and the lifetime asked for (none, in range,
    // below the server's floor, past its ceiling).
    let cases = [
        TokenCase {
            label: "the default namespace and service account, no lifetime asked for",
            namespace: kubernetes_objects::DEFAULT_NAMESPACE,
            create_namespace: false,
            service_account: kubernetes_objects::DEFAULT_SERVICE_ACCOUNT,
            create_service_account: false,
            pod: "pod-default",
            expiration_seconds: None,
            expected_seconds: kubernetes_objects::DEFAULT_TOKEN_LIFETIME.as_secs(),
        },
        TokenCase {
            label: "a service account the client created",
            namespace: kubernetes_objects::DEFAULT_NAMESPACE,
            create_namespace: false,
            service_account: "checkout",
            create_service_account: true,
            pod: "pod-checkout",
            expiration_seconds: Some(7_200),
            expected_seconds: 7_200,
        },
        TokenCase {
            label: "a namespace and service account the client created, at the floor",
            namespace: "team-a",
            create_namespace: true,
            service_account: "worker",
            create_service_account: true,
            pod: "pod-worker",
            expiration_seconds: Some(kubernetes_objects::MIN_TOKEN_LIFETIME.as_secs()),
            expected_seconds: kubernetes_objects::MIN_TOKEN_LIFETIME.as_secs(),
        },
        TokenCase {
            label: "a lifetime past the server's ceiling",
            namespace: kubernetes_objects::DEFAULT_NAMESPACE,
            create_namespace: false,
            service_account: "long-lived",
            create_service_account: true,
            pod: "pod-long-lived",
            expiration_seconds: Some(999_999),
            expected_seconds: kubernetes_objects::MAX_TOKEN_LIFETIME.as_secs(),
        },
    ];

    for case in &cases {
        let label = case.label;
        if case.create_namespace {
            let created = dial(
                &config,
                label,
                "POST",
                "/api/v1/namespaces",
                Some(&serde_json::json!({ "metadata": { "name": case.namespace } })),
            )
            .await;
            assert_eq!(
                created.status, 201,
                "case {label:?}: expected the namespace to be created, got {created:?}"
            );
        }
        if case.create_service_account {
            let created = dial(
                &config,
                label,
                "POST",
                &format!("/api/v1/namespaces/{}/serviceaccounts", case.namespace),
                Some(&serde_json::json!({
                    "metadata": { "name": case.service_account }
                })),
            )
            .await;
            assert_eq!(
                created.status, 201,
                "case {label:?}: expected the service account to be created, got {created:?}"
            );
        }

        // The Pod object, which is what the token is bound to.
        let pod = dial(
            &config,
            label,
            "POST",
            &format!("/api/v1/namespaces/{}/pods", case.namespace),
            Some(&serde_json::json!({
                "metadata": { "name": case.pod },
                "spec": {
                    "serviceAccountName": case.service_account,
                    "containers": [{ "name": "app", "image": "public.ecr.aws/app:1" }],
                },
            })),
        )
        .await;
        assert_eq!(
            pod.status, 201,
            "case {label:?}: expected the pod to be created, got {pod:?}"
        );
        let uid = pod.body["metadata"]["uid"].as_str().unwrap_or_default().to_string();
        assert!(
            !uid.is_empty(),
            "case {label:?}: a created pod reports the uid the create minted, got {:?}",
            pod.body
        );

        // ROUND TRIP: the object reads back with exactly that uid.
        let read_back = dial(
            &config,
            label,
            "GET",
            &format!("/api/v1/namespaces/{}/pods/{}", case.namespace, case.pod),
            None,
        )
        .await;
        assert_eq!(
            read_back.status, 200,
            "case {label:?}: the created pod must read back, got {read_back:?}"
        );
        assert_eq!(
            read_back.body["metadata"]["uid"],
            serde_json::json!(uid),
            "case {label:?}: the pod reads back with the uid the create minted, got {:?}",
            read_back.body
        );

        // The `TokenRequest` subresource, bound to that pod.
        let mut spec = serde_json::json!({
            "audiences": [PODS_AUDIENCE],
            "boundObjectRef": { "kind": "Pod", "name": case.pod, "uid": uid },
        });
        if let Some(seconds) = case.expiration_seconds {
            spec["expirationSeconds"] = serde_json::json!(seconds);
        }
        let minted = dial(
            &config,
            label,
            "POST",
            &format!(
                "/api/v1/namespaces/{}/serviceaccounts/{}/token",
                case.namespace, case.service_account
            ),
            Some(&serde_json::json!({ "kind": "TokenRequest", "spec": spec })),
        )
        .await;
        assert_eq!(
            minted.status, 201,
            "case {label:?}: expected the token to be issued, got {minted:?}"
        );
        assert_eq!(
            minted.body["spec"]["expirationSeconds"],
            serde_json::json!(case.expected_seconds),
            "case {label:?}: expected a {}-second lifetime, got {:?}",
            case.expected_seconds,
            minted.body["spec"]
        );
        let token = minted.body["status"]["token"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !token.is_empty(),
            "case {label:?}: the answer carries the minted token, got {:?}",
            minted.body
        );
        let claims = token_claims(label, &token);
        assert_eq!(
            claims["kubernetes.io"]["pod"]["name"],
            serde_json::json!(case.pod),
            "case {label:?}: the token names the pod it was bound to, got {claims:?}"
        );
        assert_eq!(
            claims["kubernetes.io"]["pod"]["uid"],
            serde_json::json!(uid),
            "case {label:?}: the token carries that pod's uid, got {claims:?}"
        );
        assert_eq!(
            claims["kubernetes.io"]["serviceaccount"]["name"],
            serde_json::json!(case.service_account),
            "case {label:?}: the token names its service account, got {claims:?}"
        );
        assert_eq!(
            claims["kubernetes.io"]["namespace"],
            serde_json::json!(case.namespace),
            "case {label:?}: the token names its namespace, got {claims:?}"
        );
        assert_eq!(
            claims["aud"],
            serde_json::json!([PODS_AUDIENCE]),
            "case {label:?}: the token is issued to the audience asked for, got {claims:?}"
        );

        // Before the control plane associates the service account with a role,
        // there is no role to assume: the token resolves to no association.
        let unassociated =
            assume_role_for_pod_identity(&router, REGION, cluster, &token).await;
        assert_eq!(
            unassociated.status, 404,
            "case {label:?}: a token with no association must resolve to nothing, got \
             {unassociated:?}"
        );

        // The control plane's own call, which is what binds the identity the token
        // names to an IAM role.
        let association = create_association(
            &router,
            REGION,
            cluster,
            &serde_json::json!({
                "namespace": case.namespace,
                "serviceAccount": case.service_account,
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
        assert_eq!(
            association.status, 200,
            "case {label:?}: expected 200 from CreatePodIdentityAssociation, got {association:?}"
        );
        let association_id = association_id_of(&association);

        let exchanged = assume_role_for_pod_identity(&router, REGION, cluster, &token).await;
        assert_eq!(
            exchanged.status, 200,
            "case {label:?}: the pod-bound token must exchange for credentials, got {exchanged:?}"
        );
        assert_eq!(
            exchanged.body["subject"],
            serde_json::json!({
                "namespace": case.namespace,
                "serviceAccount": case.service_account,
            }),
            "case {label:?}: the exchange names the identity the token carries, got {:?}",
            exchanged.body
        );
        assert_eq!(
            exchanged.body["podIdentityAssociation"]["associationId"],
            serde_json::json!(association_id),
            "case {label:?}: the exchange names the association the control plane created, got \
             {:?}",
            exchanged.body
        );
        assert_eq!(
            exchanged.body["audience"],
            serde_json::json!(PODS_AUDIENCE),
            "case {label:?}: the exchange reports the pod identity audience, got {:?}",
            exchanged.body
        );
        // The credentials ARE a session of the associated role: `assumedRoleUser` names
        // that session, and the session itself is recorded where every service resolves a
        // temporary credential's principal — carrying the pod identity session tags, so a
        // policy conditioned on `aws:PrincipalTag/<key>` decides what the pod may do.
        let assume_role_id = exchanged.body["assumedRoleUser"]["assumeRoleId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let session_name = assume_role_id
            .split_once(':')
            .map(|(_role_id, name)| name.to_string())
            .unwrap_or_default();
        assert_eq!(
            exchanged.body["assumedRoleUser"]["arn"],
            serde_json::json!(format!(
                "arn:aws:sts::000000000000:assumed-role/PodIdentityRole/{session_name}"
            )),
            "case {label:?}: the exchange names the session of the associated role, got {:?}",
            exchanged.body
        );
        let access_key_id = exchanged.body["credentials"]["accessKeyId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let session = sts_state::service_state(&*engine)
            .expect("the shared engine carries the token service's session registry")
            .lock()
            .session(&access_key_id)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {label:?}: the vended credential must name a recorded session")
            });
        assert_eq!(
            session.role_arn.as_deref(),
            Some(ROLE_ARN),
            "case {label:?}: the session must be one of the associated role, got {session:?}"
        );
        let expected_tags = [
            ("eks-cluster-name", cluster.to_string()),
            ("kubernetes-namespace", case.namespace.to_string()),
            (
                "kubernetes-service-account",
                case.service_account.to_string(),
            ),
            ("kubernetes-pod-name", case.pod.to_string()),
            ("kubernetes-pod-uid", uid.clone()),
        ];
        for (key, value) in expected_tags {
            assert!(
                session
                    .tags
                    .iter()
                    .any(|(held, on)| held == key && *on == value),
                "case {label:?}: the session must carry the tag {key}={value}, got {:?}",
                session.tags
            );
        }
        assert!(
            exchanged.body["credentials"]["accessKeyId"]
                .as_str()
                .is_some_and(|key| !key.is_empty()),
            "case {label:?}: the exchange carries credentials, got {:?}",
            exchanged.body
        );

        // And once the control plane deletes the association, the SAME token has no
        // association to resolve: the exchange is a not-found again.
        let deleted = delete_association(&router, REGION, cluster, &association_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: expected 200 from DeletePodIdentityAssociation, got {deleted:?}"
        );
        let after_delete = assume_role_for_pod_identity(&router, REGION, cluster, &token).await;
        assert_eq!(
            after_delete.status, 404,
            "case {label:?}: a deleted association must resolve to nothing, got {after_delete:?}"
        );
        assert_eq!(
            after_delete.body["message"],
            serde_json::json!(NO_ASSOCIATION_REFUSAL),
            "case {label:?}: the refusal states that the token has no association, got {:?}",
            after_delete.body
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the address is the CLUSTER's, so deleting the cluster takes it away.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleting_the_cluster_takes_its_api_address_away() {
    let (router, _engine) = emulator_with_pod_identity();
    let config = provisioned_cluster(&router, REGION, "k8s-deleted").await;
    let served = dial(&config, "before the delete", "GET", "/version", None).await;
    assert_eq!(
        served.status, 200,
        "the published endpoint serves the API while the cluster exists, got {served:?}"
    );

    let deleted = crate::wire::call(
        &router,
        REGION,
        "DELETE",
        &format!("/clusters/{}", crate::wire::encode_label("k8s-deleted")),
        "",
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "expected 200 from DeleteCluster, got {deleted:?}"
    );

    // The port closes with the cluster. A connection attempt either finds nothing
    // listening or is severed, so what is asserted is that no ANSWER comes back —
    // waited for until a generous deadline rather than for a fixed time.
    let port: u16 = config
        .endpoint
        .rsplit_once(':')
        .and_then(|(_base, port)| port.parse().ok())
        .expect("the published endpoint carries the port it was served on");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        // The emulator's own listener again, on the port the cluster had published.
        let attempt = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await;
        let severed = match attempt {
            Err(_) => true,
            Ok(mut stream) => {
                // The listener is gone, so nothing answers the first byte of a
                // handshake: the stream reads end-of-file instead.
                let wrote = stream.write_all(b"GET /version HTTP/1.1\r\n\r\n").await;
                let mut read = [0_u8; 1];
                wrote.is_err() || matches!(stream.read(&mut read).await, Ok(0) | Err(_))
            }
        };
        if severed {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the deleted cluster's address {:?} must stop answering",
            config.endpoint
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
