// SPDX-License-Identifier: CC0-1.0

//! WHO a request to a cluster's Kubernetes API server is served as, and what the
//! cluster's RBAC objects let that identity do — over the address the cluster itself
//! publishes.
//!
//! The RULE under test: a cluster's Kubernetes API server serves the
//! `rbac.authorization.k8s.io/v1` group — the discovery documents a client reads and
//! the four resources it addresses — and every request is decided the way an API
//! server decides one: the credential names an IAM principal, the cluster's ACCESS
//! ENTRY for that principal is the Kubernetes identity the request is served as
//! (`AccessEntry$username` is "the name of a user that can authenticate to your
//! cluster", `AccessEntry$kubernetesGroups` "a list of Kubernetes groups that the
//! principal is a member of"), and the RBAC objects bound to that username and those
//! groups — together with the access policies the entry associates — decide whether it
//! may do what the request addresses. A principal no access entry maps authenticates
//! as nobody.
//!
//! Every suite drives that over a TABLE whose rows differ along what the rule
//! generalises over: the resource and the scope an RBAC object lives in (a namespaced
//! `Role` and `RoleBinding`, a cluster-scoped `ClusterRole` and `ClusterRoleBinding`),
//! and — for the decision — the verb, the resource, the namespace, and whether the
//! identity carries the grant through an access policy, through a binding, or not at
//! all.
//!
//! The client is the raw one this crate's `kubernetes_api` suite dials with, so
//! nothing but what the emulator published is used to reach the API. The audit records
//! these requests leave in CloudWatch Logs need a second service on the engine and are
//! pinned in `core/router/tests/all/eks_kubernetes_api_rbac_audit.rs`.

use service_com_amazonaws_eks_awswesleyfrontend::state::kubernetes_rbac::{
    MAX_ENTRIES_PER_RULE, MAX_RULES_PER_ROLE, MAX_SUBJECTS_PER_BINDING,
};
use service_com_amazonaws_eks_awswesleyfrontend::state::{
    access_policy_arn, ACCESS_SCOPE_TYPE_CLUSTER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_authority::REGION;
use crate::kubernetes_api::{dial, kubeconfig_of, poll_until_active, ApiAnswer, Kubeconfig};
use crate::wire::{call, emulator, encode_label, Answer};

/// The access policy that grants the permissions of the Kubernetes `cluster-admin`
/// role, which is what the cluster's own creator is granted.
const CLUSTER_ADMIN_POLICY: &str = "AmazonEKSClusterAdminPolicy";

/// The namespace the rows address.
const NAMESPACE: &str = "rbac-team";

/// A second namespace, which the namespaced binding below does not reach.
const OTHER_NAMESPACE: &str = "rbac-elsewhere";

/// The Kubernetes group the access entry puts its principal in.
const WRITERS_GROUP: &str = "rbac-writers";

/// The username the access entry maps its IAM principal to.
const MAPPED_USERNAME: &str = "rbac-user";

/// The path of a namespaced RBAC collection.
fn rbac_path(namespace: &str, resource: &str) -> String {
    format!("/apis/rbac.authorization.k8s.io/v1/namespaces/{namespace}/{resource}")
}

/// The path of a cluster-scoped RBAC collection.
fn cluster_rbac_path(resource: &str) -> String {
    format!("/apis/rbac.authorization.k8s.io/v1/{resource}")
}

/// A `CreateCluster` body for a cluster whose Kubernetes API this suite dials: the
/// `authenticationMode` that serves the access-entry API, and
/// `bootstrapClusterCreatorAdminPermissions` as the row asks for it.
fn cluster_request(name: &str, bootstrap_creator_admin: bool) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
        "accessConfig": {
            "authenticationMode": "API_AND_CONFIG_MAP",
            "bootstrapClusterCreatorAdminPermissions": bootstrap_creator_admin,
        },
    })
}

/// Create the cluster and wait for the clock-owed provisioning to be materialised,
/// then read the kubeconfig members back the way a client does.
async fn provisioned_cluster(
    router: &MultiServiceRouter,
    name: &str,
    bootstrap_creator_admin: bool,
) -> Kubeconfig {
    let created = call(
        router,
        REGION,
        "POST",
        "/clusters",
        &cluster_request(name, bootstrap_creator_admin).to_string(),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCluster({name}), got {created:?}"
    );
    let described = poll_until_active(router, REGION, name).await;
    kubeconfig_of(name, &described)
}

