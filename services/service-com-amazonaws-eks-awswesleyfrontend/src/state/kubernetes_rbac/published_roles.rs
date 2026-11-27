// SPDX-License-Identifier: CC0-1.0

//! The permissions a cluster grants without any object of its own carrying them: the
//! Kubernetes roles every cluster has, the groups Kubernetes binds them to, and the
//! EKS access policies an access entry associates.
//!
//! They are `&'static` tables rather than objects in each cluster's store because they
//! are the same on every cluster, no request creates or changes them, and a cluster
//! that held them as objects would let a caller edit what the service grants. The
//! objects a caller DOES create live in [`super::stored_objects`]; the decision over
//! both is [`admits`].

use super::*;

/// The permissions of one published role, as a rule table.
///
/// The published Kubernetes roles and the EKS access policies are stated as
/// `&'static` tables rather than written into every cluster's store: they are the
/// same on every cluster, no request creates or changes them, and a cluster that
/// holds them as objects would let a caller edit what the service grants.
#[derive(Debug, Clone, Copy)]
pub struct PublishedRule {
    pub api_groups: &'static [&'static str],
    pub resources: &'static [&'static str],
    pub verbs: &'static [&'static str],
    pub non_resource_urls: &'static [&'static str],
}

impl PublishedRule {
    /// Whether this rule admits `attributes`.
    #[must_use]
    pub(super) fn admits(&self, attributes: &RequestAttributes) -> bool {
        rule_admits(
            self.api_groups,
            self.resources,
            &[],
            self.verbs,
            self.non_resource_urls,
            attributes,
        )
    }
}

/// Whether a rule made of these members admits `attributes`: the ONE matcher, so a
/// rule a client created and a published one are read by the same reading of RBAC.
pub(super) fn rule_admits(
    api_groups: &[&str],
    resources: &[&str],
    resource_names: &[&str],
    verbs: &[&str],
    non_resource_urls: &[&str],
    attributes: &RequestAttributes,
) -> bool {
    if !names(verbs, &attributes.verb) {
        return false;
    }
    if attributes.is_resource_request() {
        return names(api_groups, &attributes.api_group)
            && resources
                .iter()
                .any(|named| resource_names_match(named, attributes))
            && (resource_names.is_empty()
                || (!attributes.name.is_empty() && names(resource_names, &attributes.name)));
    }
    non_resource_urls
        .iter()
        .any(|pattern| non_resource_url_matches(pattern, &attributes.path))
}

/// Whether a rule's list names `wanted`: the wildcard RBAC gives every member, or
/// the value itself.
pub(super) fn names(list: &[&str], wanted: &str) -> bool {
    list.iter().any(|named| *named == "*" || *named == wanted)
}

/// Whether a rule's `resources` entry names what the request addresses.
///
/// A rule names a subresource as `<resource>/<subresource>`, so a request to one is
/// admitted by that spelling, by `<resource>/*`, or by the wildcard.
pub(super) fn resource_names_match(named: &str, attributes: &RequestAttributes) -> bool {
    if named == "*" {
        return true;
    }
    if attributes.subresource.is_empty() {
        return named == attributes.resource;
    }
    named == format!("{}/{}", attributes.resource, attributes.subresource)
        || named == format!("{}/*", attributes.resource)
}

/// Whether a rule's `nonResourceURLs` entry names `path`: the path itself, or a
/// prefix ending in `*`, which is the one pattern RBAC reads there.
pub(super) fn non_resource_url_matches(pattern: &str, path: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => path.starts_with(prefix),
        None => pattern == path,
    }
}

