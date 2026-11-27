// SPDX-License-Identifier: CC0-1.0

//! The `rbac.authorization.k8s.io/v1` objects one cluster's API server holds, and
//! the DECISION they make about a request.
//!
//! A client that builds a kubeconfig out of `Cluster$endpoint` and
//! `Certificate$data` talks to a Kubernetes API server, and the group that decides
//! what it may do there is this one: `Role` and `ClusterRole` carry the rules,
//! `RoleBinding` and `ClusterRoleBinding` bind them to a user or a group, and every
//! request is admitted or refused by the rules bound to the identity that made it.
//! So the group is SERVED — its discovery documents, its four resources, and the
//! authorizer over them — rather than answered as a path the server does not have.
//!
//! Two sources of rules meet here, and both are the cluster's own state:
//!
//! * the objects a client creates through this API ([`ClusterObjects::cluster_rbac`]
//!   and [`super::kubernetes_objects::NamespaceObject::rbac`]), evaluated exactly as
//!   RBAC evaluates them — a binding's subjects against the identity, its `roleRef`
//!   against the role it names, and the role's rules against what the request
//!   addresses;
//! * the EKS ACCESS POLICIES the cluster's access entries associate
//!   ([`access_policy_rules`]): "Amazon EKS access policies include Kubernetes
//!   permissions" and each of the general-purpose ones grants the permissions of the
//!   Kubernetes user-facing role it is published as the equivalent of, over the whole
//!   cluster or over the namespaces the association scopes it to.
//!
//! Nothing is derived from the request's own text: a rule matches because the cluster
//! holds an object or an association that says so.
//!
//! The parts, each in a file of its own: this root carries the group's shapes and its
//! discovery documents, [`request_attributes`] reads what one request addresses,
//! [`stored_objects`] holds the objects a caller creates and walks the bindings over
//! them, [`published_roles`] carries the roles and access policies every cluster
//! has without an object of its own, and [`effective_rules`] collects the rules both
//! sources put in force for one identity — which is what a client is answered when
//! it asks what it may do rather than whether it may do one thing.

use super::kubernetes_objects::{
    deleted_status_json, manifest_name, new_object_uid, rfc3339_seconds, ApiRefusal, ClusterObjects,
};
use ::std::collections::{BTreeMap, BTreeSet};
use ::std::time::SystemTime;
mod effective_rules;
mod published_roles;
mod request_attributes;
// `stored_objects` adds methods to `ClusterObjects` and names nothing of its own that
// another module reads, so it is a module without a re-export: the methods are the
// type's wherever the type is in scope.
mod stored_objects;
pub use effective_rules::*;
pub use published_roles::*;
pub use request_attributes::*;

/// The API group the RBAC objects live in.
pub const RBAC_API_GROUP: &str = "rbac.authorization.k8s.io";

/// The version of that group this API server serves, which is the one every RBAC
/// object a client sends carries in its `apiVersion`.
pub const RBAC_GROUP_VERSION: &str = "v1";

/// `<group>/<version>`, the `apiVersion` every object of the group carries.
pub const RBAC_API_VERSION: &str = "rbac.authorization.k8s.io/v1";

/// The `apiVersion` a discovery document itself carries, which is the core one
/// whatever group it describes.
const DISCOVERY_API_VERSION: &str = "v1";

/// How many objects of ONE RBAC resource one scope holds — a namespace for `Role`
/// and `RoleBinding`, the cluster for `ClusterRole` and `ClusterRoleBinding`.
///
/// CHOSEN, not published: Kubernetes puts no limit on RBAC objects and a caller can
/// create them without end, so the store needs a bound of its own. It is far above
/// the handful a cluster of this emulator is driven with.
pub const MAX_RBAC_OBJECTS_PER_SCOPE: usize = 256;

