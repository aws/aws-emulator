// SPDX-License-Identifier: CC0-1.0

//! How a cluster's Kubernetes API server AUTHENTICATES a request.
//!
//! The RULE under test: the API server a cluster publishes authenticates every
//! request it serves, and the credential of a request to it is the bearer token the
//! request carries — so a request with no `Authorization` header speaks for nobody
//! and is refused `401 Unauthorized`, carrying the Kubernetes `Status` object for
//! that refusal, before anything is routed, read or written; a token is bound to the
//! CLUSTER it was built for, so one built for another cluster speaks for nobody
//! here; and the identity the server serves an authenticated request as is the one
//! it reports back about the caller. An authenticated request to the same path is
//! served; 403 belongs to a request that authenticated and was then refused by RBAC,
//! never to one that presented nothing.
//!
//! The suites drive that over a TABLE whose rows differ along what the rule
//! generalises over: the path (a server path of its own, a discovery document, an
//! OpenAPI document, a namespaced collection, a cluster-scoped collection, a create
//! in another API group, a path the server does not serve at all), the method (a
//! read and a write), the FORM the missing credential takes (no header at all, a
//! header in another scheme, a token of another version, a token whose segment is
//! not base64url, a token whose URL carries no credential scope, and a token whose
//! URL is not the presigned caller-identity call a credential plugin builds — one
//! calling another action, one signed for another service, one carrying no
//! signature, one whose validity window is past the maximum), and WHICH cluster
//! a presented token names.
//!
//! The client is the raw, kubeconfig-built one `kubernetes_api.rs` dials with.

use crate::kubernetes_api::{
    account_bearer_token, bearer_token_for_cluster, dial, dial_with, ApiAnswer, Kubeconfig,
};
use crate::wire::DEFAULT_ACCESS_KEY_ID;

/// One request a row makes to the published address.
struct RequestCase {
    label: &'static str,
    method: &'static str,
    path: &'static str,
    /// The manifest the request carries, for the rows that write.
    body: Option<serde_json::Value>,
}

/// The requests every form of a missing credential is driven over: what a client
/// addresses on an API server, from the server's own paths to a path it does not
/// serve at all.
fn request_cases() -> Vec<RequestCase> {
    vec![
        RequestCase {
            label: "the server's own version path",
            method: "GET",
            path: "/version",
            body: None,
        },
        RequestCase {
            label: "the core group's discovery document",
            method: "GET",
            path: "/api/v1",
            body: None,
        },
        RequestCase {
            label: "the group list",
            method: "GET",
            path: "/apis",
            body: None,
        },
        RequestCase {
            label: "an OpenAPI document",
            method: "GET",
            path: "/openapi/v2",
            body: None,
        },
        RequestCase {
            label: "a namespaced collection",
            method: "GET",
            path: "/api/v1/namespaces/default/pods",
            body: None,
        },
        RequestCase {
            label: "a cluster-scoped read of one object",
            method: "GET",
            path: "/api/v1/namespaces/kube-system",
            body: None,
        },
        RequestCase {
            label: "a cluster-scoped create",
            method: "POST",
            path: "/api/v1/namespaces",
            body: Some(serde_json::json!({ "metadata": { "name": "anonymous-made-this" } })),
        },
        RequestCase {
            label: "a create in another API group",
            method: "POST",
            path: "/apis/authentication.k8s.io/v1/selfsubjectreviews",
            body: Some(serde_json::json!({
                "apiVersion": "authentication.k8s.io/v1",
                "kind": "SelfSubjectReview",
            })),
        },
        RequestCase {
            label: "a path the server does not serve",
            method: "GET",
            path: "/api/v1/nowhere",
            body: None,
        },
    ]
}

/// The forms a request that presents no credential this server can read takes:
/// the label, and the `Authorization` header it carries — `None` for a request
/// carrying none at all.
fn unauthenticated_forms() -> Vec<(&'static str, Option<String>)> {
    vec![
        ("no Authorization header at all", None),
        (
            "a header in another scheme",
            Some("Basic dXNlcjpwYXNzd29yZA==".to_string()),
        ),
        (
            "a bearer token of another version",
            Some("Bearer k8s-aws-v9.c29tZXRoaW5nLWVsc2U".to_string()),
        ),
        (
            "a bearer token whose segment is not base64url",
            Some("Bearer k8s-aws-v1.not base64url!!".to_string()),
        ),
        (
            "a bearer token carrying no credential scope",
            // The base64url form of a presigned URL with no `X-Amz-Credential`:
            // the token presents a URL that names no credential at all.
            Some(format!(
                "Bearer k8s-aws-v1.{}",
                base64url("https://sts.us-west-2.amazonaws.com/?Action=GetCallerIdentity")
            )),
        ),
        // A token is read as a credential only when it IS the request a credential
        // plugin presigns, so these rows differ along WHICH part of that request the
        // URL is not: the call it makes, the service its scope names, the signature
        // its authorization is completed by, and the window it states.
        (
            "a bearer token whose URL calls another action",
            Some(presented_url(&presigned_query(
                "GetSessionToken",
                "sts",
                "60",
                true,
            ))),
        ),
        (
            "a bearer token signed for another service",
            Some(presented_url(&presigned_query(
                "GetCallerIdentity",
                "execute-api",
                "60",
                true,
            ))),
        ),
        (
            "a bearer token carrying no signature",
            Some(presented_url(&presigned_query(
                "GetCallerIdentity",
                "sts",
                "60",
                false,
            ))),
        ),
        (
            // SOURCE: `X-Amz-Expires`, "the maximum value is 604800 seconds (seven
            // days)" (AWS General Reference, Authenticating requests: using query
            // parameters) — one second past that bound.
            "a bearer token whose validity window is past the maximum",
            Some(presented_url(&presigned_query(
                "GetCallerIdentity",
                "sts",
                "604801",
                true,
            ))),
        ),
    ]
}

