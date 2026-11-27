// SPDX-License-Identifier: CC0-1.0

//! `authentication.k8s.io/v1`: what a client asks the API server about ITSELF.
//!
//! A client that holds a credential cannot tell from the credential alone which
//! Kubernetes identity the cluster serves it as — the mapping is the cluster's
//! (an access entry, or its `aws-auth` ConfigMap: see
//! [`super::kubernetes_authentication`]) — so it asks the API server, by creating
//! a `SelfSubjectReview`: "SelfSubjectReview contains the user information that
//! the kube-apiserver has about the user making this request", and the answer's
//! `status.userInfo` is that identity, nothing from the request's own text.
//!
//! Kubernetes lets EVERY authenticated client make that request: the
//! `system:basic-user` ClusterRole — "allows a user read-only access to basic
//! information about themselves" — is bound to `system:authenticated`, which is
//! why the permission is one of the published ones every cluster has
//! ([`super::kubernetes_rbac::published_group_rules`]) rather than an object a
//! caller has to create.
//!
//! The group's other resource is the `TokenRequest` a service account's `token`
//! subresource mints ([`super::kubernetes_objects`]), which is addressed under
//! the service account rather than here.

use super::kubernetes_objects::AUTHENTICATION_API_VERSION;
use super::kubernetes_rbac::SubjectIdentity;

/// The API group a client asks about itself in.
pub const AUTHENTICATION_API_GROUP: &str = "authentication.k8s.io";

/// The version of that group this API server serves.
pub const AUTHENTICATION_GROUP_VERSION: &str = "v1";

/// The resource a client creates to be told who it is.
pub const SELF_SUBJECT_REVIEWS_RESOURCE: &str = "selfsubjectreviews";

/// The `kind` of that resource's object.
const SELF_SUBJECT_REVIEW_KIND: &str = "SelfSubjectReview";

/// The `apiVersion` a discovery document itself carries, which is the core one
/// whatever group it describes.
const DISCOVERY_API_VERSION: &str = "v1";

/// One `GroupVersionForDiscovery` of this group.
fn group_version() -> ::serde_json::Value {
    ::serde_json::json!({
        "groupVersion": AUTHENTICATION_API_VERSION,
        "version": AUTHENTICATION_GROUP_VERSION,
    })
}

/// The group's entry in the `APIGroupList` served at `/apis`.
#[must_use]
pub fn api_group_entry() -> ::serde_json::Value {
    ::serde_json::json!({
        "name": AUTHENTICATION_API_GROUP,
        "versions": [group_version()],
        "preferredVersion": group_version(),
    })
}

/// The `APIGroup` discovery document of the group, which a client reads before it
/// addresses it.
#[must_use]
pub fn api_group_document() -> ::serde_json::Value {
    ::serde_json::json!({
        "kind": "APIGroup",
        "apiVersion": DISCOVERY_API_VERSION,
        "name": AUTHENTICATION_API_GROUP,
        "versions": [group_version()],
        "preferredVersion": group_version(),
    })
}

/// The `APIResourceList` of `authentication.k8s.io/v1`: the resource this server
/// serves in the group and what may be done to it.
#[must_use]
pub fn api_resource_list_document() -> ::serde_json::Value {
    ::serde_json::json!({
        "kind": "APIResourceList",
        "apiVersion": DISCOVERY_API_VERSION,
        "groupVersion": AUTHENTICATION_API_VERSION,
        "resources": [{
            "name": SELF_SUBJECT_REVIEWS_RESOURCE,
            "singularName": "selfsubjectreview",
            "namespaced": false,
            "kind": SELF_SUBJECT_REVIEW_KIND,
            "verbs": ["create"],
        }],
    })
}

/// The `SelfSubjectReview` the API server answers `identity` with: the user
/// information it has about the client that made the request.
///
/// The object carries no name — a self subject review is not stored, it is computed
/// for the request that asked — which is what Kubernetes reports as a null creation
/// timestamp. The identity itself is [`user_info`].
#[must_use]
pub fn self_subject_review(identity: &SubjectIdentity) -> ::serde_json::Value {
    ::serde_json::json!({
        "kind": SELF_SUBJECT_REVIEW_KIND,
        "apiVersion": AUTHENTICATION_API_VERSION,
        "metadata": { "creationTimestamp": ::serde_json::Value::Null },
        "status": { "userInfo": user_info(identity) },
    })
}

/// The prefix the authenticating authority of an Amazon EKS cluster stamps on the
/// `uid` of every identity it authenticates: the authenticator the cluster maps IAM
/// principals to Kubernetes subjects with, naming itself as the authority the uid is
/// unique within.
const AUTHENTICATOR_UID_PREFIX: &str = "aws-iam-authenticator";

/// The `UserInfo` the API server reports about an authenticated identity.
///
/// The username and the groups are the ones the cluster's mapping resolved for the
/// credential the request presented, so this answer and the identity every other
/// answer on this address is decided under are one value.
///
/// The `uid` is what Kubernetes distinguishes two subjects that share a username by,
/// and the authenticating authority is what mints it: on Amazon EKS it is the
/// authenticator's own name, the ACCOUNT the authenticated principal belongs to and
/// that principal's UNIQUE ID, joined by colons — all three facts of the credential
/// that authenticated the request and of the account's own records, never of the
/// request's text. The uid's last segment is therefore the very id `extra.principalId`
/// carries: one value, written once here.
///
/// Beside them the `extra` map carries what the AUTHENTICATING authority knows about
/// the identity, which on Amazon EKS is the IAM principal the credential spoke for:
/// `arn` and `canonicalArn` name that principal, `principalId` is its unique id, and
/// `sessionName` the assumed-role session the credential was minted for — empty for a
/// long-term credential, which names none. Nothing of the CREDENTIAL itself is
/// reported.
///
/// Both are reported exactly when an IAM principal authenticated the request. A
/// request a service account token of the cluster's own authenticated
/// ([`super::kubernetes_service_account_tokens`]) was authenticated by the cluster
/// itself, and one served as the identity it asked to act as
/// ([`super::kubernetes_impersonation`]) by neither — so there is no authority to
/// report for it, and an empty ARN with a uid built from one would name a principal
/// that does not exist.
#[must_use]
pub fn user_info(identity: &SubjectIdentity) -> ::serde_json::Value {
    let mut info = ::serde_json::Map::new();
    info.insert(
        "username".to_string(),
        ::serde_json::json!(identity.username),
    );
    info.insert(
        "groups".to_string(),
        ::serde_json::json!(identity.groups.iter().collect::<Vec<&String>>()),
    );
    if !identity.principal_arn.is_empty() {
        info.insert("uid".to_string(), ::serde_json::json!(subject_uid(identity)));
        info.insert(
            "extra".to_string(),
            ::serde_json::json!({
                "arn": [identity.principal_arn],
                "canonicalArn": [identity.principal_arn],
                "principalId": [identity.principal_id],
                "sessionName": [identity.session_name.clone().unwrap_or_default()],
            }),
        );
    }
    ::serde_json::Value::Object(info)
}

/// The `uid` of [`user_info`]: the authenticator's name, the account the principal
/// belongs to — read off that principal's own ARN, the one account reading of this
/// service — and the principal's unique id.
fn subject_uid(identity: &SubjectIdentity) -> String {
    format!(
        "{AUTHENTICATOR_UID_PREFIX}:{}:{}",
        super::access_entry_usernames::arn_account_id(&identity.principal_arn),
        identity.principal_id,
    )
}
