// SPDX-License-Identifier: CC0-1.0

//! The whole pod-identity path, end to end at the address the cluster PUBLISHES: a client
//! builds a kubeconfig out of `DescribeCluster`, talks Kubernetes at that address, and the
//! credential it exchanges the resulting token for may do exactly what the association's
//! role may do.
//!
//! RULE (the published address): a value an operation publishes for a caller to dial is an
//! authority THIS emulator answers on, with the resource's own path layout preserved.
//! `Cluster$endpoint` is "the endpoint for your Kubernetes API server" and
//! `Certificate$data` is "the Base64-encoded certificate data required to communicate with
//! your cluster. Add this to the `certificate-authority-data` section of the kubeconfig
//! file for your cluster" — the documented thing to do with the pair is to reach a live
//! Kubernetes API server with it, so the host and port the member names have to be ones
//! this emulator serves, under a certificate the published authority signed. A name under
//! the service's production DNS zone resolves to nothing here and sends every such client
//! off-emulator.
//!
//! RULE (what the vended credential may do): the credential
//! `AssumeRoleForPodIdentity` answers with is a session of the role the association binds
//! the service account to — "an object with the permanent IAM role identity and the
//! temporary session name"
//! (`AssumeRoleForPodIdentityResponse$assumedRoleUser`), and "the temporary security
//! credentials … have the permissions of the role" (`sts:AssumeRole`) — so a later request
//! signed with it is decided on that role's own identity-based policies, narrowed by the
//! association's `policy` where it carries one ("the permissions for the Pod Identity
//! association", `PodIdentityAssociation$policy`). A call those documents allow is served;
//! one nothing in force allows is refused HTTP 403 `AccessDenied` with IAM's sentence,
//! naming the SESSION as the principal and ending at the clause saying why the decision
//! went that way, whose reader is `iam_authorization_details_pointer.rs` beside this file.
//!
//! Four services on one engine make the pair of rules: EKS publishes the address and serves
//! the Kubernetes API behind it, EKS Auth exchanges the pod's token, STS records the session
//! and IAM holds the role whose policies decide the later call — so no single crate's test
//! target can pin them together, which is why this suite lives here.
//!
//! The rows span what the second rule generalises over: what the role's own documents say
//! about the call (nothing at all, an `Allow` of it, an `Allow` of another action, an
//! explicit `Deny` beside an `Allow`) and what the association's session policy says about
//! it (none, one that allows it, one that allows only another action) — each of which IAM
//! words its own refusal for.
//!
//! The AWS-side request harness (the SigV4 header and the EKS, EC2 and IAM drivers) is the
//! test crate's own `eks_wire`; the XML element readers come from `query_wire`. The
//! Kubernetes client below is the one this crate owns for a per-resource listener, so the
//! suite that reads a pod-identity refusal's authorization details back
//! (`eks_pod_identity_denial_authorization_details_resolve.rs`) drives the pod through
//! these helpers rather than through a second copy of them.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;

use crate::eks_wire::{cluster_body, ec2, eks, iam, iam_as, role_arn, send};
use crate::query_wire::{element, elements, opt_element};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The account every generated emulator presents (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// The action every row asks for with the pod's credential, and the one every refusal
/// names.
const ACTION: &str = "iam:ListRoles";

/// The Kubernetes namespace every association in this suite is written for — the one
/// Kubernetes creates with the cluster.
///
/// Reachable by the suite that reads a pod-identity refusal's authorization details back
/// (`eks_pod_identity_denial_authorization_details_resolve.rs`), because the association it
/// writes has to name the namespace [`pod_bound_token`] mints the token in.
pub(crate) const NAMESPACE: &str = "default";

/// The audience a projected token for the EKS Pod Identity Agent is issued to.
const PODS_AUDIENCE: &str = "pods.eks.amazonaws.com";

/// How long the suite waits for a clock-owed move, far above the span the emulator owes
/// it, so the deadline is reached only when the move is not coming at all.
const PROVISIONING_DEADLINE: Duration = Duration::from_secs(120);

/// How often the condition is looked at again while waiting for it.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The trust policy EKS Pod Identity documents for an association's role: the pods
/// principal, with the assume and the session tagging it performs.
///
/// Written with no `&`, `=`, `+` or `%`, so it travels in an awsQuery form body as
/// written.
const POD_IDENTITY_TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"Service":"pods.eks.amazonaws.com"},"#,
    r#""Action":["sts:AssumeRole","sts:TagSession"]}]}"#
);