/// The `Authorization` header a client presents the URL `query` under: the bearer
/// token of a kubeconfig's credential plugin, carrying that URL.
fn presented_url(query: &str) -> String {
    format!(
        "Bearer k8s-aws-v1.{}",
        base64url(&format!("https://sts.us-west-2.amazonaws.com/?{query}"))
    )
}

/// The query of a presigned call, as a credential plugin writes one: the `action` it
/// calls, the `service` its credential scope names, the `expires` window it is valid
/// for, and whether the SigV4 authorization is completed by a `signature`.
fn presigned_query(action: &str, service: &str, expires: &str, signature: bool) -> String {
    let signature = if signature {
        "&X-Amz-Signature=0000"
    } else {
        ""
    };
    format!(
        "Action={action}&Version=2011-06-15&X-Amz-Algorithm=AWS4-HMAC-SHA256\
         &X-Amz-Credential=AKIDEXAMPLE%2F20260101%2Fus-west-2%2F{service}%2Faws4_request\
         &X-Amz-Date=20260101T000000Z&X-Amz-Expires={expires}\
         &X-Amz-SignedHeaders=host{signature}"
    )
}

/// The base64url form of `text` (the two alphabet substitutions and the dropped
/// padding of RFC 7515 §2), which is how a bearer token carries its presigned URL.
fn base64url(text: &str) -> String {
    aws_smithy_types::base64::encode(text.as_bytes())
        .replace('+', "-")
        .replace('/', "_")
        .replace('=', "")
}

/// Assert `answered` is the `401 Unauthorized` the API server refuses an
/// unauthenticated request with, in the Kubernetes `Status` wording.
fn assert_unauthorized(case: &str, answered: &ApiAnswer) {
    assert_eq!(
        answered.status, 401,
        "case {case:?}: expected 401 Unauthorized, got {answered:?}"
    );
    for (member, expected) in [
        ("kind", serde_json::json!("Status")),
        ("apiVersion", serde_json::json!("v1")),
        ("status", serde_json::json!("Failure")),
        ("reason", serde_json::json!("Unauthorized")),
        ("message", serde_json::json!("Unauthorized")),
        ("code", serde_json::json!(401)),
    ] {
        assert_eq!(
            answered.body[member], expected,
            "case {case:?}: the Status member {member:?} must be {expected:?}, got {:?}",
            answered.body
        );
    }
}

/// RULE: a request presenting no credential this server can read is refused 401,
/// whatever it addresses and however the credential is missing.
#[tokio::test]
async fn a_request_with_no_credential_is_refused_by_the_api_server() {
    let (_router, _engine, config) =
        crate::kubernetes_cluster_resources::published_cluster("anonymous-refused").await;

    for (form, header) in unauthenticated_forms() {
        for request in request_cases() {
            let case = format!("{form} on {}", request.label);
            let answered = dial_with(
                &config,
                &case,
                request.method,
                request.path,
                "application/json",
                header.as_deref(),
                request.body.as_ref(),
            )
            .await;
            assert_unauthorized(&case, &answered);
        }
    }
}

/// RULE: the same requests, presented WITH the account's own credential, are
/// served — so the refusal above is the authentication and not the path.
#[tokio::test]
async fn the_same_requests_are_served_to_a_credential_the_cluster_maps() {
    let (_router, _engine, config) =
        crate::kubernetes_cluster_resources::published_cluster("anonymous-compared").await;

    for request in request_cases() {
        let case = format!("{} with the account's credential", request.label);
        let answered = dial_with(
            &config,
            &case,
            request.method,
            request.path,
            "application/json",
            Some(&account_bearer_token()),
            request.body.as_ref(),
        )
        .await;
        assert_ne!(
            answered.status, 401,
            "case {case:?}: a credential the cluster maps is authenticated, got {answered:?}"
        );
        // The one row that addresses nothing is the path the server does not
        // serve, which is the 404 it always was; every other row is served.
        let expected = match request.path {
            "/api/v1/nowhere" => 404,
            _ if request.method == "POST" => 201,
            _ => 200,
        };
        assert_eq!(
            answered.status, expected,
            "case {case:?}: expected {expected}, got {answered:?}"
        );
    }
}

