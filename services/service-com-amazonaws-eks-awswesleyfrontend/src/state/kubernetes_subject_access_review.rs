// SPDX-License-Identifier: CC0-1.0

//! `authorization.k8s.io/v1`: what a client asks the API server it is ALLOWED to
//! do.
//!
//! A client cannot tell from its credential what the cluster will let it do — the
//! decision is the cluster's, taken from the access policies its access entry
//! associates and the RBAC objects it holds ([`super::kubernetes_rbac`]) — so it
//! asks the API server. There are two questions, and the group serves a resource
//! for each:
//!
//! * MAY I do this? A `SelfSubjectAccessReview`:
//!   "SelfSubjectAccessReview checks whether or not the current user can perform an
//!   action. Not filling in a spec.namespace means 'in all namespaces'", and the
//!   answer's `status.allowed` is that decision. It is what `kubectl auth can-i`
//!   sends, and what any client that pre-flights its own permissions sends.
//! * WHAT may I do here? A `SelfSubjectRulesReview`, which "enumerates the set of
//!   actions the current user can perform within a namespace" — the answer is the
//!   rules themselves, which is what `kubectl auth can-i --list` prints. A cluster
//!   whose access entries are served by an authorizer that answers one request at a
//!   time cannot enumerate THAT authorizer's rules, so the listing reports itself
//!   `incomplete` there ([`super::kubernetes_rbac::RulesListing`]).
//!
//! Both are answered from the SAME state the authorizer decides a request by
//! ([`super::kubernetes_rbac::admits`] over
//! [`super::kubernetes_rbac::RequestAttributes`], and
//! [`super::kubernetes_rbac::effective_rules`] over the same two sources of
//! rules), so neither can disagree with what the server would do with the request
//! it describes. Nothing is stored: a review is computed for the request that
//! asked, which is why Kubernetes answers it `201 Created` with a null creation
//! timestamp and no name.
//!
//! Kubernetes lets EVERY authenticated client make both requests: the
//! `system:basic-user` ClusterRole — "allows a user read-only access to basic
//! information about themselves" — is bound to `system:authenticated` and carries
//! `selfsubjectaccessreviews` and `selfsubjectrulesreviews`, so the permission is
//! one of the published ones every cluster has
//! ([`super::kubernetes_rbac::published_group_rules`]) rather than an object a
//! caller has to create.

use super::kubernetes_objects::ClusterObjects;
use super::kubernetes_rbac::{
    admits, effective_rules, EffectiveRule, RequestAttributes, SubjectIdentity,
};

/// The API group a client asks about its own permissions in.
pub const AUTHORIZATION_API_GROUP: &str = "authorization.k8s.io";

/// The version of that group this API server serves.
pub const AUTHORIZATION_GROUP_VERSION: &str = "v1";

/// The `apiVersion` every object of the group carries.
pub const AUTHORIZATION_API_VERSION: &str = "authorization.k8s.io/v1";

/// The resource a client creates to be told whether it may do something.
pub const SELF_SUBJECT_ACCESS_REVIEWS_RESOURCE: &str = "selfsubjectaccessreviews";

/// The resource a client creates to be told everything it may do in a namespace.
pub const SELF_SUBJECT_RULES_REVIEWS_RESOURCE: &str = "selfsubjectrulesreviews";

/// The `kind` of that resource's object.
const SELF_SUBJECT_ACCESS_REVIEW_KIND: &str = "SelfSubjectAccessReview";

/// The `kind` of the rules review's object.
const SELF_SUBJECT_RULES_REVIEW_KIND: &str = "SelfSubjectRulesReview";

/// The `apiVersion` a discovery document itself carries, which is the core one
/// whatever group it describes.
const DISCOVERY_API_VERSION: &str = "v1";

/// The `status.reason` the review carries for a decision the authorizer took on the
/// permissions the identity holds, which is the sentence Kubernetes' RBAC authorizer
/// words an allow in.
const ALLOWED_BY_RBAC: &str = "RBAC: allowed by the permissions of the requesting user";

/// One `GroupVersionForDiscovery` of this group.
fn group_version() -> ::serde_json::Value {
    ::serde_json::json!({
        "groupVersion": AUTHORIZATION_API_VERSION,
        "version": AUTHORIZATION_GROUP_VERSION,
    })
}

/// The group's entry in the `APIGroupList` served at `/apis`.
#[must_use]
pub fn api_group_entry() -> ::serde_json::Value {
    ::serde_json::json!({
        "name": AUTHORIZATION_API_GROUP,
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
        "name": AUTHORIZATION_API_GROUP,
        "versions": [group_version()],
        "preferredVersion": group_version(),
    })
}

