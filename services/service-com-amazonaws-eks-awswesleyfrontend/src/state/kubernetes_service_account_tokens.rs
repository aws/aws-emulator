// SPDX-License-Identifier: CC0-1.0

//! The SERVICE ACCOUNT token a client presents to a cluster's Kubernetes API server,
//! and the identity such a request is served as.
//!
//! An IAM principal reaches a cluster with the `k8s-aws-v1.` token its credential
//! plugin builds ([`super::kubernetes_authentication`]), but that is not the only
//! credential a Kubernetes API server authenticates: a cluster also issues tokens of
//! its OWN, through the `token` subresource of a service account
//! ([`super::kubernetes_objects::ClusterObjects::plan_token`]), and a client holding
//! one — the pod it was projected into, `kubectl` configured with it, a controller
//! running under a service account — presents it as a bearer token like any other.
//! A server that reads only the AWS form answers such a request `401 Unauthorized`,
//! so the identity a cluster issued a token for can do nothing on the cluster that
//! issued it, and an RBAC decision about a service account can never be reached.
//!
//! What the token NAMES is the record of the issue, never anything the presented
//! value says about itself: the token is resolved against the registry of what this
//! cluster's API server issued — which lives on the plane that answers for a
//! presented token, so there is one registry rather than a copy on each side
//! ([`super::pod_identity_plane`]) — and a value that registry does not hold names
//! nobody however well-formed it is. A token is bound to the cluster that issued it
//! and to the window it was issued for, and the AUDIENCE it was issued to has to be
//! one this API server is itself a consumer of ([`API_SERVER_AUDIENCES`]): a token
//! projected for another consumer authenticates there and not here.
//!
//! The Kubernetes identity of a service account is not a mapping anyone creates: the
//! username is `system:serviceaccount:<namespace>:<name>` and the groups are the ones
//! Kubernetes puts every service account in — `system:serviceaccounts`, the group of
//! all of them, and `system:serviceaccounts:<namespace>`, the group of the ones in
//! its namespace — beside `system:authenticated`. So what such a request may do is
//! decided by the cluster's own RBAC objects over exactly those names
//! ([`super::kubernetes_rbac`]).

use ::service_com_amazonaws_eksauth_eksauthfrontend::state::parse_service_account_token;
use ::std::time::SystemTime;

use super::kubernetes_rbac::{service_account_username, SubjectIdentity};
use super::pod_identity_plane::AuthPlane;

/// The scheme a bearer token is presented under in the `Authorization` header.
const BEARER_SCHEME: &str = "Bearer ";

/// The audiences this API server is a consumer of: the names a client inside the
/// cluster reaches the API server by, which is what a token meant for the API server
/// is issued to.
///
/// The first is the one a `TokenRequest` that asks for no audience is served with —
/// "if no audiences are specified, the audience will default to the audience of the
/// API server" — and the second is the same Service under the cluster's own DNS
/// domain. A token issued to any other audience is one another consumer checks, so
/// it authenticates nothing here.
pub const API_SERVER_AUDIENCES: &[&str] = &[
    "https://kubernetes.default.svc",
    "https://kubernetes.default.svc.cluster.local",
];

/// The group every service account is in, and the group of the service accounts of
/// ONE namespace — the two Kubernetes puts a service account's requests in beside
/// `system:authenticated`.
const SERVICE_ACCOUNTS_GROUP: &str = "system:serviceaccounts";

/// The token one request PRESENTS, and `None` for a request that presents no bearer
/// token at all.
///
/// Only the scheme is read here: whether the value is a token this cluster issued is
/// [`issued_service_account`]'s question, and whether it is the AWS form is
/// [`super::kubernetes_authentication::presented_token`]'s.
#[must_use]
pub fn presented_bearer_token(authorization: Option<&str>) -> Option<&str> {
    authorization?
        .strip_prefix(BEARER_SCHEME)
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

/// The service account `token` was ISSUED for at the cluster `cluster_name`, as the
/// `(namespace, name)` pair naming it, and `None` for a token that was issued for
/// nobody there.
///
/// A token names an account only when the cluster that issued it has the record of
/// having done so: it names this cluster's signing key, this cluster issued the value
/// itself, it was issued to an audience this API server consumes, and `now` lies
/// inside the window it was issued for. A token another cluster issued, one whose
/// characters were changed, one issued for another consumer and one whose window has
/// passed each name nobody.
///
/// Whether the cluster still HOLDS that account is the caller's read, because the
/// accounts are this service's own objects: an account that has been deleted is an
/// identity the cluster no longer has, so a token issued for it authenticates nobody
/// either — [`super::kubernetes_api`] serves a request as the account only while the
/// cluster holds it.
///
/// Reading only: nothing is issued, moved or pruned here — presenting a token is not
/// what spends it. Call it with no store of THIS service held: it takes the lock of
/// the plane the registry lives on.
#[must_use]
pub fn issued_service_account(
    plane: Option<&AuthPlane>,
    cluster_name: &str,
    token: &str,
    now: SystemTime,
) -> Option<(String, String)> {
    // The JOSE header of the presented value, read with the one reader of this
    // workspace: a value that is not a JSON Web Token at all is no service account
    // token, and a token naming a key this cluster does not publish is not one this
    // cluster signed.
    let headers = parse_service_account_token(token).ok()?;
    let store = plane?.lock();
    let cluster = store.cluster(cluster_name)?;
    if !cluster.publishes_signing_key(&headers.key_id) {
        return None;
    }
    let issued = cluster.issued_tokens.get(token)?;
    if !issued
        .audience
        .iter()
        .any(|audience| API_SERVER_AUDIENCES.contains(&audience.as_str()))
    {
        return None;
    }
    // A token is not yet a token before the instant it was issued at, and no longer
    // one once its window has passed.
    if now < issued.issued_at || issued.expires_at <= now {
        return None;
    }
    Some((issued.namespace.clone(), issued.service_account.clone()))
}

/// The identity Kubernetes serves the service account `service_account` of
/// `namespace` as.
///
/// The ONE statement of what that identity is, so a request a token of the account
/// authenticated and one admitted to ACT as the account
/// ([`super::kubernetes_impersonation`]) are served as the same identity.
#[must_use]
pub fn service_account_identity(namespace: &str, service_account: &str) -> SubjectIdentity {
    SubjectIdentity {
        username: service_account_username(namespace, service_account),
        groups: SubjectIdentity::authenticated_groups(&[
            SERVICE_ACCOUNTS_GROUP.to_string(),
            format!("{SERVICE_ACCOUNTS_GROUP}:{namespace}"),
        ]),
        // A service account is the cluster's own identity: no IAM principal spoke for
        // the request and no access entry was consulted, so there is no access policy
        // to carry and no principal to name.
        ..SubjectIdentity::default()
    }
}