/// `CreateAccessEntry` for `principal_arn`, mapping it to `MAPPED_USERNAME` and
/// `WRITERS_GROUP`.
async fn create_access_entry(
    router: &MultiServiceRouter,
    cluster: &str,
    principal_arn: &str,
) -> Answer {
    let body = serde_json::json!({
        "principalArn": principal_arn,
        "username": MAPPED_USERNAME,
        "kubernetesGroups": [WRITERS_GROUP],
    });
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/access-entries", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

/// `AssociateAccessPolicy`: grant `policy_name` over the whole cluster.
async fn associate_policy(
    router: &MultiServiceRouter,
    cluster: &str,
    principal_arn: &str,
    policy_name: &str,
) -> Answer {
    let body = serde_json::json!({
        "policyArn": access_policy_arn(policy_name),
        "accessScope": { "type": ACCESS_SCOPE_TYPE_CLUSTER },
    });
    call(
        router,
        REGION,
        "POST",
        &format!(
            "/clusters/{}/access-entries/{}/access-policies",
            encode_label(cluster),
            encode_label(principal_arn)
        ),
        &body.to_string(),
    )
    .await
}

/// `DisassociateAccessPolicy`: take that grant away again.
async fn disassociate_policy(
    router: &MultiServiceRouter,
    cluster: &str,
    principal_arn: &str,
    policy_name: &str,
) -> Answer {
    call(
        router,
        REGION,
        "DELETE",
        &format!(
            "/clusters/{}/access-entries/{}/access-policies/{}",
            encode_label(cluster),
            encode_label(principal_arn),
            encode_label(&access_policy_arn(policy_name))
        ),
        "",
    )
    .await
}

/// The `Role` the rows are decided by: the verbs it names on the config maps of its
/// own namespace, and nothing else.
fn configmap_role() -> serde_json::Value {
    serde_json::json!({
        "metadata": { "name": "configmap-writer" },
        "rules": [{
            "apiGroups": [""],
            "resources": ["configmaps"],
            "verbs": ["get", "list", "create"],
        }],
    })
}

/// The `RoleBinding` that binds that role to the group the access entry names.
fn writers_binding() -> serde_json::Value {
    serde_json::json!({
        "metadata": { "name": "configmap-writers" },
        "roleRef": {
            "apiGroup": "rbac.authorization.k8s.io",
            "kind": "Role",
            "name": "configmap-writer",
        },
        "subjects": [{ "kind": "Group", "name": WRITERS_GROUP }],
    })
}

/// Create one object through the API and assert it was.
async fn must_create(
    config: &Kubeconfig,
    case: &str,
    path: &str,
    manifest: &serde_json::Value,
) -> ApiAnswer {
    let created = dial(config, case, "POST", path, Some(manifest)).await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected 201 Created from {path}, got {created:?}"
    );
    created
}

// ---------------------------------------------------------------------------
// RULE: the `rbac.authorization.k8s.io/v1` group is served on the address the cluster
// publishes — discovered, created, read back, listed and given up — for the namespaced
// resources and the cluster-scoped ones alike.
// ---------------------------------------------------------------------------

/// The suites of this file, driven one after another in ONE test.
///
/// Every request they make takes one of the API server's own in-flight permits
/// (`listener_caps::MAX_HTTP_INFLIGHT`, a ceiling the core sets for the whole
/// process), and this test binary already runs dozens of suites that dial a cluster
/// at once; three more dialling in parallel is what sheds a request with the status
/// the API server answers one it could not take. So the three rules below are checked
/// in sequence inside one test rather than in three that race for those permits.
#[tokio::test]
async fn the_rbac_group_is_served_and_decides_every_request() {
    the_rbac_group_is_discovered_and_its_objects_round_trip().await;
    a_manifest_past_what_one_object_may_hold_is_refused().await;
    the_access_entry_and_the_rbac_objects_decide_every_request().await;
}

