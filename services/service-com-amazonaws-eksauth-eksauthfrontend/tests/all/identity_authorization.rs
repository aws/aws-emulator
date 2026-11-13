// SPDX-License-Identifier: CC0-1.0

//! The IAM AUTHORIZATION of `AssumeRoleForPodIdentity`, end to end.
//!
//! RULE: every request to this service is signed with IAM credentials, and AWS decides such a
//! call at the IAM layer BEFORE the service reads the request — so the caller's own
//! identity-based policies decide it ahead of the input's constraints, ahead of the token and
//! ahead of any lookup. The decision is IAM's own: an explicit `Deny` about the request wins
//! over every `Allow`, an `Allow` about it permits, and a principal with neither falls to the
//! default deny. The action is `eks-auth:AssumeRoleForPodIdentity` — this service's action
//! namespace is the name it signs under — and the resource is the CLUSTER the request
//! addresses, `arn:aws:eks:<region>:<account>:cluster/<name>`, built from the name the URI
//! carried rather than from a stored record, so the decision is the same whether or not that
//! cluster exists. A refusal is the modelled `AccessDeniedException` at HTTP 400, carrying
//! IAM's own sentence, and it persists nothing: no session is minted.
//!
//! The rows span what the rule generalises over: a principal with no policy at all, a broad
//! grant, a grant scoped to THIS cluster, one scoped to ANOTHER cluster, one scoped to this
//! cluster in another Region, a grant of a DIFFERENT service's actions (EKS proper, whose
//! actions are not this service's), and an explicit deny beside each kind of allow. Beside the
//! table: the ORDER the decision is taken in — a refused principal is answered on its
//! permissions even when the same request would also fail the modelled constraints, while an
//! authorized one meets those constraints — and the account's own published credential, which
//! holds no IAM policies and keeps being served.
//!
//! The principals are made with IAM's and STS's OWN operations over the same engine, so the
//! documents this service reads are the ones those services wrote.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eksauth_eksauthfrontend::state::{
    service_state, ServiceAccountTokenRequest,
};
use service_com_amazonaws_eksauth_eksauthfrontend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one the cluster ARN a decision is
/// taken against therefore carries.
const REGION: &str = "us-west-2";

/// The cluster the seeded association lives in, and a second cluster name a grant can be
/// scoped to instead.
const CLUSTER: &str = "demo-cluster";
const OTHER_CLUSTER: &str = "Prod2";

/// The seeded pod identity association: the service account the token names and the role it
/// may assume.
const NAMESPACE: &str = "default";
const SERVICE_ACCOUNT: &str = "my-sa";
const POD: &str = "my-app-7d9f8";
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/pod-role";

/// The IAM action this operation is performed under: this service's action namespace
/// (`arnNamespace` / `endpointPrefix` `eks-auth`) and the operation's own name.
const ACTION: &str = "eks-auth:AssumeRoleForPodIdentity";

/// The modelled refusal of an unauthorized caller, at the status the model binds it to
/// (`AccessDeniedException`, `@httpError(400)`).
const ACCESS_DENIED: &str = "AccessDeniedException";
const CLIENT_ERROR_STATUS: u16 = 400;

/// The modelled refusal of a value that breaks a constraint the model declares — what an
/// AUTHORIZED caller sending a malformed request meets, and what an unauthorized one must NOT
/// be answered with, since IAM decides first.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The access-key id standing for the account's own published credential: neither STS nor IAM
/// holds a record for it, so it resolves to the account's ambient identity, which holds no IAM
/// policies — the position AWS account root credentials are in. Every fixture is made with it.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The `RoleSessionName` every `AssumeRole` here asks for — the last segment of the
/// assumed-role ARN a refusal names as the principal.
const SESSION: &str = "probe";

/// A cluster name the modelled `ClusterName` pattern refuses (a leading hyphen), and a value
/// that is not a JSON Web Token at all: together, a request the service refuses on its input
/// the moment it is allowed to read it.
const MALFORMED_CLUSTER: &str = "-bad-cluster";
const MALFORMED_TOKEN: &str = "not-a-jwt-token-at-all";

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal must
/// blame it by — what `sts:GetCallerIdentity` reports for that key.
struct Principal {
    access_key_id: String,
    arn: String,
}