/// How many rules ONE `Role` or `ClusterRole` may carry, how many subjects one binding
/// may carry, and how many entries one rule's own lists may carry.
///
/// CHOSEN, not published: Kubernetes bounds an RBAC object only by the size of the
/// request that creates it, so the store needs bounds of its own — a create whose
/// manifest is one large object would otherwise retain whatever a caller sent, and the
/// authorizer walks every rule of every bound role on every request, so these are what
/// make one decision a bounded product. Each is far above the handful of rules, lists
/// and subjects a real role carries.
pub const MAX_RULES_PER_ROLE: usize = 64;
/// See [`MAX_RULES_PER_ROLE`].
pub const MAX_SUBJECTS_PER_BINDING: usize = 64;
/// See [`MAX_RULES_PER_ROLE`].
pub const MAX_ENTRIES_PER_RULE: usize = 64;

/// The four resources the group serves.
///
/// `Role` and `RoleBinding` live in a namespace; `ClusterRole` and
/// `ClusterRoleBinding` are the cluster's. The discriminant is part of the key the
/// objects are stored under, so one map holds a scope's objects of every resource
/// and a listing walks one resource of it in name order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RbacResource {
    ClusterRoleBindings,
    ClusterRoles,
    RoleBindings,
    Roles,
}

impl RbacResource {
    /// The resource this path segment addresses, or `None` for a segment the group
    /// does not serve.
    #[must_use]
    pub fn from_segment(segment: &str) -> Option<Self> {
        match segment {
            "roles" => Some(Self::Roles),
            "rolebindings" => Some(Self::RoleBindings),
            "clusterroles" => Some(Self::ClusterRoles),
            "clusterrolebindings" => Some(Self::ClusterRoleBindings),
            _ => None,
        }
    }

    /// The plural name the resource is addressed by, which is also the name an RBAC
    /// rule names it with.
    #[must_use]
    pub fn resource(self) -> &'static str {
        match self {
            Self::Roles => "roles",
            Self::RoleBindings => "rolebindings",
            Self::ClusterRoles => "clusterroles",
            Self::ClusterRoleBindings => "clusterrolebindings",
        }
    }

    /// The `kind` an object of this resource carries.
    #[must_use]
    pub fn kind(self) -> &'static str {
        match self {
            Self::Roles => "Role",
            Self::RoleBindings => "RoleBinding",
            Self::ClusterRoles => "ClusterRole",
            Self::ClusterRoleBindings => "ClusterRoleBinding",
        }
    }

    /// The `kind` of the LIST a collection read is answered with.
    #[must_use]
    fn list_kind(self) -> &'static str {
        match self {
            Self::Roles => "RoleList",
            Self::RoleBindings => "RoleBindingList",
            Self::ClusterRoles => "ClusterRoleList",
            Self::ClusterRoleBindings => "ClusterRoleBindingList",
        }
    }

    /// The singular name discovery reports for the resource.
    #[must_use]
    fn singular(self) -> &'static str {
        match self {
            Self::Roles => "role",
            Self::RoleBindings => "rolebinding",
            Self::ClusterRoles => "clusterrole",
            Self::ClusterRoleBindings => "clusterrolebinding",
        }
    }

    /// Whether an object of this resource belongs to a namespace.
    #[must_use]
    pub fn is_namespaced(self) -> bool {
        matches!(self, Self::Roles | Self::RoleBindings)
    }

    /// Whether an object of this resource carries rules (a role) rather than a
    /// binding.
    #[must_use]
    fn is_role(self) -> bool {
        matches!(self, Self::Roles | Self::ClusterRoles)
    }
}

/// The RBAC objects of ONE scope — a namespace, or the cluster — keyed by resource and
/// name, so one map holds a scope's objects of every resource and a listing walks one
/// resource of it in name order.
pub type RbacScope = BTreeMap<(RbacResource, String), RbacObject>;

/// One rule of a role: what it may be done to, and what may be done.
///
/// The members are the ones `PolicyRule` declares, each an empty list when the
/// manifest named none — which is how RBAC reads a rule that constrains nothing by
/// that member.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyRule {
    pub api_groups: Vec<String>,
    pub resources: Vec<String>,
    pub resource_names: Vec<String>,
    pub verbs: Vec<String>,
    pub non_resource_urls: Vec<String>,
}

