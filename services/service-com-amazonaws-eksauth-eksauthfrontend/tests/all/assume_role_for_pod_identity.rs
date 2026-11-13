// SPDX-License-Identifier: CC0-1.0

//! End-to-end `AssumeRoleForPodIdentity` over the wire: a pod identity credential
//! exchange is authorised by the cluster that issued the token and by the association
//! that cluster stores for the token's (namespace, service account) pair, and every
//! output member is computed from those stored records.
//!
//! Each test drives the real request path — `MultiServiceRouter::dispatch` over a
//! `POST /clusters/{clusterName}/assume-role-for-pod-identity` — so the protocol
//! binding, the modeled constraints and the handler are all exercised. The tables span
//! the class each rule generalises over (namespace, service account, pod, cluster,
//! role), and the recorded case from the issue (`demo-cluster` / `default` / `my-sa`) is
//! one row among them.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_eksauth_eksauthfrontend::state::{
    service_state, AssociationRefusal, ServiceAccountTokenRequest, DEFAULT_REGION, PODS_AUDIENCE,
    TOKEN_RECORD_GRACE,
};
use service_com_amazonaws_eksauth_eksauthfrontend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One seeded association: where the pod runs and which role it may assume.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Seed {
    pub(crate) cluster: &'static str,
    pub(crate) namespace: &'static str,
    pub(crate) service_account: &'static str,
    pub(crate) pod: &'static str,
    pub(crate) role_arn: &'static str,
}

/// The seeded world every test in this file starts from: two clusters, four
/// associations, differing in namespace, service account, pod name and role — including
/// one role associated in BOTH clusters, and two namespaces carrying the same service
/// account name.
///
/// `pub(crate)` because it is the seeded world of the sibling suites too: every module of
/// `tests/all.rs` is one test binary, so this harness is shared rather than copied.
pub(crate) const SEEDS: &[Seed] = &[
    Seed {
        cluster: "demo-cluster",
        namespace: "default",
        service_account: "my-sa",
        pod: "my-app-7d9f8",
        role_arn: "arn:aws:iam::000000000000:role/pod-role",
    },
    Seed {
        cluster: "demo-cluster",
        namespace: "kube-system",
        service_account: "my-sa",
        pod: "controller-0",
        role_arn: "arn:aws:iam::000000000000:role/system-role",
    },
    Seed {
        cluster: "demo-cluster",
        namespace: "team-a",
        service_account: "checkout-worker-1",
        pod: "checkout-worker-1-abcde",
        role_arn: "arn:aws:iam::000000000000:role/path/checkout-role",
    },
    Seed {
        cluster: "Prod2",
        namespace: "default",
        service_account: "my-sa",
        pod: "api-0",
        role_arn: "arn:aws:iam::000000000000:role/pod-role",
    },
];

/// The emulator under test: one engine, one service, one router — the same dispatch path
/// the production router uses.
pub(crate) struct Harness {
    pub(crate) engine: Arc<engine::InMemoryEngine>,
    pub(crate) router: MultiServiceRouter,
}

impl Harness {
    /// A harness with [`SEEDS`] registered through the state seam, exactly as the EKS
    /// control plane would create the cluster and its pod identity associations.
    pub(crate) fn seeded() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the emulator's own concern and is off here: these
        // tests assert this operation's behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));