/// What one call answered: the status, the error shape restJson1 resolves from
/// `x-amzn-errortype`, and the message.
#[derive(Debug)]
struct Reply {
    status: u16,
    errortype: Option<String>,
    message: String,
    body: String,
}

impl Reply {
    /// `<status> <shape>`, for comparing a whole verdict in one assertion. A real service
    /// appends a coral suffix to the header (`:http://internal.amazon.com/coral/…`), which
    /// names the same shape, so the shape name is what is compared.
    fn verdict(&self) -> String {
        let shape = self.errortype.as_deref().unwrap_or("<no errortype header>");
        format!("{} {}", self.status, shape.split(':').next().unwrap_or(shape))
    }
}

/// EKS Auth beside the IAM and STS services its callers are made with, all on ONE shared
/// engine — the same dispatch path the assembled router uses.
///
/// The sibling suites mount this service alone and send no credential scope, which is right
/// for a suite about what it serves; a suite about WHO is calling needs the two services that
/// hold principals and their policies, and a credential scope naming a different key per call.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    /// This emulator with one cluster registered and one pod identity association on it,
    /// through the state seam the EKS control plane would drive.
    fn seeded() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Nothing here signs: the `Authorization` header carries only the credential scope,
        // which is where a service reads the caller's access key id and Region from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        let aws = Self { engine, router };
        let state = service_state(&*aws.engine).expect("the in-memory engine carries this state");
        let mut guard = state.lock();
        guard
            .register_cluster(CLUSTER, REGION)
            .expect("the seeded cluster name is one EKS accepts");
        guard
            .associate_pod_identity(CLUSTER, NAMESPACE, SERVICE_ACCOUNT, ROLE_ARN)
            .expect("the seeded association is registrable");
        drop(guard);
        aws
    }

    /// A projected service account token the seeded cluster's Kubernetes API server issued for
    /// the seeded pod — the token the EKS Pod Identity Agent would present.
    fn issued_token(&self) -> String {
        let state = service_state(&*self.engine).expect("state");
        let mut guard = state.lock();
        guard
            .cluster_mut(CLUSTER)
            .expect("the seeded cluster is registered")
            .issue_service_account_token(&ServiceAccountTokenRequest::for_pod(
                NAMESPACE,
                SERVICE_ACCOUNT,
                POD,
            ))
            .expect("the cluster issues the token it was asked for")
    }

    /// How many pod identity sessions this service holds — read to prove a refused call minted
    /// nothing.
    fn sessions(&self) -> usize {
        let state = service_state(&*self.engine).expect("state");
        let held = state.lock().sessions.len();
        held
    }

    /// `POST /clusters/{cluster}/assume-role-for-pod-identity` with `token`, signed for
    /// [`REGION`] with `access_key_id` in the credential scope.
    async fn assume_as(&self, access_key_id: &str, cluster: &str, token: &str) -> Reply {
        let host = format!("eks-auth.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://{host}/clusters/{cluster}/assume-role-for-pod-identity"
            ))
            .header("host", host)
            .header("content-type", "application/json")
            .header("authorization", credential_scope(access_key_id, "eks-auth"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(&json!({ "token": token })).expect("the body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let errortype = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = String::from_utf8_lossy(response.body()).to_string();
        let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        Reply {
            status,
            errortype,
            message: parsed["message"].as_str().unwrap_or_default().to_string(),
            body,
        }
    }

    /// One awsQuery call to IAM or STS, made with the account's own credential, that must be
    /// served.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> String {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", credential_scope(ACCOUNT_KEY, signing_name))
            .body(bytes::Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "setup {action}: the call must succeed, got {status} with {text}"
        );
        text
    }
}