impl PolicyRule {
    /// The rule as the API server answers it back.
    #[must_use]
    fn to_json(&self) -> ::serde_json::Value {
        let mut rule = ::serde_json::Map::new();
        for (member, values) in [
            ("verbs", &self.verbs),
            ("apiGroups", &self.api_groups),
            ("resources", &self.resources),
            ("resourceNames", &self.resource_names),
            ("nonResourceURLs", &self.non_resource_urls),
        ] {
            if !values.is_empty() {
                rule.insert(member.to_string(), ::serde_json::json!(values));
            }
        }
        ::serde_json::Value::Object(rule)
    }

    /// Whether this rule admits `attributes`.
    #[must_use]
    fn admits(&self, attributes: &RequestAttributes) -> bool {
        rule_admits(
            &borrowed(&self.api_groups),
            &borrowed(&self.resources),
            &borrowed(&self.resource_names),
            &borrowed(&self.verbs),
            &borrowed(&self.non_resource_urls),
            attributes,
        )
    }
}

/// The `&str` view of an owned list, so one matcher serves the rules a client
/// created and the published ones below.
fn borrowed(values: &[String]) -> Vec<&str> {
    values.iter().map(String::as_str).collect()
}

/// The role a binding binds: `roleRef`.
#[derive(Debug, Clone)]
pub struct RoleRef {
    pub api_group: String,
    pub kind: String,
    pub name: String,
}

/// One subject a binding binds a role to: a user, a group, or a service account.
#[derive(Debug, Clone)]
pub struct Subject {
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
    pub api_group: Option<String>,
}

/// The username a `ServiceAccount` subject stands for, which is the username
/// Kubernetes authenticates a service account as.
///
/// `pub(super)` so the authentication of a token a service account was issued one
/// under reads it from here ([`super::kubernetes_service_account_tokens`]): the name
/// a binding's subject is matched against and the name such a request is served as
/// are one value.
pub(super) fn service_account_username(namespace: &str, name: &str) -> String {
    format!("system:serviceaccount:{namespace}:{name}")
}

/// What one stored RBAC object holds beyond its identity: the rules of a role, or
/// what a binding binds.
#[derive(Debug)]
pub enum RbacBody {
    Rules(Vec<PolicyRule>),
    Binding {
        role_ref: RoleRef,
        subjects: Vec<Subject>,
    },
}

/// One RBAC object this cluster holds.
#[derive(Debug)]
pub struct RbacObject {
    pub uid: String,
    pub created_at: SystemTime,
    /// The revision the write that last stored this object was stamped with, which
    /// a client hands back to update it
    /// ([`super::kubernetes_resource_versions`]).
    pub resource_version: u64,
    pub body: RbacBody,
}

impl RbacObject {
    /// The object as the document a client reads back.
    #[must_use]
    fn to_json(
        &self,
        resource: RbacResource,
        name: &str,
        namespace: Option<&str>,
    ) -> ::serde_json::Value {
        let mut metadata = ::serde_json::Map::new();
        metadata.insert("name".to_string(), ::serde_json::json!(name));
        if let Some(namespace) = namespace {
            metadata.insert("namespace".to_string(), ::serde_json::json!(namespace));
        }
        metadata.insert("uid".to_string(), ::serde_json::json!(self.uid));
        metadata.insert(
            "resourceVersion".to_string(),
            ::serde_json::json!(super::kubernetes_resource_versions::rendered(
                self.resource_version
            )),
        );
        metadata.insert(
            "creationTimestamp".to_string(),
            ::serde_json::json!(rfc3339_seconds(self.created_at)),
        );
        let mut document = ::serde_json::Map::new();
        document.insert("kind".to_string(), ::serde_json::json!(resource.kind()));
        document.insert(
            "apiVersion".to_string(),
            ::serde_json::json!(RBAC_API_VERSION),
        );
        document.insert(
            "metadata".to_string(),
            ::serde_json::Value::Object(metadata),
        );
        match &self.body {
            RbacBody::Rules(rules) => {
                let rules: Vec<::serde_json::Value> =
                    rules.iter().map(PolicyRule::to_json).collect();
                document.insert("rules".to_string(), ::serde_json::json!(rules));
            }
            RbacBody::Binding { role_ref, subjects } => {
                document.insert(
                    "roleRef".to_string(),
                    ::serde_json::json!({
                        "apiGroup": role_ref.api_group,
                        "kind": role_ref.kind,
                        "name": role_ref.name,
                    }),
                );
                let subjects: Vec<::serde_json::Value> = subjects
                    .iter()
                    .map(|subject| {
                        let mut rendered = ::serde_json::Map::new();
                        rendered.insert("kind".to_string(), ::serde_json::json!(subject.kind));
                        rendered.insert("name".to_string(), ::serde_json::json!(subject.name));
                        if let Some(namespace) = &subject.namespace {
                            rendered
                                .insert("namespace".to_string(), ::serde_json::json!(namespace));
                        }
                        if let Some(api_group) = &subject.api_group {
                            rendered.insert("apiGroup".to_string(), ::serde_json::json!(api_group));
                        }
                        ::serde_json::Value::Object(rendered)
                    })
                    .collect();
                document.insert("subjects".to_string(), ::serde_json::json!(subjects));
            }
        }
        ::serde_json::Value::Object(document)
    }