/// One identity-based document with a single statement of `effect` about `action`, on
/// every resource.
fn statement(effect: &str, action: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Action\":\"{action}\",\"Resource\":\"*\"}}]}}"
    )
}

/// A document that allows `action` and explicitly denies [`ACTION`] — the pair that makes
/// an explicit deny decide the call although an `Allow` is in force.
fn allow_and_deny(action: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[\
         {{\"Effect\":\"Allow\",\"Action\":\"{action}\",\"Resource\":\"*\"}},\
         {{\"Effect\":\"Deny\",\"Action\":\"{ACTION}\",\"Resource\":\"*\"}}]}}"
    )
}

/// The SENTENCE of what IAM answers a refused [`ACTION`] with: the principal, the action,
/// the resource the request was made on and why the decision went that way. The refusal goes
/// on to point at the request's own authorization details, under a per-request id that
/// suite's reader takes rather than this one pinning it.
///
/// `ListRoles` is decided on the account's `role` collection under the path prefix being
/// listed, which for a request naming none is the default path. The sentence is the whole
/// of what this suite pins: a refusal of the identity service continues with the pointer at
/// that request's own authorization details, whose id is a fact of the run and not a value
/// to pin.
fn refusal(session_arn: &str, reason: &str) -> String {
    format!(
        "User: {session_arn} is not authorized to perform: {ACTION} on resource: \
         arn:aws:iam::{ACCOUNT}:role/ {reason}"
    )
}

/// Why IAM withheld the call when nothing in the role's own documents allowed it.
fn no_identity_policy() -> String {
    format!("because no identity-based policy allows the {ACTION} action")
}

/// Why IAM withheld the call when the role allowed it and the credential's own scope did
/// not.
fn no_session_policy() -> String {
    format!("because no session policy allows the {ACTION} action")
}

/// Why IAM refused the call when a statement of the role's documents denied it.
const EXPLICIT_DENY: &str = "with an explicit deny in an identity-based policy";

/// One row: what the role and the association say about [`ACTION`], and what the pod's
/// credential then earns.
struct Row {
    /// What this row is about, quoted by every assertion it makes.
    case: &'static str,
    /// The document embedded in the association's role, when it carries one.
    role_policy: Option<String>,
    /// The association's own session policy, when it carries one.
    session_policy: Option<String>,
    /// The reason IAM's refusal carries, and `None` for a row the call is served for.
    refused_because: Option<String>,
}

/// The rows, in the order they are driven.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a role carrying no identity-based policy at all",
            role_policy: None,
            session_policy: None,
            refused_because: Some(no_identity_policy()),
        },
        Row {
            case: "a role whose policy allows the action",
            role_policy: Some(statement("Allow", ACTION)),
            session_policy: None,
            refused_because: None,
        },
        Row {
            case: "a role whose policy allows another action",
            role_policy: Some(statement("Allow", "iam:ListUsers")),
            session_policy: None,
            refused_because: Some(no_identity_policy()),
        },
        Row {
            case: "a role whose policy denies the action beside an allow of it",
            role_policy: Some(allow_and_deny(ACTION)),
            session_policy: None,
            refused_because: Some(EXPLICIT_DENY.to_string()),
        },
        Row {
            case: "the association's session policy allows what the role allows",
            role_policy: Some(statement("Allow", ACTION)),
            session_policy: Some(statement("Allow", ACTION)),
            refused_because: None,
        },
        Row {
            case: "the association's session policy allows another action only",
            role_policy: Some(statement("Allow", ACTION)),
            session_policy: Some(statement("Allow", "iam:ListUsers")),
            refused_because: Some(no_session_policy()),
        },
    ]
}

/// What a client reads out of `DescribeCluster` to build a kubeconfig: the address the
/// cluster published for its API server, and the certificate authority published beside
/// it.
pub(crate) struct Kubeconfig {
    endpoint: String,
    certificate_authority_pem: String,
}

/// Create the role `name` with the pod identity trust policy, and embed `policy` in it
/// when the row carries one.
pub(crate) fn create_role(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    name: &str,
    policy: Option<&str>,
) {
    let created = iam(
        router,
        runtime,
        "CreateRole",
        &[
            ("RoleName", name),
            ("AssumeRolePolicyDocument", POD_IDENTITY_TRUST_POLICY),
        ],
    );
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreateRole {name} expected 200, got {created:?}"
    );
    if let Some(document) = policy {
        let embedded = iam(
            router,
            runtime,
            "PutRolePolicy",
            &[
                ("RoleName", name),
                ("PolicyName", "pod-identity-scope"),
                ("PolicyDocument", document),
            ],
        );
        assert_eq!(
            embedded.status, 200,
            "case {case:?}: PutRolePolicy on {name} expected 200, got {embedded:?}"
        );
    }
}