/// A SigV4 `Authorization` header carrying nothing but the credential scope: the access key id
/// a service reads the caller from, and the Region a resource ARN is spelled in.
fn credential_scope(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal — so a policy document's braces and quotes survive the round
/// trip.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The text of the first `<name>…</name>` element of an awsQuery response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// Create an IAM role carrying `policies` as its inline policies, assume it through STS, and
/// answer the session's access key id with the session ARN.
async fn assumed_role(aws: &Emulator, role_name: &str, policies: &[String]) -> Principal {
    let account = engine::emulator_account_id();
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    aws.query(
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    for (index, policy) in policies.iter().enumerate() {
        aws.query(
            "iam",
            "2010-05-08",
            "PutRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyName", format!("inline-{index}")),
                ("PolicyDocument", policy.clone()),
            ],
        )
        .await;
    }
    let assumed = aws
        .query(
            "sts",
            "2011-06-15",
            "AssumeRole",
            &[
                ("RoleArn", format!("arn:aws:iam::{account}:role/{role_name}")),
                ("RoleSessionName", SESSION.to_string()),
            ],
        )
        .await;
    Principal {
        access_key_id: xml_element(&assumed, "AccessKeyId")
            .unwrap_or_else(|| panic!("AssumeRole answers an AccessKeyId, got {assumed}")),
        arn: xml_element(&assumed, "Arn")
            .unwrap_or_else(|| panic!("AssumeRole answers the session's Arn, got {assumed}")),
    }
}

/// One identity-based policy document carrying one statement.
fn statement(effect: &str, actions: &[&str], resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": effect, "Action": actions, "Resource": resource}],
    })
    .to_string()
}

/// The ARN of the cluster a request naming `cluster` addresses, spelled here as EKS Auth's
/// Service Authorization Reference spells the `cluster` resource type — independently of the
/// crate under test, so the test does not re-assert the implementation's own spelling.
fn cluster_resource(region: &str, cluster: &str) -> String {
    format!(
        "arn:aws:eks:{region}:{}:cluster/{cluster}",
        engine::emulator_account_id()
    )
}

/// IAM's refusal sentence: who was refused, which action, on which resource, and which of
/// IAM's rules refused it.
fn not_authorized(principal: &str, resource: &str, reason: &str) -> String {
    format!("User: {principal} is not authorized to perform: {ACTION} on resource: {resource} {reason}")
}

/// The reason clause for a request no readable document allows — IAM's default deny.
fn no_policy_allows() -> String {
    format!("because no identity-based policy allows the {ACTION} action")
}

/// The reason clause for a request an explicit `Deny` refuses.
const EXPLICIT_DENY: &str = "with an explicit deny in an identity-based policy";

/// One row of the grant table: the documents in force on the caller's role, and what the call
/// must be answered with under them.
struct Row {
    case: &'static str,
    /// The documents the role carries. Built from the cluster ARN the call addresses, so a row
    /// can scope a grant to exactly that resource.
    policies: fn(&str) -> Vec<String>,
    /// The reason clause a refusal must carry, or `None` when the call must be served.
    refused: Option<fn() -> String>,
}