/// The identity a request is authorized as: the username the cluster's access entry
/// maps the IAM principal to, the Kubernetes groups it carries, the access
/// policies associated to that entry, and the IAM principal the credential itself
/// spoke for.
#[derive(Debug, Clone, Default)]
pub struct SubjectIdentity {
    pub username: String,
    /// The groups, as a SET: a binding's subjects are matched against them once per
    /// binding of the scope, so membership has to be a lookup rather than a scan —
    /// what one decision costs is then the bounded number of subjects the cluster's
    /// own objects hold, and not that number times however many groups an access
    /// entry was created with. The set is built once per request
    /// ([`Self::authenticated_groups`]) and names each group once however often the
    /// entry repeats it.
    pub groups: BTreeSet<String>,
    /// One grant per access policy the entry associates, in the scope it was
    /// associated with.
    pub grants: Vec<AccessPolicyGrant>,
    /// The ARN of the IAM principal the request's credential spoke for — the one
    /// `sts:GetCallerIdentity` answers for it, which is what the cluster's mapping
    /// was looked up under. Reported as the authenticating authority's own
    /// information about the identity
    /// ([`super::kubernetes_self_subject_review`]) rather than used in a decision:
    /// what an identity MAY do is decided by its username and groups alone.
    pub principal_arn: String,
    /// The UNIQUE ID of that IAM principal: the `UserId` part
    /// `sts:GetCallerIdentity` reports for the same credential, without the
    /// session the credential was minted for. It is what the Kubernetes identity's
    /// `uid` is built from ([`super::kubernetes_self_subject_review`]) — two
    /// principals that happen to share a username are still two subjects, because
    /// no two IAM principals share a unique id.
    pub principal_id: String,
    /// The name of the assumed-role session the credential was minted for, and
    /// `None` for a long-term credential, which names no session.
    pub session_name: Option<String>,
}

/// One access policy an access entry associates, and where it applies.
#[derive(Debug, Clone)]
pub struct AccessPolicyGrant {
    /// The name of the published access policy, which is what decides the
    /// permissions ([`access_policy_rules`]).
    pub policy_name: String,
    /// The namespaces the association scoped the policy to, and `None` for an
    /// association scoped to the whole cluster.
    pub namespaces: Option<Vec<String>>,
}

impl AccessPolicyGrant {
    /// Whether this grant reaches `namespace`.
    ///
    /// "namespace: the access policy's permissions apply to the namespaces you
    /// specify" — a namespace-scoped association therefore reaches nothing outside
    /// them, and nothing that addresses no namespace at all; one scoped to the
    /// cluster reaches every namespace and the cluster scope alike.
    #[must_use]
    pub(super) fn applies_in(&self, namespace: &str) -> bool {
        match &self.namespaces {
            Some(namespaces) => {
                !namespace.is_empty() && namespaces.iter().any(|named| named == namespace)
            }
            None => true,
        }
    }

    /// Whether this grant admits `attributes`.
    #[must_use]
    pub(super) fn admits(&self, attributes: &RequestAttributes) -> bool {
        if !self.applies_in(&attributes.namespace) {
            return false;
        }
        access_policy_rules(&self.policy_name)
            .is_some_and(|rules| rules.iter().any(|rule| rule.admits(attributes)))
    }
}

impl SubjectIdentity {
    /// The groups every authenticated identity carries beside the ones its access
    /// entry names: `system:authenticated`, the group Kubernetes puts every
    /// authenticated request in.
    #[must_use]
    pub fn authenticated_groups(entry_groups: &[String]) -> BTreeSet<String> {
        let mut groups: BTreeSet<String> = entry_groups.iter().cloned().collect();
        groups.insert(AUTHENTICATED_GROUP.to_string());
        groups
    }

    /// Whether the access policies and the groups this identity carries admit
    /// `attributes` on their own — before the cluster's own RBAC objects are
    /// consulted.
    #[must_use]
    pub(super) fn published_admits(&self, attributes: &RequestAttributes) -> bool {
        if self
            .grants
            .iter()
            .any(|grant| grant.admits(attributes))
        {
            return true;
        }
        self.groups
            .iter()
            .filter_map(|group| published_group_rules(group))
            .any(|rules| rules.iter().any(|rule| rule.admits(attributes)))
    }
}

/// The group every authenticated request carries.
pub const AUTHENTICATED_GROUP: &str = "system:authenticated";

/// The group Kubernetes binds `cluster-admin` to, which is why an identity in it may
/// do anything on the cluster.
pub const MASTERS_GROUP: &str = "system:masters";