        let state = service_state(&*engine).expect("the in-memory engine carries this state");
        for seed in SEEDS {
            let mut guard = state.lock();
            guard
                .register_cluster(seed.cluster, DEFAULT_REGION)
                .unwrap_or_else(|| panic!("{} is a cluster name EKS accepts", seed.cluster));
            guard
                .associate_pod_identity(
                    seed.cluster,
                    seed.namespace,
                    seed.service_account,
                    seed.role_arn,
                )
                .unwrap_or_else(|refusal| panic!("seed {seed:?} is registrable, got {refusal:?}"));
        }
        Self { engine, router }
    }

    /// A projected service account token for `seed`, minted by the cluster that seed
    /// belongs to — the token the EKS Pod Identity Agent would present.
    pub(crate) fn token(&self, seed: &Seed) -> String {
        self.token_request(
            seed.cluster,
            ServiceAccountTokenRequest::for_pod(seed.namespace, seed.service_account, seed.pod),
        )
    }

    /// A token minted by `cluster` for an arbitrary request.
    pub(crate) fn token_request(
        &self,
        cluster: &str,
        request: ServiceAccountTokenRequest,
    ) -> String {
        self.issue(cluster, request)
            .unwrap_or_else(|| panic!("cluster {cluster} issues the token it was asked for"))
    }

    /// What `cluster`'s Kubernetes API server answers when asked for a token: `None`
    /// when the request names no service account.
    fn issue(&self, cluster: &str, request: ServiceAccountTokenRequest) -> Option<String> {
        let state = service_state(&*self.engine).expect("state");
        let mut guard = state.lock();
        guard
            .cluster_mut(cluster)
            .unwrap_or_else(|| panic!("cluster {cluster} is seeded"))
            .issue_service_account_token(&request)
    }

    /// `POST /clusters/{cluster}/assume-role-for-pod-identity` with `token`, returning
    /// the HTTP status and the parsed JSON body.
    pub(crate) async fn assume(&self, cluster: &str, token: &str) -> (u16, serde_json::Value) {
        self.assume_with(cluster, serde_json::json!({ "token": token }))
            .await
    }

    /// A token in the FORM a cluster issues — three base64url segments whose header names
    /// `cluster`'s own signing key — that `cluster` never issued, so only its record of
    /// what it issued can refuse it.
    fn unissued_token(&self, cluster: &str) -> String {
        crate::rejection_errors::token_for_key(&self.signing_key_id(cluster))
    }

    /// The id of the key `cluster`'s Kubernetes API server signs its tokens with — the
    /// `kid` header of every token it issues.
    pub(crate) fn signing_key_id(&self, cluster: &str) -> String {
        let state = service_state(&*self.engine).expect("state");
        let guard = state.lock();
        guard
            .cluster(cluster)
            .unwrap_or_else(|| panic!("cluster {cluster} is seeded"))
            .signing_key_id
            .clone()
    }

    /// The issuer URL `cluster`'s OIDC provider publishes, which its tokens name in their
    /// `iss` claim.
    pub(crate) fn oidc_issuer(&self, cluster: &str) -> String {
        let state = service_state(&*self.engine).expect("state");
        let guard = state.lock();
        guard
            .cluster(cluster)
            .unwrap_or_else(|| panic!("cluster {cluster} is seeded"))
            .oidc_issuer
            .clone()
    }

    /// The same call with a caller-built body, for the optional members.
    pub(crate) async fn assume_with(
        &self,
        cluster: &str,
        body: serde_json::Value,
    ) -> (u16, serde_json::Value) {
        let (status, _, raw) = self.send(cluster, body).await;
        let parsed = if raw.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null)
        };
        (status, parsed)
    }

    /// The same call, answered as the status, the `x-amzn-errortype` header and the RAW
    /// body — what a suite asserting the error envelope reads, where [`Self::assume_with`]
    /// answers the parsed body a success-path suite reads. The one place a request is
    /// built, so both read the same wire path.
    pub(crate) async fn send(
        &self,
        cluster: &str,
        body: serde_json::Value,
    ) -> (u16, Option<String>, String) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/clusters/{cluster}/assume-role-for-pod-identity"))
            // The routing key: the endpoint host names the `eks-auth` signing name.
            .header("host", format!("eks-auth.{DEFAULT_REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let errortype = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = String::from_utf8_lossy(response.body()).to_string();
        (status, errortype, raw)
    }
}

/// The error code a modeled failure renders, from the restJson1 error envelope.
fn error_code(status: u16, body: &serde_json::Value) -> String {
    let rendered = body["__type"]
        .as_str()
        .or_else(|| body["code"].as_str())
        .unwrap_or("<no __type>");
    format!(
        "{status} {}",
        rendered.rsplit(['#', '/']).next().unwrap_or(rendered)
    )
}

/// Seconds since the Unix epoch, for comparing against the `expiration` output member
/// (restJson1 renders a timestamp as epoch seconds).
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// Whether `value` is a UUID in the canonical hyphenated form (8-4-4-4-12 hex
/// characters), which is what the model documents at the end of `assumeRoleId`.
fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .map(|group| group.len())
            .eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// THE HAPPY PATH, over every seeded association: all five required output members come