/// THE RULE: the caller's identity-based policies decide this call, by IAM's own order, judged
/// against this action and the cluster the request addresses — and an authorized caller is
/// served from the stored records exactly as before.
#[tokio::test]
async fn the_callers_identity_policies_decide_the_credential_exchange() {
    let rows: &[Row] = &[
        Row {
            case: "a principal with no policy at all",
            policies: |_| Vec::new(),
            refused: Some(no_policy_allows),
        },
        Row {
            case: "an allow of every action on every resource",
            policies: |_| vec![statement("Allow", &["*"], "*")],
            refused: None,
        },
        Row {
            case: "an allow of this action on this cluster",
            policies: |cluster| vec![statement("Allow", &[ACTION], cluster)],
            refused: None,
        },
        Row {
            case: "an allow of every action of this service",
            policies: |_| vec![statement("Allow", &["eks-auth:*"], "*")],
            refused: None,
        },
        Row {
            case: "an allow of this action on another cluster",
            policies: |_| {
                vec![statement(
                    "Allow",
                    &[ACTION],
                    &cluster_resource(REGION, OTHER_CLUSTER),
                )]
            },
            refused: Some(no_policy_allows),
        },
        Row {
            case: "an allow of this action on this cluster in another Region",
            policies: |_| {
                vec![statement(
                    "Allow",
                    &[ACTION],
                    &cluster_resource("eu-central-1", CLUSTER),
                )]
            },
            refused: Some(no_policy_allows),
        },
        Row {
            // EKS proper publishes its own action namespace, and a grant of it is not a grant
            // of this service's one action.
            case: "an allow of EKS's own actions",
            policies: |_| vec![statement("Allow", &["eks:*"], "*")],
            refused: Some(no_policy_allows),
        },
        Row {
            case: "an explicit deny of this action beside an allow of every action",
            policies: |_| {
                vec![
                    statement("Allow", &["*"], "*"),
                    statement("Deny", &[ACTION], "*"),
                ]
            },
            refused: Some(|| EXPLICIT_DENY.to_string()),
        },
        Row {
            case: "an explicit deny of this whole service beside an allow of exactly this \
                   action on this cluster",
            policies: |cluster| {
                vec![
                    statement("Allow", &[ACTION], cluster),
                    statement("Deny", &["eks-auth:*"], "*"),
                ]
            },
            refused: Some(|| EXPLICIT_DENY.to_string()),
        },
        Row {
            case: "an explicit deny scoped to this cluster alone",
            policies: |cluster| {
                vec![
                    statement("Allow", &["*"], "*"),
                    statement("Deny", &[ACTION], cluster),
                ]
            },
            refused: Some(|| EXPLICIT_DENY.to_string()),
        },
    ];

    let addressed = cluster_resource(REGION, CLUSTER);
    for (index, row) in rows.iter().enumerate() {
        let aws = Emulator::seeded();
        let caller = assumed_role(
            &aws,
            &format!("pod-identity-caller-{index}"),
            &(row.policies)(&addressed),
        )
        .await;
        let token = aws.issued_token();
        let reply = aws.assume_as(&caller.access_key_id, CLUSTER, &token).await;
        match row.refused {
            Some(reason) => {
                assert_eq!(
                    reply.verdict(),
                    format!("{CLIENT_ERROR_STATUS} {ACCESS_DENIED}"),
                    "case {:?}: expected the modelled authorization refusal, got {}",
                    row.case,
                    reply.body
                );
                assert_eq!(
                    reply.message,
                    not_authorized(&caller.arn, &addressed, &reason()),
                    "case {:?}: expected IAM's sentence, got {:?}",
                    row.case,
                    reply.message
                );
                assert_eq!(
                    aws.sessions(),
                    0,
                    "case {:?}: a refused exchange must mint no session",
                    row.case
                );
            }
            None => {
                assert_eq!(
                    reply.status, 200,
                    "case {:?}: expected an authorized caller to be served, got {}",
                    row.case, reply.body
                );
                let served: Value =
                    serde_json::from_str(&reply.body).unwrap_or(Value::Null);
                assert!(
                    served["podIdentityAssociation"]["associationArn"]
                        .as_str()
                        .is_some(),
                    "case {:?}: expected the stored association in the answer, got {}",
                    row.case,
                    reply.body
                );
                // `assumedRoleUser.arn` is the ARN of the SESSION the exchange vended of
                // the associated role — what AWS reports for an assumed-role identity —
                // so it names that role under the `sts` assumed-role form rather than the
                // `iam` role ARN the association already carries.
                let role_name = ROLE_ARN.rsplit_once('/').map_or(ROLE_ARN, |(_path, name)| name);
                assert!(
                    served["assumedRoleUser"]["arn"]
                        .as_str()
                        .is_some_and(|arn| arn.starts_with(&format!(
                            "arn:aws:sts::000000000000:assumed-role/{role_name}/"
                        ))),
                    "case {:?}: expected a session of the associated role, got {}",
                    row.case,
                    reply.body
                );
                assert_eq!(
                    aws.sessions(),
                    1,
                    "case {:?}: an exchange that was served records its session",
                    row.case
                );
            }
        }
    }
}

