// SPDX-License-Identifier: CC0-1.0

//! The requests a cluster's own SERVICE ACCOUNT tokens make to its Kubernetes API
//! server, and the deletes that API server serves.
//!
//! The RULE under test, in two halves.
//!
//! A Kubernetes API server authenticates a bearer token its own cluster issued for a
//! service account: the request is served as `system:serviceaccount:<namespace>:<name>`,
//! in the groups Kubernetes puts every service account in, and what it may do is
//! decided by the cluster's RBAC objects over exactly those names — so a token the
//! cluster issued can be used at the cluster that issued it, and the refusal an
//! identity whose role lacks the verb gets is the authorizer's `403 Forbidden` with
//! the apiserver's sentence rather than a transport-level answer. A token no cluster
//! issued, one another cluster issued, and one issued for another consumer's audience
//! each authenticate nobody, which is `401 Unauthorized`.
//!
//! And every resource that API server serves serves the DELETE verb its own resource
//! row advertises: one object is given up and answered with the `Success` `Status` a
//! delete is answered with, a name the cluster does not hold is the not-found
//! `Status`, and a NAMESPACE — whose delete is a delete of everything inside it — is
//! answered with the namespace in the phase a namespace being torn down is in, and is
//! gone once that teardown is due.
//!
//! Each half drives its rule over a TABLE whose rows differ along what it generalises
//! over: for the identity, which account the token was issued for, whether a role is
//! bound to it, whether the request addresses a collection or one object, which verb
//! it asks for and why a token names nobody; for the deletes, which resource of the
//! served catalog is addressed and whether the object is there at all.
//!
//! The client is the raw, kubeconfig-built one [`crate::kubernetes_api::dial`] dials
//! with.

use std::time::{Duration, Instant};

use serde_json::json;
use service_com_amazonaws_eks_awswesleyfrontend::state::kubernetes_service_account_tokens::API_SERVER_AUDIENCES;

use crate::certificate_authority::REGION;
use crate::kubernetes_api::{
    account_bearer_token, dial, dial_bytes_with_headers, dial_with, emulator_with_pod_identity,
    provisioned_cluster, ApiAnswer, Kubeconfig, PODS_AUDIENCE,
};

/// The namespace the rows address, which the client creates.
const NAMESPACE: &str = "sa-team";

/// The service account a `Role` grants the read verbs to, and the one nothing binds a
/// role to.
const READER: &str = "reader";
const UNBOUND: &str = "stranger";

/// The ConfigMap and the Pod the rows address.
const CONFIG_MAP: &str = "settings";
const POD: &str = "worker";

/// How long a suite waits for a clock-owed teardown, and how often it looks again:
/// far above the span the service owes it, so the deadline is reached only when the
/// move is not coming at all.
const TEARDOWN_DEADLINE: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The username Kubernetes serves the service account `name` of `namespace` as.
fn service_account_username(namespace: &str, name: &str) -> String {
    format!("system:serviceaccount:{namespace}:{name}")
}

/// The `Authorization` header a client holding the service account token `token`
/// presents.
fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// Mint a token for the service account `name` of `namespace` through the subresource
/// that mints one, as the cluster's administrator, and answer the whole
/// `TokenRequest`.
///
/// `audiences` is what the request asks for, and `None` asks for none — which is the
/// request a client that wants a token for the API SERVER makes.
async fn request_token(
    config: &Kubeconfig,
    case: &str,
    namespace: &str,
    name: &str,
    audiences: Option<&[&str]>,
) -> ApiAnswer {
    let body = match audiences {
        Some(audiences) => json!({ "spec": { "audiences": audiences } }),
        None => json!({ "spec": {} }),
    };
    dial(
        config,
        case,
        "POST",
        &format!("/api/v1/namespaces/{namespace}/serviceaccounts/{name}/token"),
        Some(&body),
    )
    .await
}

/// The token value of a minted `TokenRequest`, which is what a client presents.
fn minted_token(case: &str, answered: &ApiAnswer) -> String {
    assert_eq!(
        answered.status, 201,
        "case {case:?}: the token subresource must mint a token, got {answered:?}"
    );
    answered.body["status"]["token"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the minted TokenRequest must carry a token, got {answered:?}")
        })
        .to_string()
}