/// RULE: an unauthenticated request is refused BEFORE anything is written, so it
/// leaves no object behind.
#[tokio::test]
async fn an_unauthenticated_write_stores_nothing() {
    let (_router, _engine, config) =
        crate::kubernetes_cluster_resources::published_cluster("anonymous-writes-nothing").await;
    let namespace = "left-no-trace";

    let refused = dial_with(
        &config,
        "an anonymous namespace create",
        "POST",
        "/api/v1/namespaces",
        "application/json",
        None,
        Some(&serde_json::json!({ "metadata": { "name": namespace } })),
    )
    .await;
    assert_unauthorized("an anonymous namespace create", &refused);

    let read_back = read_namespace(&config, namespace).await;
    assert_eq!(
        read_back.status, 404,
        "case an anonymous namespace create: the refused create must store nothing, got \
         {read_back:?}"
    );
}

/// `GET /api/v1/namespaces/<namespace>` as the account's own credential: the read
/// that says whether an object is there at all.
async fn read_namespace(config: &Kubeconfig, namespace: &str) -> ApiAnswer {
    dial(
        config,
        "the read-back",
        "GET",
        &format!("/api/v1/namespaces/{namespace}"),
        None,
    )
    .await
}

/// RULE: a bearer token is bound to the CLUSTER it was built for — one naming
/// another cluster speaks for nobody here, however good the credential behind it
/// is.
#[tokio::test]
async fn a_token_built_for_another_cluster_is_refused() {
    let cluster = "token-bound-cluster";
    let (_router, _engine, config) =
        crate::kubernetes_cluster_resources::published_cluster(cluster).await;

    // (the case, the cluster the token names, whether the server serves it) — the
    // rows differ along WHICH cluster the credential plugin bound into the token.
    let rows = [
        ("a token bound to this cluster", cluster, true),
        ("a token bound to another cluster", "some-other-cluster", false),
        (
            "a token bound to a cluster of a similar name",
            "token-bound-cluster-2",
            false,
        ),
    ];
    for (case, named, served) in rows {
        let token = bearer_token_for_cluster(DEFAULT_ACCESS_KEY_ID, named);
        let answered = dial_with(
            &config,
            case,
            "GET",
            "/api/v1/namespaces",
            "application/json",
            Some(&token),
            None,
        )
        .await;
        if served {
            assert_eq!(
                answered.status, 200,
                "case {case:?}: a token built for this cluster is authenticated, got {answered:?}"
            );
        } else {
            assert_unauthorized(case, &answered);
        }
    }
}

/// RULE: the identity the server serves an authenticated request as is the one it
/// REPORTS about the caller — the username the cluster's mapping resolved, and the
/// IAM principal the credential spoke for.
#[tokio::test]
async fn the_server_reports_the_identity_it_serves_the_request_as() {
    let (_router, _engine, config) =
        crate::kubernetes_cluster_resources::published_cluster("reported-identity").await;

    let reviewed = dial(
        &config,
        "the identity the server serves the request as",
        "POST",
        "/apis/authentication.k8s.io/v1/selfsubjectreviews",
        Some(&serde_json::json!({
            "apiVersion": "authentication.k8s.io/v1",
            "kind": "SelfSubjectReview",
        })),
    )
    .await;
    assert_eq!(
        reviewed.status, 201,
        "a self subject review must be created, got {reviewed:?}"
    );
    // The IAM principal the dialling credential speaks for, read from the one
    // definition of it rather than written out here.
    let principal_arn = service_common::caller_principal::ambient_user_arn();
    let info = &reviewed.body["status"]["userInfo"];
    assert_eq!(
        info["username"],
        serde_json::json!(principal_arn),
        "the cluster's creator authenticates under its own principal, got {reviewed:?}"
    );
    // (the member of `extra` the authenticating authority reports, what it carries)
    // — the IAM principal the credential spoke for, and the session it was minted
    // for, which a long-term credential names as empty.
    for (member, expected) in [
        ("arn", serde_json::json!([principal_arn])),
        ("canonicalArn", serde_json::json!([principal_arn])),
        ("sessionName", serde_json::json!([""])),
    ] {
        assert_eq!(
            info["extra"][member], expected,
            "case {member:?}: expected {expected:?} in the reported identity, got {reviewed:?}"
        );
    }
    assert!(
        info["groups"]
            .as_array()
            .is_some_and(|groups| groups.contains(&serde_json::json!("system:authenticated"))),
        "an authenticated identity is in the authenticated group, got {reviewed:?}"
    );
}