async fn the_rbac_group_is_discovered_and_its_objects_round_trip() {
    let router = emulator();
    let config = provisioned_cluster(&router, "rbac-group", true).await;

    // Discovery: the group a client finds at `/apis` is the one it can then address,
    // and the version document names the resources the group serves.
    let groups = dial(&config, "the group list", "GET", "/apis", None).await;
    assert_eq!(
        groups.status, 200,
        "case the group list: expected 200, got {groups:?}"
    );
    let discovered: Vec<String> = groups.body["groups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|group| group["name"].as_str().map(str::to_owned))
        .collect();
    assert!(
        discovered.contains(&"rbac.authorization.k8s.io".to_string()),
        "case the group list: the RBAC group must be discovered, got {discovered:?}"
    );
    for (case, path) in [
        ("the group document", "/apis/rbac.authorization.k8s.io"),
        ("the version document", "/apis/rbac.authorization.k8s.io/v1"),
    ] {
        let answered = dial(&config, case, "GET", path, None).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected 200 from {path}, got {answered:?}"
        );
    }
    let version = dial(
        &config,
        "the version document",
        "GET",
        "/apis/rbac.authorization.k8s.io/v1",
        None,
    )
    .await;
    let served: Vec<String> = version.body["resources"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|resource| resource["name"].as_str().map(str::to_owned))
        .collect();
    for resource in ["roles", "rolebindings", "clusterroles", "clusterrolebindings"] {
        assert!(
            served.contains(&resource.to_string()),
            "case the version document: {resource:?} must be served, got {served:?}"
        );
    }

    must_create(
        &config,
        "the namespace the namespaced rows live in",
        "/api/v1/namespaces",
        &serde_json::json!({ "metadata": { "name": NAMESPACE } }),
    )
    .await;

    // The rows differ along the resource, its scope, and whether it carries rules or
    // a binding.
    let rows: Vec<(&str, String, serde_json::Value, &str, bool)> = vec![
        (
            "a namespaced Role",
            rbac_path(NAMESPACE, "roles"),
            configmap_role(),
            "Role",
            true,
        ),
        (
            "a namespaced RoleBinding",
            rbac_path(NAMESPACE, "rolebindings"),
            writers_binding(),
            "RoleBinding",
            true,
        ),
        (
            "a cluster-scoped ClusterRole",
            cluster_rbac_path("clusterroles"),
            serde_json::json!({
                "metadata": { "name": "namespace-reader" },
                "rules": [{
                    "apiGroups": [""],
                    "resources": ["namespaces"],
                    "verbs": ["get", "list"],
                }],
            }),
            "ClusterRole",
            false,
        ),
        (
            "a cluster-scoped ClusterRoleBinding",
            cluster_rbac_path("clusterrolebindings"),
            serde_json::json!({
                "metadata": { "name": "namespace-readers" },
                "roleRef": {
                    "apiGroup": "rbac.authorization.k8s.io",
                    "kind": "ClusterRole",
                    "name": "namespace-reader",
                },
                "subjects": [{ "kind": "User", "name": MAPPED_USERNAME }],
            }),
            "ClusterRoleBinding",
            false,
        ),
    ];

    for (case, path, manifest, kind, namespaced) in &rows {
        let name = manifest["metadata"]["name"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the row names its object"))
            .to_string();
        let created = must_create(&config, case, path, manifest).await;
        assert_eq!(
            created.body["kind"],
            serde_json::json!(kind),
            "case {case:?}: expected a {kind}, got {:?}",
            created.body
        );
        assert_eq!(
            created.body["apiVersion"],
            serde_json::json!("rbac.authorization.k8s.io/v1"),
            "case {case:?}: expected the group's apiVersion, got {:?}",
            created.body
        );
        assert!(
            created.body["metadata"]["uid"]
                .as_str()
                .is_some_and(|uid| !uid.is_empty()),
            "case {case:?}: the create must mint a uid, got {:?}",
            created.body
        );
        assert_eq!(
            created.body["metadata"]["namespace"].is_string(),
            *namespaced,
            "case {case:?}: only a namespaced object carries a namespace, got {:?}",
            created.body
        );

        // Round trip: the object reads back as the create stored it, and the
        // collection lists it.
        let read = dial(&config, case, "GET", &format!("{path}/{name}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the object must read back, got {read:?}"
        );
        assert_eq!(
            read.body, created.body,
            "case {case:?}: the read must answer what the create stored, got {:?}",
            read.body
        );
        let listed = dial(&config, case, "GET", path, None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: the collection must be listed, got {listed:?}"
        );
        let names: Vec<String> = listed.body["items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|item| item["metadata"]["name"].as_str().map(str::to_owned))
            .collect();
        assert!(
            names.contains(&name),
            "case {case:?}: the listing must hold {name:?}, got {names:?}"
        );

        // A second create of the same name is the conflict it is, and a read of a
        // name nothing created is the not-found `Status`.
        let again = dial(&config, case, "POST", path, Some(manifest)).await;
        assert_eq!(
            again.status, 409,
            "case {case:?}: a second create of one name must conflict, got {again:?}"
        );
        let absent = dial(&config, case, "GET", &format!("{path}/absent"), None).await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: an object nothing created must be 404, got {absent:?}"
        );
        assert_eq!(
            absent.body["reason"],
            serde_json::json!("NotFound"),
            "case {case:?}: a refusal is a Kubernetes Status, got {:?}",
            absent.body
        );

        // What a delete gives up stops being held.
        let deleted = dial(&config, case, "DELETE", &format!("{path}/{name}"), None).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: the delete must be served, got {deleted:?}"
        );
        let gone = dial(&config, case, "GET", &format!("{path}/{name}"), None).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: a deleted object must be 404, got {gone:?}"
        );
    }

    // A namespaced resource addressed without a namespace, and a cluster-scoped one
    // addressed under a namespace, are paths the group does not serve.
    for (case, path) in [
        ("a Role addressed with no namespace", cluster_rbac_path("roles")),
        (
            "a ClusterRole addressed under a namespace",
            rbac_path(NAMESPACE, "clusterroles"),
        ),
    ] {
        let answered = dial(&config, case, "GET", &path, None).await;
        assert_eq!(
            answered.status, 404,
            "case {case:?}: expected 404 from {path}, got {answered:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: what one object may RETAIN is bounded before it is stored — the bytes of a
// config map (the 1 MiB Kubernetes publishes for the resource) and the lengths of an
// RBAC object's own lists (the emulator's own ceilings, which also bound what one
// authorization decision costs) — and a manifest past a bound is refused rather than
// kept.
// ---------------------------------------------------------------------------

async fn a_manifest_past_what_one_object_may_hold_is_refused() {
    let router = emulator();
    let config = provisioned_cluster(&router, "rbac-bounds", true).await;
    must_create(
        &config,
        "the namespace the bounded objects are created in",
        "/api/v1/namespaces",
        &serde_json::json!({ "metadata": { "name": NAMESPACE } }),
    )
    .await;

    /// One list of `count` entries, each a distinct string.
    fn entries(count: usize) -> Vec<String> {
        (0..count).map(|index| format!("entry-{index}")).collect()
    }

    // The rows differ along which of this module's own bounds is passed: the rules of
    // one role, the entries of one rule's list, and the subjects of one binding. Each
    // row is paired with the largest manifest the bound admits, so the refusal is
    // shown to be the bound rather than the shape of the manifest. (The BYTES one
    // config map may hold is `kubernetes_config_maps`' own bound, pinned by that
    // module's suite; a row for it here would send a megabyte over the API server's
    // in-flight permits for a rule this file does not own.)
    let rows: Vec<(&str, String, serde_json::Value, serde_json::Value)> = vec![
        (
            "the rules of one Role",
            rbac_path(NAMESPACE, "roles"),
            serde_json::json!({
                "metadata": { "name": "rules-at-the-limit" },
                "rules": vec![
                    serde_json::json!({
                        "apiGroups": [""],
                        "resources": ["configmaps"],
                        "verbs": ["get"],
                    });
                    MAX_RULES_PER_ROLE
                ],
            }),
            serde_json::json!({
                "metadata": { "name": "rules-past-the-limit" },
                "rules": vec![
                    serde_json::json!({
                        "apiGroups": [""],
                        "resources": ["configmaps"],
                        "verbs": ["get"],
                    });
                    MAX_RULES_PER_ROLE + 1
                ],
            }),
        ),
        (
            "the entries of one rule's list",
            rbac_path(NAMESPACE, "roles"),
            serde_json::json!({
                "metadata": { "name": "verbs-at-the-limit" },
                "rules": [{
                    "apiGroups": [""],
                    "resources": ["configmaps"],
                    "verbs": entries(MAX_ENTRIES_PER_RULE),
                }],
            }),
            serde_json::json!({
                "metadata": { "name": "verbs-past-the-limit" },
                "rules": [{
                    "apiGroups": [""],
                    "resources": ["configmaps"],
                    "verbs": entries(MAX_ENTRIES_PER_RULE + 1),
                }],
            }),
        ),
        (
            "the subjects of one binding",
            rbac_path(NAMESPACE, "rolebindings"),
            serde_json::json!({
                "metadata": { "name": "subjects-at-the-limit" },
                "roleRef": {
                    "apiGroup": "rbac.authorization.k8s.io",
                    "kind": "Role",
                    "name": "configmap-writer",
                },
                "subjects": entries(MAX_SUBJECTS_PER_BINDING)
                    .into_iter()
                    .map(|name| serde_json::json!({ "kind": "Group", "name": name }))
                    .collect::<Vec<serde_json::Value>>(),
            }),
            serde_json::json!({
                "metadata": { "name": "subjects-past-the-limit" },
                "roleRef": {
                    "apiGroup": "rbac.authorization.k8s.io",
                    "kind": "Role",
                    "name": "configmap-writer",
                },
                "subjects": entries(MAX_SUBJECTS_PER_BINDING + 1)
                    .into_iter()
                    .map(|name| serde_json::json!({ "kind": "Group", "name": name }))
                    .collect::<Vec<serde_json::Value>>(),
            }),
        ),
    ];

    for (case, path, admitted, refused) in &rows {
        must_create(&config, case, path, admitted).await;
        let answered = dial(&config, case, "POST", path, Some(refused)).await;
        assert_eq!(
            answered.status, 422,
            "case {case:?}: a manifest past the bound must be refused, got {answered:?}"
        );
        assert_eq!(
            answered.body["reason"],
            serde_json::json!("Invalid"),
            "case {case:?}: the refusal states the manifest is invalid, got {:?}",
            answered.body
        );
        // Nothing was stored for it: the bound refuses rather than retains.
        let name = refused["metadata"]["name"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the row names its object"));
        let read = dial(&config, case, "GET", &format!("{path}/{name}"), None).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: the refused manifest must not be held, got {read:?}"
        );
    }
}
struct DecisionRow {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<serde_json::Value>,
    /// The status while the identity carries the cluster-admin access policy.
    as_admin: u16,
    /// The status once that policy is taken away, leaving only the binding.
    as_bound_group: u16,
}

// ---------------------------------------------------------------------------
// RULE: what an identity may do is decided by the access policies its access entry
// associates and the RBAC objects bound to the username and groups that entry maps its
// IAM principal to — and a principal no access entry maps authenticates as nobody.
// ---------------------------------------------------------------------------

async fn the_access_entry_and_the_rbac_objects_decide_every_request() {
    let router = emulator();
    let cluster = "rbac-decides";
    // No bootstrap entry: the cluster starts with no access entry at all, so the
    // credential this suite dials with is mapped by the entry it creates below and by
    // nothing else.
    let config = provisioned_cluster(&router, cluster, false).await;

    // A principal no entry maps authenticates as nobody, whatever the cluster holds.
    let unmapped = dial(
        &config,
        "a principal no access entry maps",
        "GET",
        "/api/v1/namespaces",
        None,
    )
    .await;
    assert_eq!(
        unmapped.status, 401,
        "case a principal no access entry maps: expected 401 Unauthorized, got {unmapped:?}"
    );

    // The IAM principal the dialling credential speaks for is the one this emulator
    // publishes a long-term credential for, read from the one definition of it rather
    // than written out here.
    let principal_arn = service_common::caller_principal::ambient_user_arn();
    let entry = create_access_entry(&router, cluster, &principal_arn).await;
    assert_eq!(
        entry.status, 200,
        "setup: expected 200 from CreateAccessEntry, got {entry:?}"
    );
    let associated = associate_policy(&router, cluster, &principal_arn, CLUSTER_ADMIN_POLICY).await;
    assert_eq!(
        associated.status, 200,
        "setup: expected 200 from AssociateAccessPolicy, got {associated:?}"
    );

    // With the cluster-admin policy associated, the identity sets the cluster up: the
    // two namespaces, the Role, and the binding that names the group its entry puts
    // it in.
    for namespace in [NAMESPACE, OTHER_NAMESPACE] {
        must_create(
            &config,
            "a namespace the rows address",
            "/api/v1/namespaces",
            &serde_json::json!({ "metadata": { "name": namespace } }),
        )
        .await;
    }
    must_create(
        &config,
        "the Role the rows are decided by",
        &rbac_path(NAMESPACE, "roles"),
        &configmap_role(),
    )
    .await;
    must_create(
        &config,
        "the binding that names the entry's group",
        &rbac_path(NAMESPACE, "rolebindings"),
        &writers_binding(),
    )
    .await;

    let rows = vec![
        DecisionRow {
            case: "a verb and resource the bound Role names",
            method: "GET",
            path: format!("/api/v1/namespaces/{NAMESPACE}/configmaps"),
            body: None,
            as_admin: 200,
            as_bound_group: 200,
        },
        DecisionRow {
            case: "a create the bound Role names",
            method: "POST",
            path: format!("/api/v1/namespaces/{NAMESPACE}/configmaps"),
            body: Some(serde_json::json!({
                "metadata": { "name": "admitted" },
                "data": { "key": "value" },
            })),
            as_admin: 201,
            // Created again in the second pass: the first pass gave the ConfigMap up
            // again (the row below deletes it), so the name is free and the create is
            // the create it was the first time.
            as_bound_group: 201,
        },
        DecisionRow {
            case: "a verb the bound Role does not name",
            method: "DELETE",
            path: format!("/api/v1/namespaces/{NAMESPACE}/configmaps/admitted"),
            body: None,
            // The authorizer decides BEFORE the routing does, so the identity the
            // Role does not grant the verb to is refused 403 while the cluster admin
            // reaches the routing and the delete is served.
            as_admin: 200,
            as_bound_group: 403,
        },
        DecisionRow {
            case: "a resource the bound Role does not name",
            method: "GET",
            path: format!("/api/v1/namespaces/{NAMESPACE}/serviceaccounts"),
            body: None,
            as_admin: 200,
            as_bound_group: 403,
        },
        DecisionRow {
            case: "the same resource in a namespace the binding does not reach",
            method: "GET",
            path: format!("/api/v1/namespaces/{OTHER_NAMESPACE}/configmaps"),
            body: None,
            as_admin: 200,
            as_bound_group: 403,
        },
        DecisionRow {
            case: "a cluster-scoped read no binding grants",
            method: "GET",
            path: "/api/v1/namespaces".to_string(),
            body: None,
            as_admin: 200,
            as_bound_group: 403,
        },
        DecisionRow {
            case: "an RBAC object of the namespace the Role does not name",
            method: "POST",
            path: rbac_path(NAMESPACE, "roles"),
            body: Some(serde_json::json!({
                "metadata": { "name": "escalated" },
                "rules": [{ "apiGroups": ["*"], "resources": ["*"], "verbs": ["*"] }],
            })),
            as_admin: 201,
            as_bound_group: 403,
        },
        DecisionRow {
            case: "the version document every authenticated client reads",
            method: "GET",
            path: "/version".to_string(),
            body: None,
            as_admin: 200,
            as_bound_group: 200,
        },
    ];

    // The cluster-admin access policy admits every row, which is what the EKS access
    // policy published as that role's equivalent grants.
    for row in &rows {
        let answered = dial(
            &config,
            row.case,
            row.method,
            &row.path,
            row.body.as_ref(),
        )
        .await;
        assert_eq!(
            answered.status, row.as_admin,
            "case {:?} as the cluster admin: expected HTTP {}, got {answered:?}",
            row.case, row.as_admin
        );
    }

    // Take the policy away: the identity keeps the username and the group its entry
    // names, so what it may do is now exactly what the binding grants.
    let revoked =
        disassociate_policy(&router, cluster, &principal_arn, CLUSTER_ADMIN_POLICY).await;
    assert!(
        revoked.status == 200 || revoked.status == 204,
        "setup: expected DisassociateAccessPolicy to be served, got {revoked:?}"
    );

    for row in &rows {
        let answered = dial(
            &config,
            row.case,
            row.method,
            &row.path,
            row.body.as_ref(),
        )
        .await;
        assert_eq!(
            answered.status, row.as_bound_group,
            "case {:?} as the bound group: expected HTTP {}, got {answered:?}",
            row.case, row.as_bound_group
        );
        if row.as_bound_group == 403 {
            assert_eq!(
                answered.body["reason"],
                serde_json::json!("Forbidden"),
                "case {:?}: a refusal is the Forbidden Status, got {:?}",
                row.case,
                answered.body
            );
            let message = answered.body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(MAPPED_USERNAME),
                "case {:?}: the refusal must name the user it was taken for, got {message:?}",
                row.case
            );
        }
    }
}