/// THE ORDER: the authorization decision comes BEFORE the request's own input is read, so a
/// caller IAM refuses is answered on its permissions whatever it sent — and a caller IAM
/// allows meets the operation's own refusals instead. The resource the refusal names is built
/// from the cluster name the URI carried, even one the modelled pattern refuses.
#[tokio::test]
async fn authorization_is_decided_before_the_requests_input_is_read() {
    let addressed = cluster_resource(REGION, CLUSTER);
    let aws = Emulator::seeded();
    let denied = assumed_role(
        &aws,
        "pod-identity-denied",
        &[
            statement("Allow", &["*"], "*"),
            statement("Deny", &[ACTION], "*"),
        ],
    )
    .await;
    let allowed = assumed_role(&aws, "pod-identity-allowed", &[statement("Allow", &["*"], "*")]).await;
    let token = aws.issued_token();

    // (case, the cluster the URI names, the token, the resource a refusal must name)
    let requests: &[(&str, &str, &str, String)] = &[
        (
            "a request the service would serve",
            CLUSTER,
            token.as_str(),
            addressed.clone(),
        ),
        (
            "a cluster name the modelled pattern refuses, beside a value that is no token",
            MALFORMED_CLUSTER,
            MALFORMED_TOKEN,
            cluster_resource(REGION, MALFORMED_CLUSTER),
        ),
        (
            "a token that is no token, against a cluster that is not registered",
            OTHER_CLUSTER,
            MALFORMED_TOKEN,
            cluster_resource(REGION, OTHER_CLUSTER),
        ),
    ];
    for (case, cluster, sent, resource) in requests {
        let reply = aws.assume_as(&denied.access_key_id, cluster, sent).await;
        assert_eq!(
            reply.verdict(),
            format!("{CLIENT_ERROR_STATUS} {ACCESS_DENIED}"),
            "case {case:?}: expected the refused principal to be answered on its permissions, \
             got {}",
            reply.body
        );
        assert_eq!(
            reply.message,
            not_authorized(&denied.arn, resource, EXPLICIT_DENY),
            "case {case:?}: expected IAM's sentence naming {resource}, got {:?}",
            reply.message
        );
    }

    // The same malformed request from a principal IAM allows is refused on its INPUT, by the
    // modelled constraint error — the layer the decision above runs in front of.
    let reply = aws
        .assume_as(&allowed.access_key_id, MALFORMED_CLUSTER, MALFORMED_TOKEN)
        .await;
    assert_eq!(
        reply.verdict(),
        format!("{CLIENT_ERROR_STATUS} {INVALID_PARAMETER}"),
        "case \"an authorized caller sending a malformed request\": expected the modelled \
         constraint refusal, got {}",
        reply.body
    );
    assert_eq!(
        aws.sessions(),
        0,
        "case \"the refusals above\": none of them may mint a session"
    );
}

/// RULE: the account's own published credential holds no IAM policies — the position AWS
/// account root credentials are in — so a request presenting it is not refused by this
/// decision, and neither is one presenting a key no record resolves.
#[tokio::test]
async fn the_accounts_own_credential_is_not_bounded_by_this_decision() {
    let aws = Emulator::seeded();
    let token = aws.issued_token();
    for (case, access_key_id) in [
        ("the account's own published credential", ACCOUNT_KEY),
        ("a key no record of this emulator resolves", "AKIDNOTAKEY00000"),
    ] {
        let reply = aws.assume_as(access_key_id, CLUSTER, &token).await;
        assert_ne!(
            reply.errortype.as_deref().map(|shape| shape
                .split(':')
                .next()
                .unwrap_or(shape)
                .to_string()),
            Some(ACCESS_DENIED.to_string()),
            "case {case:?}: expected no authorization refusal, got {}",
            reply.body
        );
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the exchange to be served, got {}",
            reply.body
        );
    }
}