/// A JSON Web Token NO cluster issued: the shape a service account token has, with a
/// key id no cluster published and a signature of nothing.
///
/// Written out here rather than taken from the code that reads one, so the value this
/// client presents is built the way a holder of a stolen token would build it.
fn fabricated_token() -> String {
    let segment = |bytes: &[u8]| {
        aws_smithy_types::base64::encode(bytes)
            .replace('+', "-")
            .replace('/', "_")
            .replace('=', "")
    };
    format!(
        "{}.{}.{}",
        segment(json!({ "alg": "RS256", "kid": "not-a-published-key", "typ": "JWT" }).to_string().as_bytes()),
        segment(
            json!({
                "iss": "https://oidc.eks.us-west-2.amazonaws.com/id/FABRICATED",
                "sub": service_account_username(NAMESPACE, READER),
            })
            .to_string()
            .as_bytes()
        ),
        segment(b"not-a-signature"),
    )
}

/// The sentence the API server refuses a request RBAC does not admit with, as the
/// authorizer words one: the resource it addressed, the identity it was served as, the
/// verb, the API group (empty for the core one) and the namespace.
fn forbidden_sentence(
    resource: &str,
    name: Option<&str>,
    username: &str,
    verb: &str,
    namespace: &str,
) -> String {
    let addressed = match name {
        Some(name) => format!("{resource} \"{name}\""),
        None => resource.to_string(),
    };
    format!(
        "{addressed} is forbidden: User \"{username}\" cannot {verb} resource \"{resource}\" \
         in API group \"\" in the namespace \"{namespace}\""
    )
}

/// One row of the identity table: the token presented, the request made, and the
/// answer the API server owes it.
struct IdentityCase {
    label: &'static str,
    token: String,
    method: &'static str,
    path: String,
    status: u16,
    /// The sentence the answer's `Status` carries, where the answer is a refusal.
    message: Option<String>,
}