/// Whether the cluster admits `attributes` for `identity`.
///
/// The decision is the union RBAC takes: the request is admitted when the published
/// permissions the identity carries admit it — the access policies its access entry
/// associates, and the roles Kubernetes binds to the groups it is in — or when a
/// binding the cluster holds binds it to a role whose rules admit it. Nothing else
/// admits a request, so an identity with no grant and no binding is refused.
#[must_use]
pub fn admits(
    objects: &ClusterObjects,
    identity: &SubjectIdentity,
    attributes: &RequestAttributes,
) -> bool {
    identity.published_admits(attributes) || objects.rbac_admits(identity, attributes)
}

/// `cluster-admin`: "allows super-user access to perform any action on any
/// resource", which is the Kubernetes role the cluster-admin access policy is
/// published as the equivalent of.
pub(super) const CLUSTER_ADMIN_RULES: &[PublishedRule] = &[
    PublishedRule {
        api_groups: &["*"],
        resources: &["*"],
        verbs: &["*"],
        non_resource_urls: &[],
    },
    PublishedRule {
        api_groups: &[],
        resources: &[],
        verbs: &["*"],
        non_resource_urls: &["*"],
    },
];

/// The verbs a role that may change an object carries, as the Kubernetes
/// user-facing roles carry them.
pub(super) const WRITE_VERBS: &[&str] = &[
    "create",
    "delete",
    "deletecollection",
    "get",
    "list",
    "patch",
    "update",
    "watch",
];

/// The verbs a read-only role carries.
pub(super) const READ_VERBS: &[&str] = &["get", "list", "watch"];

/// The API groups the Kubernetes `admin`, `edit` and `view` roles cover the
/// resources of: the core group and the workload, networking and policy groups.
pub(super) const WORKLOAD_API_GROUPS: &[&str] = &[
    "",
    "apps",
    "autoscaling",
    "batch",
    "extensions",
    "networking.k8s.io",
    "policy",
];

/// The resources those roles may READ and WRITE in a namespace, which is the
/// namespaced part of the Kubernetes `edit` role: the workload, service and
/// configuration objects of a namespace. Secrets are deliberately not among them —
/// `edit` and `view` do not carry them, and the `admin` table below adds them.
pub(super) const EDITABLE_RESOURCES: &[&str] = &[
    "configmaps",
    "cronjobs",
    "daemonsets",
    "deployments",
    "endpoints",
    "events",
    "horizontalpodautoscalers",
    "ingresses",
    "jobs",
    "networkpolicies",
    "persistentvolumeclaims",
    "poddisruptionbudgets",
    "pods",
    "pods/attach",
    "pods/exec",
    "pods/log",
    "pods/portforward",
    "replicasets",
    "replicationcontrollers",
    "serviceaccounts",
    "serviceaccounts/token",
    "services",
    "statefulsets",
];

/// The resources `admin` carries beyond [`EDITABLE_RESOURCES`]: the secrets and
/// quota objects of its namespace, and the RBAC objects it may grant within it —
/// "allows admin access, intended to be granted within a namespace … includes the
/// ability to create roles and role bindings within the namespace".
pub(super) const ADMIN_EXTRA_RESOURCES: &[&str] = &["limitranges", "resourcequotas", "secrets"];

/// `admin`.
pub(super) const ADMIN_RULES: &[PublishedRule] = &[
    PublishedRule {
        api_groups: WORKLOAD_API_GROUPS,
        resources: EDITABLE_RESOURCES,
        verbs: WRITE_VERBS,
        non_resource_urls: &[],
    },
    PublishedRule {
        api_groups: &[""],
        resources: ADMIN_EXTRA_RESOURCES,
        verbs: WRITE_VERBS,
        non_resource_urls: &[],
    },
    PublishedRule {
        api_groups: &[RBAC_API_GROUP],
        resources: &["rolebindings", "roles"],
        verbs: WRITE_VERBS,
        non_resource_urls: &[],
    },
    PublishedRule {
        api_groups: &[""],
        resources: &["namespaces"],
        verbs: READ_VERBS,
        non_resource_urls: &[],
    },
];