/// from the stored cluster/association records, and the session is persisted.
#[tokio::test]
async fn every_seeded_association_exchanges_its_token_for_credentials_from_state() {
    let harness = Harness::seeded();

    for seed in SEEDS {
        let token = harness.token(seed);
        let (status, body) = harness.assume(seed.cluster, &token).await;
        assert_eq!(
            status, 200,
            "case {seed:?}: expected HTTP 200, got {status} {body}"
        );

        // What state holds for this seed, to compare the response against.
        let state = service_state(&*harness.engine).expect("state");
        let (association, cluster_arn) = {
            let guard = state.lock();
            let cluster = guard.cluster(seed.cluster).expect("seeded cluster");
            (
                cluster
                    .association_for(seed.namespace, seed.service_account)
                    .expect("seeded association")
                    .clone(),
                cluster.arn.clone(),
            )
        };
        assert!(
            cluster_arn.ends_with(&format!("cluster/{}", seed.cluster)),
            "case {seed:?}: the cluster ARN must name the cluster, got {cluster_arn:?}"
        );

        assert_eq!(
            body["subject"],
            serde_json::json!({
                "namespace": seed.namespace,
                "serviceAccount": seed.service_account,
            }),
            "case {seed:?}: subject must come from the token, got {}",
            body["subject"]
        );
        assert_eq!(
            body["audience"].as_str(),
            Some(PODS_AUDIENCE),
            "case {seed:?}: expected the pod identity audience, got {}",
            body["audience"]
        );
        assert_eq!(
            body["podIdentityAssociation"],
            serde_json::json!({
                "associationId": association.association_id,
                "associationArn": association.association_arn,
            }),
            "case {seed:?}: expected the stored association, got {}",
            body["podIdentityAssociation"]
        );
        // `<roleId>:eks-<clustername>-<podname>-<random UUID>`, as the model documents.
        let assume_role_id = body["assumedRoleUser"]["assumeRoleId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (role_id, session_name) = assume_role_id
            .split_once(':')
            .unwrap_or_else(|| panic!("case {seed:?}: no role id in {assume_role_id:?}"));
        assert_eq!(
            role_id, association.role_id,
            "case {seed:?}: expected the association's role id, got {role_id:?}"
        );
        let prefix = format!("eks-{}-{}-", seed.cluster, seed.pod);
        let uuid = session_name.strip_prefix(&prefix).unwrap_or_else(|| {
            panic!("case {seed:?}: expected session name {prefix}<uuid>, got {session_name:?}")
        });
        assert!(
            is_uuid(uuid),
            "case {seed:?}: expected a UUID after the pod name, got {uuid:?}"
        );

        // `assumedRoleUser.arn` is the ARN of the SESSION the exchange vended — what AWS
        // reports for an assumed-role identity — not the ARN of the role, which the
        // caller already holds on the association: the credentials ARE a session of that
        // role, so they speak under its assumed-role identity.
        let role_name = seed
            .role_arn
            .rsplit_once('/')
            .map(|(_path, name)| name)
            .unwrap_or_default();
        assert_eq!(
            body["assumedRoleUser"]["arn"].as_str(),
            Some(
                format!("arn:aws:sts::000000000000:assumed-role/{role_name}/{session_name}")
                    .as_str()
            ),
            "case {seed:?}: expected the session's assumed-role ARN, got {}",
            body["assumedRoleUser"]["arn"]
        );

        // The credentials, and the session persisted behind them.
        let access_key_id = body["credentials"]["accessKeyId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !access_key_id.is_empty()
                && !body["credentials"]["secretAccessKey"]
                    .as_str()
                    .unwrap_or_default()
                    .is_empty()
                && !body["credentials"]["sessionToken"]
                    .as_str()
                    .unwrap_or_default()
                    .is_empty(),
            "case {seed:?}: every credential member must be set, got {}",
            body["credentials"]
        );
        let expiration = body["credentials"]["expiration"].as_f64().unwrap_or(0.0);
        assert!(
            expiration > now_epoch_seconds(),
            "case {seed:?}: expiration must be in the future, got {expiration}"
        );

        let guard = state.lock();
        let session = guard
            .sessions
            .get(&access_key_id)
            .unwrap_or_else(|| panic!("case {seed:?}: no session stored for {access_key_id}"));
        assert_eq!(
            (
                session.cluster_name.as_str(),
                session.namespace.as_str(),
                session.service_account.as_str(),
                session.association_id.as_str(),
                session.assume_role_id.as_str(),
            ),
            (
                seed.cluster,
                seed.namespace,
                seed.service_account,
                association.association_id.as_str(),
                assume_role_id.as_str(),
            ),
            "case {seed:?}: the stored session must match the response, got {session:?}"
        );
    }
}

/// Each call mints a NEW session: nothing in the response is a canned constant.
#[tokio::test]
async fn successive_calls_mint_distinct_sessions() {
    let harness = Harness::seeded();
    let seed = SEEDS[0];
    let token = harness.token(&seed);

    let mut keys = Vec::new();
    let mut role_ids = Vec::new();
    for call in 0..3 {
        let (status, body) = harness.assume(seed.cluster, &token).await;
        assert_eq!(status, 200, "call {call}: expected HTTP 200, got {body}");
        keys.push(body["credentials"]["accessKeyId"].as_str().unwrap_or("").to_string());
        role_ids.push(
            body["assumedRoleUser"]["assumeRoleId"]
                .as_str()
                .unwrap_or("")
                .to_string(),
        );
    }

    for (left, right) in [(0, 1), (0, 2), (1, 2)] {
        assert_ne!(
            keys[left], keys[right],
            "calls {left} and {right} returned the same accessKeyId {:?}",
            keys[left]
        );
        assert_ne!(
            role_ids[left], role_ids[right],
            "calls {left} and {right} returned the same assumeRoleId {:?}",
            role_ids[left]
        );
    }

    let state = service_state(&*harness.engine).expect("state");
    assert_eq!(
        state.lock().sessions.len(),
        3,
        "each call must persist its own session"
    );
}

/// The optional node members are recorded on the session they were sent with.
#[tokio::test]
async fn the_worker_node_members_are_recorded_on_the_session() {
    let harness = Harness::seeded();
    let seed = SEEDS[0];
    let cases: &[(Option<&str>, Option<&str>, Option<&str>)] = &[
        (None, None, None),
        (Some("ip-10-0-1-5.ec2.internal"), Some("i-0123456789abcdef0"), Some("use1-az1")),
        (Some("ip-10-0-9-9.ec2.internal"), None, Some("use1-az4")),
    ];

    for case in cases {
        let (node, instance, zone) = *case;
        let mut body = serde_json::json!({ "token": harness.token(&seed) });
        for (member, value) in [
            ("eksNodeName", node),
            ("instanceId", instance),
            ("zone", zone),
        ] {
            if let Some(value) = value {
                body[member] = serde_json::Value::String(value.to_string());
            }
        }

        let (status, response) = harness.assume_with(seed.cluster, body).await;
        assert_eq!(status, 200, "case {case:?}: expected HTTP 200, got {response}");
        let access_key_id = response["credentials"]["accessKeyId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let state = service_state(&*harness.engine).expect("state");
        let guard = state.lock();
        let session = guard.sessions.get(&access_key_id).expect("a stored session");
        assert_eq!(
            (
                session.eks_node_name.as_deref(),
                session.instance_id.as_deref(),
                session.zone.as_deref(),
            ),
            (node, instance, zone),
            "case {case:?}: the session must record what the agent reported, got {session:?}"
        );
    }
}

/// An unregistered cluster publishes no signing key, so a token presented against one is
/// answered on the key its header names and never on whether that cluster exists — the
/// label is looked up in state, and a caller holding none of that cluster's key material
/// is told about its token.
#[tokio::test]
async fn an_unregistered_cluster_is_answered_on_the_token() {
    let harness = Harness::seeded();
    let token = harness.token(&SEEDS[0]);
    // Every name here satisfies the modeled `clusterName` pattern and length, so the
    // refusal comes from state, not from the boundary validator.
    for cluster in ["no-such-cluster", "demo-cluster2", "DEMO-CLUSTER", "0", "prod2"] {
        let (status, body) = harness.assume(cluster, &token).await;
        assert_eq!(
            error_code(status, &body),
            "400 InvalidTokenException",
            "case {cluster:?}: expected the modeled token refusal, got {status} {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some("The token included in the request has invalid header - kid."),
            "case {cluster:?}: the refusal must name the header that resolves to no key, \
             got {status} {body}"
        );
    }
}

/// A token the cluster issued, for a service account it holds no association for, is
/// refused as a not-found: there is no association resource to resolve, which is what a
/// token whose association was DELETED is answered with too — the state is the same.
#[tokio::test]
async fn a_service_account_without_an_association_is_denied() {
    let harness = Harness::seeded();
    // Each row is a (namespace, service account) pair the cluster has no association
    // for — including pairs that cross two seeded associations.
    let cases: &[(&str, &str)] = &[
        ("default", "other-sa"),
        ("team-a", "my-sa"),
        ("default", "checkout-worker-1"),
        ("not-a-namespace", "my-sa"),
        ("kube-system", "checkout-worker-1"),
    ];

    for case in cases {
        let (namespace, service_account) = *case;
        let token = harness.token_request(
            "demo-cluster",
            ServiceAccountTokenRequest::for_pod(namespace, service_account, "pod-0"),
        );
        let (status, body) = harness.assume("demo-cluster", &token).await;
        assert_eq!(
            error_code(status, &body),
            "404 ResourceNotFoundException",
            "case {case:?}: expected the modeled 404, got {status} {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(
                "The token included in the request has no service account role association \
                 for it."
            ),
            "case {case:?}: the refusal states that the token has no association, got \
             {status} {body}"
        );
    }
}

/// A token whose window has closed is expired while its record is still kept, and a
/// token still inside its window is not. Past the grace period the cluster keeps records
/// for, the record is gone and the token is simply one the cluster cannot vouch for.
#[tokio::test]
async fn a_token_past_its_expiry_is_refused_as_expired() {
    let harness = Harness::seeded();
    let seed = SEEDS[0];
    let now = SystemTime::now();

    // Each row expired inside `TOKEN_RECORD_GRACE`, so the cluster still holds its
    // record and can say that it expired.
    let grace = TOKEN_RECORD_GRACE.as_secs();
    for seconds_ago in [1_u64, 60, grace / 2, grace - 60] {
        // Issued before it expired, as a cluster issues a token: the window opened and
        // has since closed.
        let request =
            ServiceAccountTokenRequest::for_pod(seed.namespace, seed.service_account, seed.pod)
                .issued_at(now - Duration::from_secs(seconds_ago + 60))
                .expiring_at(now - Duration::from_secs(seconds_ago));
        let token = harness.token_request(seed.cluster, request);
        let (status, body) = harness.assume(seed.cluster, &token).await;
        assert_eq!(
            error_code(status, &body),
            "400 ExpiredTokenException",
            "case exp {seconds_ago}s ago: expected the modeled 400, got {status} {body}"
        );
    }

    // Past the grace period the record is forgotten, so the cluster no longer knows the
    // token at all and answers the invalid-token failure instead.
    for seconds_ago in [grace + 60, grace * 24] {
        let request =
            ServiceAccountTokenRequest::for_pod(seed.namespace, seed.service_account, seed.pod)
                .issued_at(now - Duration::from_secs(seconds_ago + 60))
                .expiring_at(now - Duration::from_secs(seconds_ago));
        let token = harness.token_request(seed.cluster, request);
        let (status, body) = harness.assume(seed.cluster, &token).await;
        assert_eq!(
            error_code(status, &body),
            "400 InvalidTokenException",
            "case exp {seconds_ago}s ago: expected the modeled 400, got {status} {body}"
        );
    }

    for seconds_ahead in [5_u64, 3_600, 86_400] {
        let request =
            ServiceAccountTokenRequest::for_pod(seed.namespace, seed.service_account, seed.pod)
                .expiring_at(now + Duration::from_secs(seconds_ahead));
        let token = harness.token_request(seed.cluster, request);
        let (status, body) = harness.assume(seed.cluster, &token).await;
        assert_eq!(
            status, 200,
            "case exp in {seconds_ahead}s: expected HTTP 200, got {status} {body}"
        );
    }
}

/// A token in the modelled JWT shape that this cluster did not issue for pod identity is
/// invalid: a token another cluster issued, one no cluster issued, one whose characters
/// were changed, or one issued to another audience. A request that names no service
/// account is not a token the cluster will issue at all.
#[tokio::test]
async fn a_token_the_cluster_did_not_issue_for_pod_identity_is_invalid() {
    let harness = Harness::seeded();
    let seed = SEEDS[0];

    // Issued by the OTHER cluster for the same namespace/service account.
    let other_cluster = harness.token_request(
        "Prod2",
        ServiceAccountTokenRequest::for_pod(seed.namespace, seed.service_account, seed.pod),
    );
    // The right token with its last segment replaced: a value the cluster never issued.
    let mut tampered = harness.token(&seed);
    let last_segment_at = tampered.rfind('.').expect("the token has two dots") + 1;
    tampered.replace_range(last_segment_at.., "AAAABBBBCCCCDDDDEEEEFFFF");
    // Issued for audiences that are not the pod identity one.
    let wrong_audience = harness.token_request(
        seed.cluster,
        ServiceAccountTokenRequest::for_pod(seed.namespace, seed.service_account, seed.pod)
            .for_audience(&["https://kubernetes.default.svc"]),
    );
    let no_audience = harness.token_request(
        seed.cluster,
        ServiceAccountTokenRequest::for_pod(seed.namespace, seed.service_account, seed.pod)
            .for_audience(&[]),
    );
    // A token in the form this cluster issues that it never issued.
    let unissued = harness.unissued_token(seed.cluster);
    // A token this cluster issued for a window that has not opened yet.
    let not_yet_valid = harness.token_request(
        seed.cluster,
        ServiceAccountTokenRequest::for_pod(seed.namespace, seed.service_account, seed.pod)
            .issued_at(SystemTime::now() + Duration::from_secs(300)),
    );

    let cases: &[(&str, &str)] = &[
        ("another cluster's token", other_cluster.as_str()),
        ("a tampered token", tampered.as_str()),
        ("another audience", wrong_audience.as_str()),
        ("no audience", no_audience.as_str()),
        ("a token no cluster issued", unissued.as_str()),
        ("a token not yet valid", not_yet_valid.as_str()),
    ];
    for case in cases {
        let (name, token) = *case;
        let (status, body) = harness.assume(seed.cluster, token).await;
        assert_eq!(
            error_code(status, &body),
            "400 InvalidTokenException",
            "case {name:?}: expected the modeled 400, got {status} {body}"
        );
    }

    // A Kubernetes API server issues a token FOR a service account, so a request that
    // names none it could issue for — or one whose validity window is empty — is refused
    // at the source rather than becoming an unusable token.
    let now = SystemTime::now();
    let unservable: &[(&str, ServiceAccountTokenRequest)] = &[
        (
            "an empty namespace",
            ServiceAccountTokenRequest::for_pod("", "my-sa", "pod-0"),
        ),
        (
            "an empty service account",
            ServiceAccountTokenRequest::for_pod("default", "", "pod-0"),
        ),
        (
            "a namespace that is not a DNS label",
            ServiceAccountTokenRequest::for_pod("Default", "my-sa", "pod-0"),
        ),
        (
            "a service account with an underscore",
            ServiceAccountTokenRequest::for_pod("default", "my_sa", "pod-0"),
        ),
        (
            "a namespace over 63 characters",
            ServiceAccountTokenRequest::for_pod(&"n".repeat(64), "my-sa", "pod-0"),
        ),
        (
            "an empty validity window",
            ServiceAccountTokenRequest::for_pod("default", "my-sa", "pod-0").expiring_at(now),
        ),
    ];
    for case in unservable {
        let (name, request) = case;
        let issued = harness.issue(seed.cluster, request.clone());
        assert!(
            issued.is_none(),
            "case {name:?}: the cluster must issue no token, got {issued:?}"
        );
    }
}

/// The registration seam refuses what the EKS control plane refuses, and records nothing
/// for a refused registration — not even the role's id.
#[tokio::test]
async fn the_registration_seam_refuses_what_eks_refuses() {
    let harness = Harness::seeded();
    let state = service_state(&*harness.engine).expect("state");

    // Cluster names: the modelled `ClusterName` is 1..100 characters of
    // `^[0-9A-Za-z][A-Za-z0-9\-_]*$`, so each of these names no cluster can carry.
    let refused_names: &[(&str, &str)] = &[
        ("empty", ""),
        ("leading dash", "-cluster"),
        ("a dot", "cluster.one"),
        ("a slash", "cluster/one"),
        ("a space", "my cluster"),
    ];
    for case in refused_names {
        let (name, cluster) = *case;
        let registered = state
            .lock()
            .register_cluster(cluster, DEFAULT_REGION)
            .is_some();
        assert!(
            !registered,
            "case {name:?}: {cluster:?} must not register as a cluster"
        );
    }
    // The published bounds of the same rule: 1 and 100 characters register, 101 does not.
    for (characters, expected) in [(1_usize, true), (100, true), (101, false)] {
        let cluster = "c".repeat(characters);
        let registered = state
            .lock()
            .register_cluster(&cluster, DEFAULT_REGION)
            .is_some();
        assert_eq!(
            registered, expected,
            "case {characters} characters: expected registered={expected}"
        );
    }

    // Associations: each row is refused for the reason EKS refuses it for.
    let refused_associations: &[(&str, &str, &str, &str, AssociationRefusal)] = &[
        (
            "unregistered cluster",
            "no-such-cluster",
            "default",
            "my-sa",
            AssociationRefusal::UnknownCluster,
        ),
        (
            "not an ARN",
            "demo-cluster",
            "default",
            "sa-a",
            AssociationRefusal::RoleArn,
        ),
        (
            "a user ARN",
            "demo-cluster",
            "default",
            "sa-b",
            AssociationRefusal::RoleArn,
        ),
        (
            "a role ARN with a region",
            "demo-cluster",
            "default",
            "sa-c",
            AssociationRefusal::RoleArn,
        ),
        (
            "a role ARN with a short account",
            "demo-cluster",
            "default",
            "sa-d",
            AssociationRefusal::RoleArn,
        ),
        (
            "an uppercase namespace",
            "demo-cluster",
            "Default",
            "sa-e",
            AssociationRefusal::Namespace,
        ),
        (
            "a service account with an underscore",
            "demo-cluster",
            "default",
            "sa_f",
            AssociationRefusal::ServiceAccount,
        ),
    ];
    // The role ARNs the rows above name, in the same order, so each refusal reason is
    // reached by the member it is about.
    let role_arns = [
        "arn:aws:iam::000000000000:role/pod-role",
        "not-an-arn",
        "arn:aws:iam::000000000000:user/someone",
        "arn:aws:iam:us-east-1:000000000000:role/pod-role",
        "arn:aws:iam::12345:role/pod-role",
        "arn:aws:iam::000000000000:role/pod-role",
        "arn:aws:iam::000000000000:role/pod-role",
    ];
    for (case, role_arn) in refused_associations.iter().zip(role_arns) {
        let (name, cluster, namespace, service_account, expected) = *case;
        // The role ids on record before the refused registration: a refusal must add
        // none, whatever the reason it was refused for.
        let role_ids_before = {
            let mut ids: Vec<String> = state.lock().role_ids.keys().cloned().collect();
            ids.sort();
            ids
        };
        let refusal = state
            .lock()
            .associate_pod_identity(cluster, namespace, service_account, role_arn)
            .map(|association| association.association_id.clone())
            .err();
        assert_eq!(
            refusal,
            Some(expected),
            "case {name:?}: expected {expected:?} for role {role_arn:?}"
        );
        let role_ids_after = {
            let mut ids: Vec<String> = state.lock().role_ids.keys().cloned().collect();
            ids.sort();
            ids
        };
        assert_eq!(
            role_ids_after, role_ids_before,
            "case {name:?}: a refused registration must record no role id"
        );
    }

    // The same seam accepts the forms EKS accepts, including a role path and a service
    // account name with a dot (a DNS subdomain).
    let accepted: &[(&str, &str, &str, &str)] = &[
        (
            "a plain role",
            "default",
            "accepted-sa",
            "arn:aws:iam::000000000000:role/plain-role",
        ),
        (
            "a role with a path",
            "default",
            "accepted-sa-2",
            "arn:aws:iam::000000000000:role/team/deep/path-role",
        ),
        (
            "another partition",
            "default",
            "accepted-sa-3",
            "arn:aws-us-gov:iam::000000000000:role/gov-role",
        ),
        (
            "a dotted service account",
            "default",
            "accepted.sa.4",
            "arn:aws:iam::000000000000:role/dotted-role",
        ),
    ];
    for case in accepted {
        let (name, namespace, service_account, role_arn) = *case;
        let recorded = state
            .lock()
            .associate_pod_identity("demo-cluster", namespace, service_account, role_arn)
            .map(|association| association.role_arn.clone());
        assert_eq!(
            recorded,
            Ok(role_arn.to_string()),
            "case {name:?}: expected the association to be recorded"
        );
    }
}

/// The role id is the role's own, so the same role associated in two clusters reports
/// one id — and two different roles never share one.
#[tokio::test]
async fn the_role_id_belongs_to_the_role_not_to_the_association() {
    let harness = Harness::seeded();
    let state = service_state(&*harness.engine).expect("state");

    let mut seen: Vec<(&str, String)> = Vec::new();
    for seed in SEEDS {
        let token = harness.token(seed);
        let (status, body) = harness.assume(seed.cluster, &token).await;
        assert_eq!(status, 200, "case {seed:?}: expected HTTP 200, got {body}");
        let assume_role_id = body["assumedRoleUser"]["assumeRoleId"]
            .as_str()
            .unwrap_or_default();
        let role_id = assume_role_id
            .split_once(':')
            .map(|(id, _)| id.to_string())
            .unwrap_or_default();
        // The id the service recorded for this role when it was first associated.
        let recorded = state.lock().role_ids.get(seed.role_arn).cloned();
        assert_eq!(
            Some(role_id.clone()),
            recorded,
            "case {seed:?}: the reported role id must be the one stored for the role, got \
             {role_id:?}"
        );
        // The shape AWS gives a role id, asserted independently of how it is minted:
        // `AROA` + 17 characters of the unique-id alphabet (uppercase letters, 2-7).
        let suffix = role_id
            .strip_prefix("AROA")
            .unwrap_or_else(|| panic!("case {seed:?}: expected an AROA role id, got {role_id:?}"));
        assert!(
            suffix.len() == 17
                && suffix
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || (b'2'..=b'7').contains(&b)),
            "case {seed:?}: expected AROA + 17 unique-id characters, got {role_id:?}"
        );
        seen.push((seed.role_arn, role_id));
    }

    for (left_arn, left_id) in &seen {
        for (right_arn, right_id) in &seen {
            assert_eq!(
                left_arn == right_arn,
                left_id == right_id,
                "{left_arn} and {right_arn} must share a role id only when they are one role: \
                 got {left_id} and {right_id}"
            );
        }
    }
}

/// A session is forgotten once the clock passes its expiration, whether or not anyone
/// asked — and a live session is left alone.
#[tokio::test]
async fn an_expired_session_is_forgotten_on_the_next_call() {
    let harness = Harness::seeded();
    let seed = SEEDS[0];
    let token = harness.token(&seed);

    let (status, body) = harness.assume(seed.cluster, &token).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} {body}");
    let stale = body["credentials"]["accessKeyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Backdate the stored session's expiration: the same state the clock would have
    // reached by itself six hours from now.
    let state = service_state(&*harness.engine).expect("state");
    state
        .lock()
        .sessions
        .get_mut(&stale)
        .expect("the session just minted")
        .expiration = SystemTime::now() - Duration::from_secs(1);

    let (status, body) = harness.assume(seed.cluster, &token).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} {body}");
    let fresh = body["credentials"]["accessKeyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let guard = state.lock();
    assert!(
        !guard.sessions.contains_key(&stale),
        "the expired session {stale} must be forgotten"
    );
    assert!(
        guard.sessions.contains_key(&fresh),
        "the live session {fresh} must be kept"
    );
}