/// The `APIResourceList` of `authorization.k8s.io/v1`: the resources this server
/// serves in the group and what may be done to each.
#[must_use]
pub fn api_resource_list_document() -> ::serde_json::Value {
    ::serde_json::json!({
        "kind": "APIResourceList",
        "apiVersion": DISCOVERY_API_VERSION,
        "groupVersion": AUTHORIZATION_API_VERSION,
        "resources": [
            {
                "name": SELF_SUBJECT_ACCESS_REVIEWS_RESOURCE,
                "singularName": "selfsubjectaccessreview",
                "namespaced": false,
                "kind": SELF_SUBJECT_ACCESS_REVIEW_KIND,
                "verbs": ["create"],
            },
            // A rules review is not namespaced either: the namespace it is about is
            // a member of its own spec rather than a part of its path.
            {
                "name": SELF_SUBJECT_RULES_REVIEWS_RESOURCE,
                "singularName": "selfsubjectrulesreview",
                "namespaced": false,
                "kind": SELF_SUBJECT_RULES_REVIEW_KIND,
                "verbs": ["create"],
            },
        ],
    })
}

/// The `SelfSubjectAccessReview` the API server answers: the review the client sent,
/// with the decision the authorizer took on the action its spec describes.
///
/// The decision is taken for `identity` — who the request is being served as — over
/// `objects`, the cluster's own RBAC objects, through the authorizer that admits
/// every other request on this address. A review whose spec describes no action at
/// all (neither `resourceAttributes` nor `nonResourceAttributes`) is refused the way
/// the API server refuses it: there is nothing to decide.
#[must_use]
pub fn self_subject_access_review(
    objects: &ClusterObjects,
    identity: &SubjectIdentity,
    review: &::serde_json::Value,
    attributes: &RequestAttributes,
) -> ::serde_json::Value {
    let allowed = admits(objects, identity, attributes);
    let spec = review
        .get("spec")
        .cloned()
        .unwrap_or_else(|| ::serde_json::json!({}));
    let mut status = ::serde_json::Map::new();
    status.insert("allowed".to_string(), ::serde_json::json!(allowed));
    // "Denied is true if the action would be denied, otherwise false": the
    // authorizer of this server either allows an action or holds no rule that allows
    // it, which is not the same as an explicit deny.
    status.insert("denied".to_string(), ::serde_json::json!(false));
    // "Reason is optional. It indicates why a request was allowed or denied": the
    // authorizer states the permissions that ALLOWED an action, and has nothing to
    // say about one no rule of it allowed — a client renders the reason after the
    // decision it reports, so a sentence on a denial would be read as the authorizer
    // having given one.
    if allowed {
        status.insert("reason".to_string(), ::serde_json::json!(ALLOWED_BY_RBAC));
    }
    ::serde_json::json!({
        "kind": SELF_SUBJECT_ACCESS_REVIEW_KIND,
        "apiVersion": AUTHORIZATION_API_VERSION,
        "metadata": { "creationTimestamp": ::serde_json::Value::Null },
        // "Spec holds information about the request being evaluated": echoed back as
        // the client wrote it, which is what the API server answers with.
        "spec": spec,
        "status": ::serde_json::Value::Object(status),
    })
}

