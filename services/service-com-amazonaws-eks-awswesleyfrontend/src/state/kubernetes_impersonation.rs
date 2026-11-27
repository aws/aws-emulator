// SPDX-License-Identifier: CC0-1.0

//! The identity a request asks to be served AS: `Impersonate-User` and the
//! `Impersonate-Group` headers beside it.
//!
//! "A user can act as another user through impersonation headers. These let requests
//! manually override the user info a request authenticates as." A client checking what
//! an identity may do — an operator verifying a Role, `kubectl --as`, a controller
//! acting for the account a workload runs as — sends its OWN credential and names the
//! identity the request is to be served as; a server that ignores those headers serves
//! the request as the REQUESTER, so a check of what the named identity may do answers
//! what the requester may do instead, and every refusal such a check expects is
//! missing.
//!
//! Impersonating is itself something an identity must be permitted to do, and it is
//! permitted by the cluster's own RBAC objects like anything else: the `impersonate`
//! verb on the `users` resource for a user, on the `serviceaccounts` resource of a
//! namespace for a service account of it, and on the `groups` resource for each group
//! the request asks to carry — all in the core API group, named by the value asked
//! for. An identity whose rules do not admit that is refused the authorizer's
//! `403 Forbidden` naming the REQUESTER, and the request is not served at all.
//!
//! Once it is admitted, the request is decided and answered as the impersonated
//! identity and nothing else: the groups are the ones the headers name, else — for a
//! service account — the groups Kubernetes puts a service account in, and
//! `system:authenticated` beside them, because "the impersonated user is
//! authenticated". No access policy of the requester's own access entry travels with
//! it: an EKS access policy is associated to the ENTRY of an IAM principal
//! ([`super::kubernetes_rbac`]), and the identity being acted as is not that
//! principal, so what it may do is the cluster's own RBAC objects and the published
//! permissions of its groups.

use super::kubernetes_objects::{ApiRefusal, ClusterObjects};
use super::kubernetes_rbac::{self as rbac, RequestAttributes, SubjectIdentity};
use super::kubernetes_service_account_tokens::service_account_identity;

/// The headers a request names the identity it asks to be served as in.
pub const IMPERSONATE_USER_HEADER: &str = "impersonate-user";
pub const IMPERSONATE_GROUP_HEADER: &str = "impersonate-group";

/// The verb impersonating is, as an RBAC rule names it.
const IMPERSONATE_VERB: &str = "impersonate";

/// How many groups ONE request may ask to carry.
///
/// CHOSEN, not published: Kubernetes bounds a request's headers by their size alone,
/// and every group named here is authorized on its own against the cluster's
/// bindings — so this is what keeps one request's decision a bounded product rather
/// than one the caller sizes. It is far above the handful an impersonating client
/// names.
pub const MAX_IMPERSONATED_GROUPS: usize = 32;

/// The resources an impersonation is authorized against: a user, a service account of
/// a namespace, and a group.
const USERS_RESOURCE: &str = "users";
const SERVICE_ACCOUNTS_RESOURCE: &str = "serviceaccounts";
const GROUPS_RESOURCE: &str = "groups";

/// The prefix the username of a SERVICE ACCOUNT carries, which is what says that an
/// impersonation is authorized against the service accounts of a namespace rather
/// than against the cluster's users.
const SERVICE_ACCOUNT_PREFIX: &str = "system:serviceaccount:";

/// What one request asks to be served as.
#[derive(Debug, Clone, Copy)]
pub struct Impersonation<'request> {
    /// The `Impersonate-User` header: the identity the request is to be served as.
    pub user: &'request str,
    /// The `Impersonate-Group` headers: the groups that identity is to carry.
    pub groups: &'request [&'request str],
}

/// What the request asks to be served as, and `None` for one that asks for nothing —
/// which is served as the identity its own credential authenticated.
#[must_use]
pub fn requested<'request>(
    user: Option<&'request str>,
    groups: &'request [&'request str],
) -> Option<Impersonation<'request>> {
    let user = user.map(str::trim).filter(|user| !user.is_empty())?;
    Some(Impersonation { user, groups })
}

/// The identity the request is served as when `identity` asks to act as `requested`,
/// or the `403 Forbidden` the authorizer refuses an identity that may not act as it
/// with.
///
/// Every value the request asks to carry is authorized on its own — the user and each
/// group — so an identity permitted to act as a user but not to claim a group is
/// refused rather than served with the group dropped.
pub fn impersonated(
    objects: &ClusterObjects,
    identity: &SubjectIdentity,
    requested: &Impersonation<'_>,
) -> Result<SubjectIdentity, ApiRefusal> {
    if requested.groups.len() > MAX_IMPERSONATED_GROUPS {
        return Err(ApiRefusal::bad_request(format!(
            "a request may not ask to carry more than {MAX_IMPERSONATED_GROUPS} groups"
        )));
    }
    let mut asked_for = vec![user_attributes(requested.user)];
    for group in requested.groups {
        asked_for.push(attributes(GROUPS_RESOURCE, String::new(), (*group).to_string()));
    }
    for attributes in &asked_for {
        if !rbac::admits(objects, identity, attributes) {
            return Err(attributes.forbidden(&identity.username));
        }
    }
    Ok(acting_identity(requested))
}

/// The identity a request admitted to act as `requested` is served as.
fn acting_identity(requested: &Impersonation<'_>) -> SubjectIdentity {
    let named: Vec<String> = requested
        .groups
        .iter()
        .map(|group| (*group).to_string())
        .collect();
    // A service account's own identity, so the username and the groups of one are
    // read from the one place that states them
    // ([`super::kubernetes_service_account_tokens`]).
    let mut identity = match service_account_of(requested.user) {
        Some((namespace, name)) => service_account_identity(namespace, name),
        None => SubjectIdentity {
            username: requested.user.to_string(),
            groups: SubjectIdentity::authenticated_groups(&named),
            ..SubjectIdentity::default()
        },
    };
    // The groups the headers NAME replace the ones the identity carries by default:
    // "the groups the user is a member of" is what the request states, and a request
    // that states none leaves the identity its own.
    if !named.is_empty() {
        identity.groups = SubjectIdentity::authenticated_groups(&named);
    }
    identity
}

/// The `(namespace, name)` of the service account `user` names, and `None` for a
/// username that is not a service account's.
fn service_account_of(user: &str) -> Option<(&str, &str)> {
    user.strip_prefix(SERVICE_ACCOUNT_PREFIX)?.split_once(':')
}

/// What impersonating the user `user` addresses: the service accounts of its namespace
/// for a service account's username, and the cluster's users otherwise.
fn user_attributes(user: &str) -> RequestAttributes {
    match service_account_of(user) {
        Some((namespace, name)) => attributes(
            SERVICE_ACCOUNTS_RESOURCE,
            namespace.to_string(),
            name.to_string(),
        ),
        None => attributes(USERS_RESOURCE, String::new(), user.to_string()),
    }
}

/// The request the authorizer decides one impersonated value by: the `impersonate`
/// verb on `resource`, in the core API group, naming the value asked for.
fn attributes(resource: &str, namespace: String, name: String) -> RequestAttributes {
    RequestAttributes {
        verb: IMPERSONATE_VERB.to_string(),
        resource: resource.to_string(),
        namespace,
        name,
        ..RequestAttributes::default()
    }
}