#[tokio::test]
async fn a_token_the_cluster_issued_is_served_as_its_service_account() {
    let (router, _engine) = emulator_with_pod_identity();
    let config = provisioned_cluster(&router, REGION, "sa-token-identity").await;
    // A SECOND cluster, whose tokens are its own: a token it issued names nobody at
    // the cluster above.
    let other = provisioned_cluster(&router, REGION, "sa-token-elsewhere").await;

    // The namespace, the two service accounts and the objects the rows address.
    for (case, path, body) in [
        (
            "the namespace",
            "/api/v1/namespaces".to_string(),
            json!({ "metadata": { "name": NAMESPACE } }),
        ),
        (
            "the bound service account",
            format!("/api/v1/namespaces/{NAMESPACE}/serviceaccounts"),
            json!({ "metadata": { "name": READER } }),
        ),
        (
            "the unbound service account",
            format!("/api/v1/namespaces/{NAMESPACE}/serviceaccounts"),
            json!({ "metadata": { "name": UNBOUND } }),
        ),
        (
            "the ConfigMap the role may read",
            format!("/api/v1/namespaces/{NAMESPACE}/configmaps"),
            json!({ "metadata": { "name": CONFIG_MAP }, "data": { "log-level": "debug" } }),
        ),
        (
            "the pod the role may read",
            format!("/api/v1/namespaces/{NAMESPACE}/pods"),
            json!({
                "metadata": { "name": POD },
                "spec": {
                    "containers": [{
                        "name": "app",
                        "image": "public.ecr.aws/docker/library/nginx:1.27",
                    }],
                },
            }),
        ),
        (
            "the role that grants the read verbs",
            format!("/apis/rbac.authorization.k8s.io/v1/namespaces/{NAMESPACE}/roles"),
            json!({
                "metadata": { "name": "read-only" },
                "rules": [{
                    "apiGroups": [""],
                    "resources": ["pods", "configmaps"],
                    "verbs": ["get", "list"],
                }],
            }),
        ),
        (
            "the binding that binds it to the reader account",
            format!("/apis/rbac.authorization.k8s.io/v1/namespaces/{NAMESPACE}/rolebindings"),
            json!({
                "metadata": { "name": "read-only" },
                "roleRef": {
                    "apiGroup": "rbac.authorization.k8s.io",
                    "kind": "Role",
                    "name": "read-only",
                },
                "subjects": [{
                    "kind": "ServiceAccount",
                    "name": READER,
                    "namespace": NAMESPACE,
                }],
            }),
        ),
    ] {
        let created = dial(&config, case, "POST", &path, Some(&body)).await;
        assert_eq!(
            created.status, 201,
            "setup {case:?}: expected the object to be created, got {created:?}"
        );
    }

    // A `TokenRequest` that asks for no audience is served with the API server's own
    // audience, which is the audience the API server then authenticates it for.
    let reader = request_token(&config, "the reader's token", NAMESPACE, READER, None).await;
    assert_eq!(
        reader.body["spec"]["audiences"],
        json!([API_SERVER_AUDIENCES[0]]),
        "a token asked for with no audience is served with the API server's own, got {reader:?}"
    );
    let reader_token = minted_token("the reader's token", &reader);
    let unbound_token = minted_token(
        "the unbound account's token",
        &request_token(&config, "the unbound account's token", NAMESPACE, UNBOUND, None).await,
    );
    let projected_token = minted_token(
        "a token projected for the pod identity audience",
        &request_token(
            &config,
            "a token projected for the pod identity audience",
            NAMESPACE,
            READER,
            Some(&[PODS_AUDIENCE]),
        )
        .await,
    );
    let elsewhere_token = minted_token(
        "a token the other cluster issued",
        &request_token(
            &other,
            "a token the other cluster issued",
            "default",
            "default",
            None,
        )
        .await,
    );

    let reader_username = service_account_username(NAMESPACE, READER);
    let unbound_username = service_account_username(NAMESPACE, UNBOUND);
    let pods = format!("/api/v1/namespaces/{NAMESPACE}/pods");
    let pod = format!("{pods}/{POD}");
    let config_map = format!("/api/v1/namespaces/{NAMESPACE}/configmaps/{CONFIG_MAP}");
    let unauthorized = Some("Unauthorized".to_string());

    // The rows differ along what the rule generalises over: which account the token
    // names, whether a role is bound to it, whether the request addresses a collection
    // or one object, the verb it asks for, and why a presented token names nobody.
    let cases = vec![
        IdentityCase {
            label: "the bound account listing the pods of its namespace",
            token: reader_token.clone(),
            method: "GET",
            path: pods.clone(),
            status: 200,
            message: None,
        },
        IdentityCase {
            label: "the bound account reading one pod",
            token: reader_token.clone(),
            method: "GET",
            path: pod.clone(),
            status: 200,
            message: None,
        },
        IdentityCase {
            label: "the bound account reading one ConfigMap",
            token: reader_token.clone(),
            method: "GET",
            path: config_map.clone(),
            status: 200,
            message: None,
        },
        IdentityCase {
            label: "the bound account deleting a pod, which its role does not grant",
            token: reader_token.clone(),
            method: "DELETE",
            path: pod.clone(),
            status: 403,
            message: Some(forbidden_sentence(
                "pods",
                Some(POD),
                &reader_username,
                "delete",
                NAMESPACE,
            )),
        },
        IdentityCase {
            label: "the bound account deleting a ConfigMap, which its role does not grant",
            token: reader_token.clone(),
            method: "DELETE",
            path: config_map.clone(),
            status: 403,
            message: Some(forbidden_sentence(
                "configmaps",
                Some(CONFIG_MAP),
                &reader_username,
                "delete",
                NAMESPACE,
            )),
        },
        IdentityCase {
            label: "an account nothing binds a role to, listing a collection",
            token: unbound_token.clone(),
            method: "GET",
            path: pods.clone(),
            status: 403,
            message: Some(forbidden_sentence(
                "pods",
                None,
                &unbound_username,
                "list",
                NAMESPACE,
            )),
        },
        IdentityCase {
            label: "a token issued for another consumer's audience",
            token: projected_token,
            method: "GET",
            path: pods.clone(),
            status: 401,
            message: unauthorized.clone(),
        },
        IdentityCase {
            label: "a token another cluster issued",
            token: elsewhere_token,
            method: "GET",
            path: pods.clone(),
            status: 401,
            message: unauthorized.clone(),
        },
        IdentityCase {
            label: "a token no cluster issued",
            token: fabricated_token(),
            method: "GET",
            path: pods.clone(),
            status: 401,
            message: unauthorized.clone(),
        },
        IdentityCase {
            label: "a token whose characters were changed",
            token: format!("{reader_token}tampered"),
            method: "GET",
            path: pods.clone(),
            status: 401,
            message: unauthorized,
        },
    ];

    for case in &cases {
        let label = case.label;
        let answered = dial_with(
            &config,
            label,
            case.method,
            &case.path,
            "application/json",
            Some(&bearer(&case.token)),
            None,
        )
        .await;
        assert_eq!(
            answered.status, case.status,
            "case {label:?}: expected {} from {} {}, got {answered:?}",
            case.status, case.method, case.path
        );
        if let Some(message) = &case.message {
            assert_eq!(
                answered.body["message"],
                json!(message),
                "case {label:?}: expected the sentence the API server words it in, got {:?}",
                answered.body
            );
            assert_eq!(
                answered.body["kind"],
                json!("Status"),
                "case {label:?}: a refusal is a Status object, got {:?}",
                answered.body
            );
        }
    }

    // And a token the cluster issued for an account it NO LONGER HOLDS: deleting the
    // account is deleting the identity, so what the token names is gone with it.
    let deleted = dial(
        &config,
        "a token whose service account was deleted",
        "DELETE",
        &format!("/api/v1/namespaces/{NAMESPACE}/serviceaccounts/{UNBOUND}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "a token whose service account was deleted: the account must be given up, got \
         {deleted:?}"
    );
    let orphaned = dial_with(
        &config,
        "a token whose service account was deleted",
        "GET",
        &pods,
        "application/json",
        Some(&bearer(&unbound_token)),
        None,
    )
    .await;
    assert_eq!(
        orphaned.status, 401,
        "a token whose service account was deleted: the identity it names is gone, so it \
         authenticates nobody, got {orphaned:?}"
    );

    // The identity the API server itself reports for the token: the username and the
    // groups Kubernetes puts a service account in, and no authenticating-authority
    // information, because no IAM principal spoke for the request.
    let reviewed = dial_with(
        &config,
        "the identity the server reports",
        "POST",
        "/apis/authentication.k8s.io/v1/selfsubjectreviews",
        "application/json",
        Some(&bearer(&reader_token)),
        Some(&json!({ "apiVersion": "authentication.k8s.io/v1", "kind": "SelfSubjectReview" })),
    )
    .await;
    assert_eq!(
        reviewed.status, 201,
        "the identity the server reports: every authenticated client may ask, got {reviewed:?}"
    );
    assert_eq!(
        reviewed.body["status"]["userInfo"]["username"],
        json!(reader_username),
        "the identity the server reports: expected the service account's username, got {:?}",
        reviewed.body
    );
    let groups: Vec<String> = reviewed.body["status"]["userInfo"]["groups"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .filter_map(|group| group.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    for expected in [
        "system:authenticated".to_string(),
        "system:serviceaccounts".to_string(),
        format!("system:serviceaccounts:{NAMESPACE}"),
    ] {
        assert!(
            groups.contains(&expected),
            "the identity the server reports: expected the group {expected:?}, got {groups:?}"
        );
    }
    assert!(
        reviewed.body["status"]["userInfo"]["extra"].is_null(),
        "the identity the server reports: no IAM principal authenticated this request, so \
         there is no authority information to report, got {:?}",
        reviewed.body
    );

    // And the decision the same authorizer takes, as a client asks for it: the verb the
    // bound role grants is allowed and the one it does not is denied.
    for (verb, allowed) in [("get", true), ("delete", false)] {
        let review = dial_with(
            &config,
            verb,
            "POST",
            "/apis/authorization.k8s.io/v1/selfsubjectaccessreviews",
            "application/json",
            Some(&bearer(&reader_token)),
            Some(&json!({
                "apiVersion": "authorization.k8s.io/v1",
                "kind": "SelfSubjectAccessReview",
                "spec": {
                    "resourceAttributes": {
                        "namespace": NAMESPACE,
                        "verb": verb,
                        "resource": "pods",
                    },
                },
            })),
        )
        .await;
        assert_eq!(
            review.status, 201,
            "case {verb:?}: a client may ask what it is allowed, got {review:?}"
        );
        assert_eq!(
            review.body["status"]["allowed"],
            json!(allowed),
            "case {verb:?}: expected allowed={allowed}, got {:?}",
            review.body
        );
        // A client renders the reason after the decision it reports, so a denial the
        // authorizer holds no rule for states none at all.
        assert_eq!(
            review.body["status"]["reason"].is_null(),
            !allowed,
            "case {verb:?}: expected a reason only where the action is allowed, got {:?}",
            review.body
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a request that names the identity it asks to be SERVED AS is decided and
// answered as that identity, once the requester's own rules admit acting as it.
// ---------------------------------------------------------------------------

/// One row of the impersonation table: who asks, who it asks to act as, the request
/// it makes, and the answer it owes.
struct ActingCase {
    label: &'static str,
    /// The credential the request presents, which is the requester.
    authorization: String,
    /// The `Impersonate-User` header, and the `Impersonate-Group` headers beside it.
    user: String,
    groups: Vec<String>,
    method: &'static str,
    path: String,
    status: u16,
    message: Option<String>,
}

#[tokio::test]
async fn a_request_is_served_as_the_identity_it_asks_to_act_as() {
    let (router, _engine) = emulator_with_pod_identity();
    let config = provisioned_cluster(&router, REGION, "sa-token-acting-as").await;

    let namespace = "acting-team";
    let group = "acting-editors";
    for (case, path, body) in [
        (
            "the namespace",
            "/api/v1/namespaces".to_string(),
            json!({ "metadata": { "name": namespace } }),
        ),
        (
            "the service account the rows act as",
            format!("/api/v1/namespaces/{namespace}/serviceaccounts"),
            json!({ "metadata": { "name": READER } }),
        ),
        (
            "the ConfigMap the rows address",
            format!("/api/v1/namespaces/{namespace}/configmaps"),
            json!({ "metadata": { "name": CONFIG_MAP }, "data": { "log-level": "debug" } }),
        ),
        (
            "the pod the rows address",
            format!("/api/v1/namespaces/{namespace}/pods"),
            json!({
                "metadata": { "name": POD },
                "spec": {
                    "containers": [{
                        "name": "app",
                        "image": "public.ecr.aws/docker/library/nginx:1.27",
                    }],
                },
            }),
        ),
        (
            "the role that grants the read verbs",
            format!("/apis/rbac.authorization.k8s.io/v1/namespaces/{namespace}/roles"),
            json!({
                "metadata": { "name": "read-only" },
                "rules": [{
                    "apiGroups": [""],
                    "resources": ["pods", "configmaps"],
                    "verbs": ["get", "list"],
                }],
            }),
        ),
        (
            "the binding that binds it to the service account",
            format!("/apis/rbac.authorization.k8s.io/v1/namespaces/{namespace}/rolebindings"),
            json!({
                "metadata": { "name": "read-only" },
                "roleRef": {
                    "apiGroup": "rbac.authorization.k8s.io",
                    "kind": "Role",
                    "name": "read-only",
                },
                "subjects": [{
                    "kind": "ServiceAccount",
                    "name": READER,
                    "namespace": namespace,
                }],
            }),
        ),
        (
            "the role that may change a ConfigMap",
            format!("/apis/rbac.authorization.k8s.io/v1/namespaces/{namespace}/roles"),
            json!({
                "metadata": { "name": "editor" },
                "rules": [{
                    "apiGroups": [""],
                    "resources": ["configmaps"],
                    "verbs": ["delete"],
                }],
            }),
        ),
        (
            "the binding that binds it to a GROUP",
            format!("/apis/rbac.authorization.k8s.io/v1/namespaces/{namespace}/rolebindings"),
            json!({
                "metadata": { "name": "editor" },
                "roleRef": {
                    "apiGroup": "rbac.authorization.k8s.io",
                    "kind": "Role",
                    "name": "editor",
                },
                "subjects": [{ "kind": "Group", "name": group }],
            }),
        ),
    ] {
        let created = dial(&config, case, "POST", &path, Some(&body)).await;
        assert_eq!(
            created.status, 201,
            "setup {case:?}: expected the object to be created, got {created:?}"
        );
    }

    // The service account's own token, so a row can ask as an identity that is NOT
    // permitted to act as anybody.
    let reader_token = minted_token(
        "the service account's own token",
        &request_token(
            &config,
            "the service account's own token",
            namespace,
            READER,
            None,
        )
        .await,
    );
    let administrator = account_bearer_token();
    let reader_username = service_account_username(namespace, READER);
    let pods = format!("/api/v1/namespaces/{namespace}/pods");
    let pod = format!("{pods}/{POD}");
    let config_map = format!("/api/v1/namespaces/{namespace}/configmaps/{CONFIG_MAP}");

    // The rows differ along what the rule generalises over: whether the identity acted
    // as is a service account or a user, whether the request also claims groups, which
    // verb it asks for, and whether the REQUESTER may act as it at all.
    let cases = vec![
        ActingCase {
            label: "acting as a service account a role is bound to, for a verb it grants",
            authorization: administrator.clone(),
            user: reader_username.clone(),
            groups: Vec::new(),
            method: "GET",
            path: pods.clone(),
            status: 200,
            message: None,
        },
        ActingCase {
            label: "acting as that service account for a verb its role does not grant",
            authorization: administrator.clone(),
            user: reader_username.clone(),
            groups: Vec::new(),
            method: "DELETE",
            path: pod.clone(),
            status: 403,
            message: Some(forbidden_sentence(
                "pods",
                Some(POD),
                &reader_username,
                "delete",
                namespace,
            )),
        },
        ActingCase {
            label: "acting as a user nothing binds a role to",
            authorization: administrator.clone(),
            user: "nobody".to_string(),
            groups: Vec::new(),
            method: "GET",
            path: pods.clone(),
            status: 403,
            message: Some(forbidden_sentence(
                "pods",
                None,
                "nobody",
                "list",
                namespace,
            )),
        },
        ActingCase {
            label: "acting as that user in a GROUP a role is bound to",
            authorization: administrator,
            user: "nobody".to_string(),
            groups: vec![group.to_string()],
            method: "DELETE",
            path: config_map,
            status: 200,
            message: None,
        },
        ActingCase {
            label: "a requester whose own rules do not let it act as anybody",
            authorization: bearer(&reader_token),
            user: "nobody".to_string(),
            groups: Vec::new(),
            method: "GET",
            path: pods.clone(),
            status: 403,
            message: Some(format!(
                "users \"nobody\" is forbidden: User \"{reader_username}\" cannot impersonate \
                 resource \"users\" in API group \"\" at the cluster scope"
            )),
        },
    ];

    for case in &cases {
        let label = case.label;
        let mut headers: Vec<(&str, &str)> = vec![("Impersonate-User", case.user.as_str())];
        for group in &case.groups {
            headers.push(("Impersonate-Group", group.as_str()));
        }
        let answered = dial_bytes_with_headers(
            &config,
            label,
            case.method,
            &case.path,
            "application/json",
            Some(&case.authorization),
            &headers,
            &[],
        )
        .await;
        assert_eq!(
            answered.status, case.status,
            "case {label:?}: expected {} from {} {}, got {answered:?}",
            case.status, case.method, case.path
        );
        if let Some(message) = &case.message {
            assert_eq!(
                answered.body["message"],
                json!(message),
                "case {label:?}: expected the sentence the API server words it in, got {:?}",
                answered.body
            );
        }
    }

    // The identity the API server reports for an impersonating request is the one it
    // was asked to act as, not the requester.
    let reviewed = dial_bytes_with_headers(
        &config,
        "the identity the server reports",
        "POST",
        "/apis/authentication.k8s.io/v1/selfsubjectreviews",
        "application/json",
        Some(&account_bearer_token()),
        &[
            ("Impersonate-User", reader_username.as_str()),
            ("Impersonate-Group", "acting-auditors"),
        ],
        json!({ "apiVersion": "authentication.k8s.io/v1", "kind": "SelfSubjectReview" })
            .to_string()
            .as_bytes(),
    )
    .await;
    assert_eq!(
        reviewed.status, 201,
        "the identity the server reports: the review must be served, got {reviewed:?}"
    );
    assert_eq!(
        reviewed.body["status"]["userInfo"]["username"],
        json!(reader_username),
        "the identity the server reports: expected the identity acted as, got {:?}",
        reviewed.body
    );
    let groups: Vec<String> = reviewed.body["status"]["userInfo"]["groups"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .filter_map(|group| group.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    for expected in ["acting-auditors", "system:authenticated"] {
        assert!(
            groups.contains(&expected.to_string()),
            "the identity the server reports: expected the group {expected:?} the request \
             asked to carry, got {groups:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the DELETE verb the served resource list advertises gives the object up —
// answered with the `Success` `Status` a delete is answered with, the not-found
// `Status` for a name the cluster does not hold, and, for a namespace, with the
// namespace in the phase a namespace being torn down is in.
// ---------------------------------------------------------------------------

/// One row of the delete table: the resource addressed, the manifest that creates the
/// object, and where it lives.
struct DeleteCase {
    label: &'static str,
    resource: &'static str,
    collection: String,
    object_path: String,
    missing_path: String,
    manifest: serde_json::Value,
}

#[tokio::test]
async fn deleting_an_object_the_server_serves_gives_it_up() {
    let (router, _engine) = emulator_with_pod_identity();
    let config = provisioned_cluster(&router, REGION, "sa-token-deletes").await;

    let namespace = "delete-team";
    let created = dial(
        &config,
        "the namespace the objects live in",
        "POST",
        "/api/v1/namespaces",
        Some(&json!({ "metadata": { "name": namespace } })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "setup: expected the namespace to be created, got {created:?}"
    );

    // The rows differ along which resource of the served catalog is addressed and what
    // an object of it carries.
    let cases = vec![
        DeleteCase {
            label: "a ConfigMap",
            resource: "configmaps",
            collection: format!("/api/v1/namespaces/{namespace}/configmaps"),
            object_path: format!("/api/v1/namespaces/{namespace}/configmaps/settings"),
            missing_path: format!("/api/v1/namespaces/{namespace}/configmaps/no-such-map"),
            manifest: json!({ "metadata": { "name": "settings" }, "data": { "a": "b" } }),
        },
        DeleteCase {
            label: "a service account",
            resource: "serviceaccounts",
            collection: format!("/api/v1/namespaces/{namespace}/serviceaccounts"),
            object_path: format!("/api/v1/namespaces/{namespace}/serviceaccounts/builder"),
            missing_path: format!("/api/v1/namespaces/{namespace}/serviceaccounts/no-such-sa"),
            manifest: json!({ "metadata": { "name": "builder" } }),
        },
        DeleteCase {
            label: "a Service",
            resource: "services",
            collection: format!("/api/v1/namespaces/{namespace}/services"),
            object_path: format!("/api/v1/namespaces/{namespace}/services/front"),
            missing_path: format!("/api/v1/namespaces/{namespace}/services/no-such-service"),
            manifest: json!({
                "metadata": { "name": "front" },
                "spec": { "selector": { "app": "front" }, "ports": [{ "port": 80 }] },
            }),
        },
    ];

    for case in &cases {
        let label = case.label;
        let created = dial(&config, label, "POST", &case.collection, Some(&case.manifest)).await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: expected the object to be created, got {created:?}"
        );
        let deleted = dial(&config, label, "DELETE", &case.object_path, None).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: the delete the resource advertises must be served at {:?}, got \
             {deleted:?}",
            case.object_path
        );
        assert_eq!(
            deleted.body["status"],
            json!("Success"),
            "case {label:?}: a delete is answered with the Success Status, got {:?}",
            deleted.body
        );
        assert_eq!(
            deleted.body["details"]["kind"],
            json!(case.resource),
            "case {label:?}: the Status names what was given up, got {:?}",
            deleted.body
        );
        // BACKED: the object is gone, so a read of it is the not-found Status the API
        // server words, and a second delete is the same answer.
        for (what, path) in [
            ("the object that was deleted", &case.object_path),
            ("a name the cluster never held", &case.missing_path),
        ] {
            let read = dial(&config, label, "GET", path, None).await;
            assert_eq!(
                read.status, 404,
                "case {label:?}: {what} must read back as not found, got {read:?}"
            );
            let again = dial(&config, label, "DELETE", path, None).await;
            assert_eq!(
                again.status, 404,
                "case {label:?}: deleting {what} must answer the not-found Status, got {again:?}"
            );
            let name = path.rsplit('/').next().unwrap_or_default();
            assert_eq!(
                again.body["message"],
                json!(format!("{} \"{name}\" not found", case.resource)),
                "case {label:?}: expected the sentence the API server words it in, got {:?}",
                again.body
            );
        }
    }

    // A POD is given up the same way, and the object a delete answers with is the pod
    // it deleted rather than a `Status`.
    let pod = format!("/api/v1/namespaces/{namespace}/pods/worker");
    let created = dial(
        &config,
        "a pod",
        "POST",
        &format!("/api/v1/namespaces/{namespace}/pods"),
        Some(&json!({
            "metadata": { "name": "worker" },
            "spec": {
                "containers": [{
                    "name": "app",
                    "image": "public.ecr.aws/docker/library/nginx:1.27",
                }],
            },
        })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case \"a pod\": expected the pod to be created, got {created:?}"
    );
    let deleted = dial(&config, "a pod", "DELETE", &pod, None).await;
    assert_eq!(
        deleted.status, 200,
        "case \"a pod\": the delete must be served, got {deleted:?}"
    );
    assert_eq!(
        deleted.body["kind"],
        json!("Pod"),
        "case \"a pod\": a delete of one pod answers the pod it deleted, got {:?}",
        deleted.body
    );
    assert!(
        deleted.body["metadata"]["deletionTimestamp"]
            .as_str()
            .is_some_and(|stamp| !stamp.is_empty()),
        "case \"a pod\": the answered object carries the instant it was deleted, got {:?}",
        deleted.body
    );
    let read = dial(&config, "a pod", "GET", &pod, None).await;
    assert_eq!(
        read.status, 404,
        "case \"a pod\": the deleted pod must be gone, got {read:?}"
    );

    // And a NAMESPACE: the delete is answered with the namespace in the phase a
    // namespace being torn down is in, and the namespace and everything inside it are
    // given up once that teardown is due. The wait is for the CONDITION, re-read until
    // it holds or a generous deadline passes.
    let path = format!("/api/v1/namespaces/{namespace}");
    let deleted = dial(&config, "the namespace", "DELETE", &path, None).await;
    assert_eq!(
        deleted.status, 200,
        "case \"the namespace\": the delete must be served, got {deleted:?}"
    );
    assert_eq!(
        deleted.body["kind"],
        json!("Namespace"),
        "case \"the namespace\": a delete of a namespace answers the namespace, got {:?}",
        deleted.body
    );
    assert_eq!(
        deleted.body["status"]["phase"],
        json!("Terminating"),
        "case \"the namespace\": expected the phase a namespace being torn down is in, got {:?}",
        deleted.body
    );
    assert!(
        deleted.body["metadata"]["deletionTimestamp"]
            .as_str()
            .is_some_and(|stamp| !stamp.is_empty()),
        "case \"the namespace\": the namespace carries the instant it was deleted, got {:?}",
        deleted.body
    );
    // A namespace that is going away takes no new content: the create is refused with
    // the API server's own sentence while the namespace is still being torn down, and
    // with the not-found `Status` once it has been given up — never served. Which of
    // the two a host reaches depends on how fast it ran the teardown the clock owes,
    // so both are read as the refusal they are and the sentence is pinned on the one
    // that names it.
    let refused = dial(
        &config,
        "a create in a namespace being torn down",
        "POST",
        &format!("/api/v1/namespaces/{namespace}/configmaps"),
        Some(&json!({ "metadata": { "name": "too-late" } })),
    )
    .await;
    assert!(
        refused.status == 403 || refused.status == 404,
        "case \"a create in a namespace being torn down\": a create addressed at a namespace a \
         delete has reached is refused, got {refused:?}"
    );
    if refused.status == 403 {
        assert_eq!(
            refused.body["message"],
            json!(format!(
                "configmaps \"too-late\" is forbidden: unable to create new content in \
                 namespace {namespace} because it is being terminated"
            )),
            "case \"a create in a namespace being torn down\": expected the sentence the API \
             server words it in, got {:?}",
            refused.body
        );
    } else {
        assert_eq!(
            refused.body["message"],
            json!(format!("namespaces \"{namespace}\" not found")),
            "case \"a create in a namespace being torn down\": the namespace has been given \
             up, so the create is the not-found Status, got {:?}",
            refused.body
        );
    }
    let deadline = Instant::now() + TEARDOWN_DEADLINE;
    loop {
        let read = dial(&config, "the namespace", "GET", &path, None).await;
        if read.status == 404 {
            break;
        }
        assert_eq!(
            read.body["status"]["phase"],
            json!("Terminating"),
            "case \"the namespace\": a namespace a delete reached stays in that phase until it \
             is given up, got {read:?}"
        );
        assert!(
            Instant::now() < deadline,
            "case \"the namespace\": the clock owes the teardown of the namespace, got {read:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let listed = dial(
        &config,
        "the namespace",
        "GET",
        &format!("/api/v1/namespaces/{namespace}/configmaps"),
        None,
    )
    .await;
    assert_eq!(
        listed.status, 404,
        "case \"the namespace\": the objects inside a namespace go with it, got {listed:?}"
    );
}