/// `edit`.
pub(super) const EDIT_RULES: &[PublishedRule] = &[
    PublishedRule {
        api_groups: WORKLOAD_API_GROUPS,
        resources: EDITABLE_RESOURCES,
        verbs: WRITE_VERBS,
        non_resource_urls: &[],
    },
    PublishedRule {
        api_groups: &[""],
        resources: &["namespaces"],
        verbs: READ_VERBS,
        non_resource_urls: &[],
    },
];

/// `view`: "allows read-only access … does not allow viewing secrets".
pub(super) const VIEW_RULES: &[PublishedRule] = &[
    PublishedRule {
        api_groups: WORKLOAD_API_GROUPS,
        resources: EDITABLE_RESOURCES,
        verbs: READ_VERBS,
        non_resource_urls: &[],
    },
    PublishedRule {
        api_groups: &[""],
        resources: &["namespaces"],
        verbs: READ_VERBS,
        non_resource_urls: &[],
    },
];

/// The read-only mirror of `admin`, which is what the admin-view access policy
/// grants: "list/view all resources in a cluster", secrets included, and change
/// none.
pub(super) const ADMIN_VIEW_RULES: &[PublishedRule] = &[PublishedRule {
    api_groups: &["*"],
    resources: &["*"],
    verbs: READ_VERBS,
    non_resource_urls: &[],
}];

/// The permissions Kubernetes binds to `system:authenticated` on every cluster:
/// the two ClusterRoles it binds to that group — `system:discovery`, the
/// discovery documents every authenticated client reads before anything else, and
/// `system:basic-user`, "allows a user read-only access to basic information
/// about themselves", which is what lets a client ask the API server which
/// identity it is serving it as
/// ([`super::kubernetes_self_subject_review`]).
pub(super) const AUTHENTICATED_GROUP_RULES: &[PublishedRule] = &[
    PublishedRule {
        api_groups: &[],
        resources: &[],
        verbs: &["get"],
        non_resource_urls: &[
            "/api",
            "/api/*",
            "/apis",
            "/apis/*",
            "/healthz",
            "/livez",
            "/openapi",
            "/openapi/*",
            "/readyz",
            "/version",
            "/version/",
        ],
    },
    PublishedRule {
        api_groups: &["authentication.k8s.io"],
        resources: &["selfsubjectreviews"],
        verbs: &["create"],
        non_resource_urls: &[],
    },
    PublishedRule {
        api_groups: &["authorization.k8s.io"],
        resources: &["selfsubjectaccessreviews", "selfsubjectrulesreviews"],
        verbs: &["create"],
        non_resource_urls: &[],
    },
];

/// The permissions the EKS access policy `policy_name` grants, or `None` for a
/// published policy whose permissions this emulator does not state.
///
/// The general-purpose policies are the ones Amazon EKS publishes as the equivalents
/// of the Kubernetes user-facing roles — cluster-admin, admin, edit, view, and the
/// read-only mirror of admin — so each grants that role's permissions, in the scope
/// the association was made with. A policy published for one service's controller
/// (`ListAccessPolicies` reports many) grants the permissions that controller needs
/// on the objects it owns; this emulator states none of those, so an entry carrying
/// only such a policy is authorized by its Kubernetes groups and the cluster's own
/// RBAC objects alone.
#[must_use]
pub fn access_policy_rules(policy_name: &str) -> Option<&'static [PublishedRule]> {
    match policy_name {
        "AmazonEKSClusterAdminPolicy" => Some(CLUSTER_ADMIN_RULES),
        "AmazonEKSAdminPolicy" => Some(ADMIN_RULES),
        "AmazonEKSAdminViewPolicy" => Some(ADMIN_VIEW_RULES),
        "AmazonEKSEditPolicy" => Some(EDIT_RULES),
        "AmazonEKSViewPolicy" => Some(VIEW_RULES),
        _ => None,
    }
}

/// The permissions Kubernetes binds to the group `group` on every cluster.
pub(super) fn published_group_rules(group: &str) -> Option<&'static [PublishedRule]> {
    match group {
        MASTERS_GROUP => Some(CLUSTER_ADMIN_RULES),
        AUTHENTICATED_GROUP => Some(AUTHENTICATED_GROUP_RULES),
        _ => None,
    }
}