    /// Whether this binding binds a role to `identity`.
    #[must_use]
    fn binds(&self, identity: &SubjectIdentity) -> bool {
        let RbacBody::Binding { subjects, .. } = &self.body else {
            return false;
        };
        subjects.iter().any(|subject| match subject.kind.as_str() {
            "User" => subject.name == identity.username,
            "Group" => identity.groups.contains(&subject.name),
            "ServiceAccount" => subject
                .namespace
                .as_deref()
                .is_some_and(|namespace| {
                    service_account_username(namespace, &subject.name) == identity.username
                }),
            _ => false,
        })
    }
}

/// The verb a method is on a path that addresses no resource: the method itself, in
/// lower case, which is how a `nonResourceURLs` rule names it.
fn non_resource_verb(method: &str) -> String {
    method.to_ascii_lowercase()
}

/// The `APIGroup` discovery document of the RBAC group, which a client reads before
/// it addresses the group.
#[must_use]
pub fn api_group_document() -> ::serde_json::Value {
    ::serde_json::json!({
        "kind": "APIGroup",
        "apiVersion": DISCOVERY_API_VERSION,
        "name": RBAC_API_GROUP,
        "versions": [group_version()],
        "preferredVersion": group_version(),
    })
}

/// The group's entry in the `APIGroupList` served at `/apis`.
#[must_use]
pub fn api_group_entry() -> ::serde_json::Value {
    ::serde_json::json!({
        "name": RBAC_API_GROUP,
        "versions": [group_version()],
        "preferredVersion": group_version(),
    })
}

/// One `GroupVersionForDiscovery`.
fn group_version() -> ::serde_json::Value {
    ::serde_json::json!({
        "groupVersion": RBAC_API_VERSION,
        "version": RBAC_GROUP_VERSION,
    })
}

/// The verbs this API server serves on an RBAC resource, which is what its
/// discovery document reports for each of them.
const SERVED_VERBS: [&str; 5] = ["create", "delete", "get", "list", "update"];

/// The `APIResourceList` of `rbac.authorization.k8s.io/v1`: the four resources the
/// group serves and what may be done to each.
#[must_use]
pub fn api_resource_list_document() -> ::serde_json::Value {
    let resources: Vec<::serde_json::Value> = [
        RbacResource::ClusterRoleBindings,
        RbacResource::ClusterRoles,
        RbacResource::RoleBindings,
        RbacResource::Roles,
    ]
    .into_iter()
    .map(|resource| {
        ::serde_json::json!({
            "name": resource.resource(),
            "singularName": resource.singular(),
            "namespaced": resource.is_namespaced(),
            "kind": resource.kind(),
            "verbs": SERVED_VERBS,
        })
    })
    .collect();
    ::serde_json::json!({
        "kind": "APIResourceList",
        "apiVersion": DISCOVERY_API_VERSION,
        "groupVersion": RBAC_API_VERSION,
        "resources": resources,
    })
}