/// `DescribeCluster` until the cluster reports `ACTIVE` with its API server address
/// published, then read the kubeconfig members back the way a client does — the address,
/// and the certificate authority decoded as the member documents ("the Base64-encoded
/// certificate data required to communicate with your cluster").
///
/// The wait is on the CONDITION, re-read until it holds or a generous deadline passes, so
/// a loaded host only makes it slower rather than failing it.
pub(crate) fn provisioned_kubeconfig(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    cluster: &str,
) -> Kubeconfig {
    let deadline = Instant::now() + PROVISIONING_DEADLINE;
    loop {
        let described = eks(router, runtime, "GET", &format!("/clusters/{cluster}"), None);
        assert_eq!(
            described.status, 200,
            "setup: DescribeCluster {cluster} expected 200, got {described:?}"
        );
        let reported = &described.body["cluster"];
        if reported["status"] == serde_json::json!("ACTIVE") && reported["endpoint"].is_string() {
            let endpoint = reported["endpoint"]
                .as_str()
                .expect("the published endpoint")
                .to_string();
            let data = reported["certificateAuthority"]["data"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("setup: {cluster} publishes no certificate authority: {described:?}")
                });
            let decoded = aws_smithy_types::base64::decode(data).unwrap_or_else(|error| {
                panic!("setup: the published CA data must be Base64: {error}")
            });
            let certificate_authority_pem = String::from_utf8(decoded)
                .unwrap_or_else(|error| panic!("setup: the published CA must be text: {error}"));
            return Kubeconfig {
                endpoint,
                certificate_authority_pem,
            };
        }
        assert!(
            Instant::now() < deadline,
            "setup: the clock owes {cluster} ACTIVE with a published endpoint, got {described:?}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// A Kubernetes client for `config`, trusting NOTHING but the certificate authority the
/// cluster published — so a request below reaches the cluster exactly when the certificate
/// served at the published endpoint was issued by that authority.
pub(crate) fn kubernetes_client(config: &Kubeconfig) -> reqwest::blocking::Client {
    let authority = reqwest::Certificate::from_pem(config.certificate_authority_pem.as_bytes())
        .unwrap_or_else(|error| {
            panic!("setup: the published CA must load as a trust anchor: {error}")
        });
    reqwest::blocking::Client::builder()
        .add_root_certificate(authority)
        .build()
        .unwrap_or_else(|error| panic!("setup: the Kubernetes client must build: {error}"))
}

/// Assert the published endpoint is an address this emulator serves — a loopback host and
/// the port its own listener answers on — rather than a name under the service's
/// production DNS zone, and answer its `https://` origin.
pub(crate) fn served_origin(config: &Kubeconfig) -> String {
    let authority = config
        .endpoint
        .strip_prefix("https://")
        .unwrap_or_else(|| {
            panic!(
                "the published endpoint must be an https URL, got {:?}",
                config.endpoint
            )
        });
    let (host, port) = authority.rsplit_once(':').unwrap_or_else(|| {
        panic!(
            "the published endpoint must name the port it is served on, got {:?}",
            config.endpoint
        )
    });
    assert!(
        matches!(host, "127.0.0.1" | "localhost" | "[::1]"),
        "the published endpoint must be an address this emulator serves, not a name under \
         the service's own DNS zone, got {:?}",
        config.endpoint
    );
    port.parse::<u16>().unwrap_or_else(|error| {
        panic!(
            "the published endpoint's port must be a port, got {:?}: {error}",
            config.endpoint
        )
    });
    config.endpoint.clone()
}

/// One request to the cluster's published Kubernetes API address, with the path layout the
/// Kubernetes API publishes, answered as the status and the parsed body.
///
/// It presents the ACCOUNT's own credential as the bearer token a kubeconfig's credential
/// plugin builds: an API server authenticates every request, so a client of one presents a
/// token.
pub(crate) fn kubernetes(
    client: &reqwest::blocking::Client,
    origin: &str,
    case: &str,
    path: &str,
    body: &serde_json::Value,
) -> (u16, serde_json::Value) {
    let response = client
        .post(format!("{origin}{path}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "Bearer {}",
                crate::eks_wire::account_kubernetes_bearer_token()
            ),
        )
        .body(body.to_string())
        .send()
        .unwrap_or_else(|error| {
            panic!("case {case:?}: POST {path} must reach the published endpoint: {error}")
        });
    let status = response.status().as_u16();
    let text = response.text().unwrap_or_default();
    let parsed = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    (status, parsed)
}

/// Mint a pod-bound projected token over the cluster's OWN Kubernetes API, as a pod's
/// kubelet does: the service account, then the Pod object, then the `TokenRequest` bound to
/// that Pod — the only token class `AssumeRoleForPodIdentity` accepts.
pub(crate) fn pod_bound_token(
    client: &reqwest::blocking::Client,
    origin: &str,
    case: &str,
    service_account: &str,
    pod: &str,
) -> String {
    let (status, created) = kubernetes(
        client,
        origin,
        case,
        &format!("/api/v1/namespaces/{NAMESPACE}/serviceaccounts"),
        &serde_json::json!({ "metadata": { "name": service_account } }),
    );
    assert_eq!(
        status, 201,
        "case {case:?}: the service account must be created, got {status} {created}"
    );
    let (status, created) = kubernetes(
        client,
        origin,
        case,
        &format!("/api/v1/namespaces/{NAMESPACE}/pods"),
        &serde_json::json!({
            "metadata": { "name": pod },
            "spec": {
                "serviceAccountName": service_account,
                "containers": [{ "name": "app", "image": "public.ecr.aws/app:1" }],
            },
        }),
    );
    assert_eq!(
        status, 201,
        "case {case:?}: the pod must be created, got {status} {created}"
    );
    let uid = created["metadata"]["uid"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the created pod must carry a uid, got {created}"))
        .to_string();
    let (status, minted) = kubernetes(
        client,
        origin,
        case,
        &format!("/api/v1/namespaces/{NAMESPACE}/serviceaccounts/{service_account}/token"),
        &serde_json::json!({
            "kind": "TokenRequest",
            "spec": {
                "audiences": [PODS_AUDIENCE],
                "boundObjectRef": { "kind": "Pod", "name": pod, "uid": uid },
            },
        }),
    );
    assert_eq!(
        status, 201,
        "case {case:?}: the token must be issued, got {status} {minted}"
    );
    minted["status"]["token"]
        .as_str()
        .filter(|token| !token.is_empty())
        .unwrap_or_else(|| panic!("case {case:?}: the answer must carry the token, got {minted}"))
        .to_string()
}

/// Exchange `token` at `AssumeRoleForPodIdentity`, and answer the access key id of the
/// session with the ARN that session is reported under.
pub(crate) fn exchange(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    cluster: &str,
    token: &str,
) -> (String, String) {
    let answer = send(
        router,
        runtime,
        "eks-auth",
        "POST",
        &format!("/clusters/{cluster}/assume-role-for-pod-identity"),
        "application/json",
        serde_json::json!({ "token": token }).to_string(),
    );
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the pod identity exchange expected 200, got {answer:?}"
    );
    let access_key_id = answer.body["credentials"]["accessKeyId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no credentials in {}", answer.text))
        .to_string();
    let session_arn = answer.body["assumedRoleUser"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no assumedRoleUser in {}", answer.text))
        .to_string();
    (access_key_id, session_arn)
}

/// RULE: a client that does the documented thing — build a kubeconfig out of
/// `DescribeCluster`, reach the Kubernetes API at the published address, and exchange a
/// pod-bound token — is handed a session of the association's role, and that session may do
/// exactly what the role's identity-based policies (narrowed by the association's session
/// policy) allow: served where they allow the call, refused HTTP 403 `AccessDenied` naming
/// the session, the action, the resource and the reason, and pointing at the authorization
/// details of the request, where they do not.
#[test]
fn a_pod_identity_credential_from_the_published_endpoint_carries_the_roles_permissions() {
    std::thread::Builder::new()
        .name("eks-pod-identity-credential-scope".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            // The cluster's Kubernetes API answers on a listener of the emulator's own, so
            // the runtime this suite drives the emulator on has to carry the IO driver that
            // socket is opened on, and its tasks the stack the assembly needs.
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_stack_size(BOOT_STACK_SIZE)
                .enable_all()
                .build()
                .expect("a runtime with the IO driver");

            // The cluster is created in subnets EC2 resolves, so the account's own default
            // subnets are read from EC2 rather than invented.
            let defaults = ec2(
                &router,
                &runtime,
                "DescribeSubnets",
                &[
                    ("Filter.1.Name", "default-for-az"),
                    ("Filter.1.Value.1", "true"),
                ],
            );
            assert_eq!(
                defaults.status, 200,
                "setup: DescribeSubnets must succeed, got {defaults:?}"
            );
            let subnets = elements(&defaults.text, "subnetId");
            assert!(
                subnets.len() >= 2,
                "setup: the account must have default subnets, got {subnets:?}"
            );
            let subnets = vec![subnets[0].clone(), subnets[1].clone()];

            let cluster = "pod-identity-scope-host";
            let created = eks(
                &router,
                &runtime,
                "POST",
                "/clusters",
                Some(cluster_body(cluster, &subnets)),
            );
            assert_eq!(
                created.status, 200,
                "setup: the cluster must be created, got {created:?}"
            );
            let config = provisioned_kubeconfig(&router, &runtime, cluster);
            let origin = served_origin(&config);
            let client = kubernetes_client(&config);

            for (index, row) in rows().into_iter().enumerate() {
                let Row {
                    case,
                    role_policy,
                    session_policy,
                    refused_because,
                } = row;
                let role_name = format!("pod-identity-scope-{index}");
                create_role(
                    &router,
                    &runtime,
                    case,
                    &role_name,
                    role_policy.as_deref(),
                );

                // The association that binds the service account to that role, written
                // through the control plane exactly as a customer writes it. An association
                // carrying a session policy turns the session tags off, which is what the
                // control plane requires of one ("When policy is specified,
                // disableSessionTags must be set to true").
                let service_account = format!("scope-{index}");
                let mut body = serde_json::json!({
                    "namespace": NAMESPACE,
                    "serviceAccount": service_account,
                    "roleArn": role_arn(&role_name),
                });
                if let Some(document) = session_policy.as_deref() {
                    body["policy"] = serde_json::json!(document);
                    body["disableSessionTags"] = serde_json::json!(true);
                }
                let association = eks(
                    &router,
                    &runtime,
                    "POST",
                    &format!("/clusters/{cluster}/pod-identity-associations"),
                    Some(body),
                );
                assert_eq!(
                    association.status, 200,
                    "case {case:?}: the association must be created, got {association:?} ({})",
                    association.message()
                );

                // The token a pod of that service account presents, minted over the
                // published address.
                let token = pod_bound_token(
                    &client,
                    &origin,
                    case,
                    &service_account,
                    &format!("{service_account}-pod"),
                );
                let (access_key_id, session_arn) =
                    exchange(&router, &runtime, case, cluster, &token);
                assert!(
                    session_arn.starts_with(&format!(
                        "arn:aws:sts::{ACCOUNT}:assumed-role/{role_name}/"
                    )),
                    "case {case:?}: the exchange must report a session of the association's \
                     role, got {session_arn:?}"
                );

                // The call the pod then makes with that credential.
                let listed = iam_as(&router, &runtime, &access_key_id, "ListRoles", &[]);
                match refused_because {
                    None => {
                        assert_eq!(
                            listed.status, 200,
                            "case {case:?}: a call the role allows must be served, got {listed:?}"
                        );
                        let names = elements(&listed.text, "RoleName");
                        assert!(
                            names.iter().any(|name| name == &role_name),
                            "case {case:?}: expected {role_name} among the listed roles, got \
                             {names:?}"
                        );
                    }
                    Some(reason) => {
                        assert_eq!(
                            listed.status, 403,
                            "case {case:?}: a call nothing in force allows must be refused 403, \
                             got {listed:?}"
                        );
                        assert_eq!(
                            opt_element(&listed.text, "Code").as_deref(),
                            Some("AccessDenied"),
                            "case {case:?}: expected the error code AccessDenied, got {}",
                            listed.text
                        );
                        // An authorization refusal of the identity service IS the sentence
                        // naming the decision, closed by the pointer at that decision's own
                        // authorization details, which the suite that owns that reader pins
                        // (`iam_authorization_details_pointer.rs`).
                        crate::iam_authorization_details_pointer::assert_identity_refusal(
                            case,
                            &element(&listed.text, "Message"),
                            &refusal(&session_arn, &reason),
                        );
                    }
                }
            }
        })
        .expect("spawn the eks pod-identity credential-scope thread")
        .join()
        .expect("the eks pod-identity credential-scope thread");
}