/// What the action a review's spec describes ADDRESSES, read into the same
/// attributes the authorizer decides a request by, or `None` for a spec that
/// describes no action.
///
/// "ResourceAuthorizationAttributes describes information for a resource access
/// request" — `group`, `resource`, `subresource`, `namespace`, `name` and `verb` —
/// and `NonResourceAttributes` describes one by `path` and `verb`. Both are read
/// here into [`RequestAttributes`], the one reading the authorizer takes a decision
/// over, so a review and the request it describes are decided alike. A spec that
/// names neither is nothing to decide.
#[must_use]
pub fn reviewed_attributes(review: &::serde_json::Value) -> Option<RequestAttributes> {
    let spec = review.get("spec")?;
    let member = |attributes: &::serde_json::Value, name: &str| -> String {
        attributes
            .get(name)
            .and_then(::serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    if let Some(resource) = spec.get("resourceAttributes") {
        return Some(RequestAttributes {
            verb: member(resource, "verb"),
            api_group: member(resource, "group"),
            resource: member(resource, "resource"),
            subresource: member(resource, "subresource"),
            namespace: member(resource, "namespace"),
            name: member(resource, "name"),
            // A resource review addresses a resource rather than a path of the
            // server's own, so it names none.
            path: String::new(),
        });
    }
    let non_resource = spec.get("nonResourceAttributes")?;
    Some(RequestAttributes {
        verb: member(non_resource, "verb"),
        path: member(non_resource, "path"),
        ..RequestAttributes::default()
    })
}

/// The message the API server refuses a review whose spec describes no action with:
/// "exactly one of nonResourceAttributes or resourceAttributes must be specified".
pub const NO_ATTRIBUTES_MESSAGE: &str =
    "spec.resourceAttributes: Required value: exactly one of nonResourceAttributes or \
     resourceAttributes must be specified";

/// The sentence the API server refuses a RULES review that names no namespace
/// with.
///
/// "Namespace to evaluate rules for. Required"
/// (`SelfSubjectRulesReviewSpec$namespace`): the rules a client may exercise are
/// per namespace, so a review that names none asks about nothing and is a bad
/// request rather than an empty answer.
pub const NO_NAMESPACE_MESSAGE: &str = "no namespace on the request";

/// The namespace a rules review asks about, and `None` for one that names none.
#[must_use]
pub fn reviewed_namespace(review: &::serde_json::Value) -> Option<&str> {
    review
        .get("spec")?
        .get("namespace")?
        .as_str()
        .filter(|namespace| !namespace.is_empty())
}

/// The `SelfSubjectRulesReview` the API server answers: the rules `identity` may
/// exercise in `namespace`, as the cluster's own state puts them in force
/// ([`effective_rules`]).
///
/// "ResourceRules is the list of actions the subject is allowed to perform on
/// resources" and "NonResourceRules is the list of actions the subject is allowed
/// to perform on non-resources", so each collected rule is reported under the one
/// it is about — a rule carrying both is reported under both, and one carrying
/// neither constrains nothing and is reported under neither.
///
/// `access_entry_authorizer` is whether the authorizer Amazon EKS serves the
/// cluster's access entries by is in its chain: its rules cannot be enumerated, so
/// a listing taken while it is in force leaves them out and reports itself
/// `incomplete`, which is what tells a client the listing is not the whole of what
/// it may do.
#[must_use]
pub fn self_subject_rules_review(
    objects: &ClusterObjects,
    identity: &SubjectIdentity,
    review: &::serde_json::Value,
    namespace: &str,
    access_entry_authorizer: bool,
) -> ::serde_json::Value {
    let listing = effective_rules(objects, identity, namespace, access_entry_authorizer);
    let resource_rules: Vec<::serde_json::Value> = listing
        .rules
        .iter()
        .filter(|rule| rule.is_resource_rule())
        .map(resource_rule_json)
        .collect();
    let non_resource_rules: Vec<::serde_json::Value> = listing
        .rules
        .iter()
        .filter(|rule| rule.is_non_resource_rule())
        .map(non_resource_rule_json)
        .collect();
    let spec = review
        .get("spec")
        .cloned()
        .unwrap_or_else(|| ::serde_json::json!({}));
    let mut status = ::serde_json::Map::new();
    status.insert(
        "resourceRules".to_string(),
        ::serde_json::json!(resource_rules),
    );
    status.insert(
        "nonResourceRules".to_string(),
        ::serde_json::json!(non_resource_rules),
    );
    status.insert(
        "incomplete".to_string(),
        ::serde_json::json!(listing.incomplete),
    );
    // "EvaluationError can appear in combination with Rules": the error the
    // authorizer that could not be enumerated answered with. A listing every
    // authorizer answered carries none, which is how the API server reports one.
    if let Some(error) = listing.evaluation_error {
        status.insert("evaluationError".to_string(), ::serde_json::json!(error));
    }
    ::serde_json::json!({
        "kind": SELF_SUBJECT_RULES_REVIEW_KIND,
        "apiVersion": AUTHORIZATION_API_VERSION,
        "metadata": { "creationTimestamp": ::serde_json::Value::Null },
        "spec": spec,
        "status": ::serde_json::Value::Object(status),
    })
}

/// One `ResourceRule`: what the identity may do, to which resources of which API
/// groups, and — for a rule that names some — to which objects by name. A member
/// the rule constrains nothing by is left out, which is how the API server reports
/// one.
fn resource_rule_json(rule: &EffectiveRule) -> ::serde_json::Value {
    let mut reported = ::serde_json::Map::new();
    reported.insert("verbs".to_string(), ::serde_json::json!(rule.verbs));
    for (member, values) in [
        ("apiGroups", &rule.api_groups),
        ("resources", &rule.resources),
        ("resourceNames", &rule.resource_names),
    ] {
        if !values.is_empty() {
            reported.insert(member.to_string(), ::serde_json::json!(values));
        }
    }
    ::serde_json::Value::Object(reported)
}

/// One `NonResourceRule`: what the identity may do to which paths of the server's
/// own.
fn non_resource_rule_json(rule: &EffectiveRule) -> ::serde_json::Value {
    ::serde_json::json!({
        "verbs": rule.verbs,
        "nonResourceURLs": rule.non_resource_urls,
    })
}
