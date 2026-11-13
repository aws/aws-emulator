// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EKS add-on slice: `CreateAddon` and the `DescribeAddon`
//! read-back, driven over the wire through the same `MultiServiceRouter` the
//! production binary mounts.
//!
//! Every suite here asserts a RULE over a TABLE of inputs rather than one recorded
//! request: the rows differ along what the rule generalises over (which optional
//! members are supplied, the shape of a name or an ARN, the cluster's status and
//! region, the Kubernetes version a default is taken from), and each assertion names
//! its case and prints the actual value.
//!
//! The request harness (`Answer`, `emulator`, `call`, `encode_label`) is the one
//! every suite in this crate shares, from `tests/all/wire.rs`.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    ComAmazonawsEksAwswesleyfrontendState, ADDON_ACTIVATION_DELAY, ADDON_DEFAULT_NAMESPACE,
    CLUSTER_ACTIVATION_DELAY, MAX_ADDONS_PER_CLUSTER, MAX_KUBERNETES_NAME_LEN,
    MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::nodegroup_lifecycle::{create_nodegroup_in, minimal_request as nodegroup_request};
use crate::wire::{call, emulator, emulator_with_engine, encode_label, Answer};

/// The region every request in this suite is signed for, unless a case names
/// another to show that an add-on belongs to one Region's cluster.
const REGION: &str = "us-west-2";

/// An IAM role ARN of the form `CreateAddon$serviceAccountRoleArn` wants.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AddonRole";

/// The smallest request `CreateCluster` accepts: just the three required members.
fn cluster_request(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    })
}

async fn create_cluster_in(router: &MultiServiceRouter, region: &str, body: &serde_json::Value) {
    let answer = call(router, region, "POST", "/clusters", &body.to_string()).await;
    assert_eq!(
        answer.status, 200,
        "setup CreateCluster {}: got {answer:?}",
        body["name"]
    );
}

/// Wait out the span the clock owes a `CREATING` cluster, so the add-on layer that
/// requires an `ACTIVE` cluster is past.
async fn wait_for_cluster_active() {
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
}

/// One cluster, waited into `ACTIVE`, in [`REGION`].
async fn active_cluster(router: &MultiServiceRouter, name: &str) {
    create_cluster_in(router, REGION, &cluster_request(name)).await;
    wait_for_cluster_active().await;
}

/// The same, carrying one managed node group: a cluster with compute, so an
/// add-on whose workload has a replica to schedule has somewhere to place it and
/// its install is owed the running status rather than a degraded one.
async fn active_cluster_with_nodes(router: &MultiServiceRouter, name: &str) {
    active_cluster(router, name).await;
    let nodes = create_nodegroup_in(router, REGION, name, &nodegroup_request("nodes")).await;
    assert_eq!(
        nodes.status, 200,
        "setup CreateNodegroup on {name:?}: got {nodes:?}"
    );
}

async fn create_addon_in(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        region,
        "POST",
        &format!("/clusters/{}/addons", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

pub(crate) async fn create_addon(
    router: &MultiServiceRouter,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    create_addon_in(router, REGION, cluster, body).await
}

async fn describe_addon_in(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    addon: &str,
) -> Answer {
    call(
        router,
        region,
        "GET",
        &format!(
            "/clusters/{}/addons/{}",
            encode_label(cluster),
            encode_label(addon)
        ),
        "",
    )
    .await
}

pub(crate) async fn describe_addon(
    router: &MultiServiceRouter,
    cluster: &str,
    addon: &str,
) -> Answer {
    describe_addon_in(router, REGION, cluster, addon).await
}

/// One EKS Pod Identity association of a cluster in [`REGION`], read through the
/// pod identity slice — the reader for the associations an add-on owns.
pub(crate) async fn describe_association(
    router: &MultiServiceRouter,
    cluster: &str,
    association_id: &str,
) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/pod-identity-associations/{}",
            encode_label(cluster),
            encode_label(association_id)
        ),
        "",
    )
    .await
}

/// The add-on body with `status` dropped, so a read-back can be compared whole
/// without racing the `CREATING` -> `ACTIVE` move the clock owes it.
fn without_status(addon: &serde_json::Value) -> serde_json::Value {
    let mut copy = addon.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("status");
    }
    copy
}

// ---------------------------------------------------------------------------
// RULE: an add-on read back is the record that was written — every member the
// request supplied, plus the server-generated ones (`addonArn`, `createdAt`,
// `modifiedAt`, the assigned version, the association ARNs), unchanged.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: which optional members are
/// supplied at all, and the forms a supplied member takes (a JSON configuration
/// document and a YAML one, one association and several, tags, a namespace
/// override).
#[tokio::test]
async fn a_created_addon_reads_back_exactly_as_stored() {
    let router = emulator();
    active_cluster(&router, "read-back").await;

    // The explicit-version row names a build out of the catalog itself, narrowed to
    // the Kubernetes version the cluster runs: the builds a cluster is offered are
    // the ones `DescribeAddonVersions` reports for it.
    let cluster_version = default_cluster_version(&router).await;
    let coredns_build = catalog_default_version(&router, "coredns", &cluster_version)
        .await
        .unwrap_or_else(|| panic!("setup: the catalog publishes coredns for {cluster_version}"));

    let cases: Vec<(&str, serde_json::Value)> = vec![
        (
            "only the required members",
            serde_json::json!({ "addonName": "vpc-cni" }),
        ),
        (
            "an explicit version",
            serde_json::json!({ "addonName": "coredns", "addonVersion": coredns_build }),
        ),
        (
            "a service account role and tags",
            serde_json::json!({
                "addonName": "kube-proxy",
                "serviceAccountRoleArn": ROLE_ARN,
                "tags": { "team": "platform", "env": "test" },
            }),
        ),
        (
            "a JSON configuration document and a namespace override",
            serde_json::json!({
                "addonName": "aws-ebs-csi-driver",
                "configurationValues": "{\"replicaCount\":2}",
                "namespaceConfig": { "namespace": "csi-system" },
            }),
        ),
        (
            "a YAML configuration document",
            serde_json::json!({
                "addonName": "aws-efs-csi-driver",
                "configurationValues": "replicaCount: 2\n",
            }),
        ),
        (
            "several pod identity associations",
            serde_json::json!({
                "addonName": "amazon-cloudwatch-observability",
                "podIdentityAssociations": [
                    { "serviceAccount": "cloudwatch-agent", "roleArn": ROLE_ARN },
                    { "serviceAccount": "fluent-bit", "roleArn": ROLE_ARN },
                ],
            }),
        ),
    ];

    for (case, request) in &cases {
        let created = create_addon(&router, "read-back", request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateAddon, got {created:?}"
        );
        let addon_name = request["addonName"].as_str().expect("an addon name");

        // Every member the request supplied is reported back as given.
        assert_eq!(
            created.addon()["addonName"],
            serde_json::json!(addon_name),
            "case {case:?}: addonName must be echoed, got {:?}",
            created.addon()["addonName"]
        );
        assert_eq!(
            created.addon()["clusterName"],
            serde_json::json!("read-back"),
            "case {case:?}: clusterName must name the cluster it was created on, got {:?}",
            created.addon()["clusterName"]
        );
        for member in [
            "serviceAccountRoleArn",
            "configurationValues",
            "tags",
            "namespaceConfig",
        ] {
            if let Some(supplied) = request.get(member) {
                assert_eq!(
                    &created.addon()[member],
                    supplied,
                    "case {case:?}: {member} must be reported as supplied, got {:?}",
                    created.addon()[member]
                );
            }
        }

        // The server-generated members.
        let arn = created.addon()["addonArn"].as_str().unwrap_or_default();
        let expected_prefix =
            format!("arn:aws:eks:{REGION}:000000000000:addon/read-back/{addon_name}/");
        assert!(
            arn.starts_with(&expected_prefix),
            "case {case:?}: expected an ARN starting {expected_prefix:?}, got {arn:?}"
        );
        // An Amazon EKS-managed add-on reports no owner, publisher or Marketplace
        // listing: those identify the listing a Marketplace add-on is bought
        // through, and the catalog (`DescribeAddonVersions`) is where who publishes
        // and owns each published add-on is stated.
        for member in ["owner", "publisher", "marketplaceInformation"] {
            assert_eq!(
                created.addon()[member],
                serde_json::Value::Null,
                "case {case:?}: an EKS-managed add-on reports no {member}, got {:?}",
                created.addon()[member]
            );
        }
        assert_eq!(
            created.addon()["health"],
            serde_json::json!({ "issues": [] }),
            "case {case:?}: a healthy add-on reports no issues, got {:?}",
            created.addon()["health"]
        );
        assert!(
            created.addon()["createdAt"].is_number(),
            "case {case:?}: createdAt must be an epoch timestamp, got {:?}",
            created.addon()["createdAt"]
        );
        assert_eq!(
            created.addon()["modifiedAt"],
            created.addon()["createdAt"],
            "case {case:?}: a just-created add-on was last modified when it was created, \
             got {:?}",
            created.addon()["modifiedAt"]
        );

        // One association ARN per requested association, in the order asked for.
        let requested_accounts: Vec<&str> = request["podIdentityAssociations"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item["serviceAccount"].as_str().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        let reported = created.addon()["podIdentityAssociations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            reported,
            requested_accounts.len(),
            "case {case:?}: expected {} association ARNs, got {:?}",
            requested_accounts.len(),
            created.addon()["podIdentityAssociations"]
        );
        for (index, arn) in created.addon()["podIdentityAssociations"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .enumerate()
        {
            let arn = arn.as_str().unwrap_or_default();
            let prefix =
                format!("arn:aws:eks:{REGION}:000000000000:podidentityassociation/read-back/a-");
            assert!(
                arn.starts_with(&prefix),
                "case {case:?}: expected an association ARN starting {prefix:?}, got {arn:?}"
            );
            // The association the add-on claims to own EXISTS: it reads back through
            // the pod identity slice, for the service account the request named, in
            // the add-on's namespace, owned by this add-on.
            let association_id = arn.rsplit('/').next().unwrap_or_default();
            let association = describe_association(&router, "read-back", association_id).await;
            assert_eq!(
                association.status, 200,
                "case {case:?}: the association {association_id:?} the add-on owns must \
                 read back, got {association:?}"
            );
            let described_association = &association.body["association"];
            assert_eq!(
                described_association["serviceAccount"],
                serde_json::json!(requested_accounts[index]),
                "case {case:?}: association {index} is for the service account the \
                 request named, got {:?}",
                described_association["serviceAccount"]
            );
            assert_eq!(
                described_association["roleArn"],
                serde_json::json!(ROLE_ARN),
                "case {case:?}: association {index} binds the role the request named, \
                 got {:?}",
                described_association["roleArn"]
            );
            assert_eq!(
                described_association["namespace"],
                created.addon()["namespaceConfig"]["namespace"],
                "case {case:?}: association {index} lives in the add-on's namespace, \
                 got {:?}",
                described_association["namespace"]
            );
            assert_eq!(
                described_association["ownerArn"],
                created.addon()["addonArn"],
                "case {case:?}: association {index} is owned by the add-on that created \
                 it, got {:?}",
                described_association["ownerArn"]
            );
        }

        // The read-back: the same record, byte for byte apart from the status the
        // clock may already have moved.
        let described = describe_addon(&router, "read-back", addon_name).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 from DescribeAddon, got {described:?}"
        );
        assert_eq!(
            without_status(described.addon()),
            without_status(created.addon()),
            "case {case:?}: DescribeAddon must report what CreateAddon wrote"
        );
        let status = described.addon()["status"].as_str().unwrap_or_default();
        assert!(
            status == "CREATING" || status == "ACTIVE",
            "case {case:?}: an add-on settles from CREATING to ACTIVE, got {status:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: an add-on is a resource INSIDE one cluster, so a request naming a cluster
// the request's own region does not hold is 404 `ResourceNotFoundException` —
// whatever else the body says, and whether or not a cluster of that name exists
// somewhere else.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_cluster_that_does_not_exist_is_not_found_whatever_the_body_says() {
    let router = emulator();
    // A cluster that DOES exist, in another region, under the name the rows use:
    // a cluster name identifies a cluster only within one region.
    create_cluster_in(&router, "eu-west-1", &cluster_request("ghost")).await;
    wait_for_cluster_active().await;

    // The rows differ along what else is wrong with the request — nothing, an
    // add-on name of a form no add-on has, a configuration document that is not
    // valid JSON, a duplicated service account — because the cluster layer is
    // checked before any of them. What is NOT here is the request's own addressing:
    // the `@httpLabel` cluster name and the `serviceAccountRoleArn` this request
    // asks Amazon EKS to pass are judged BEFORE the cluster is resolved, which is
    // the `addon_request_precedence` suite's rule.
    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("nothing else wrong", serde_json::json!({ "addonName": "vpc-cni" })),
        (
            "an add-on name of an impossible form",
            serde_json::json!({ "addonName": "not an addon name" }),
        ),
        (
            "configurationValues that are not valid JSON",
            serde_json::json!({ "addonName": "vpc-cni", "configurationValues": "{nope" }),
        ),
        (
            "an association role ARN that is not a role ARN",
            serde_json::json!({
                "addonName": "vpc-cni",
                "podIdentityAssociations": [{ "serviceAccount": "sa", "roleArn": "nope" }],
            }),
        ),
    ];

    for (case, request) in &cases {
        let answer = create_addon(&router, "ghost", request).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404 for a cluster this region does not hold, \
             got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
    }

    // Reading an add-on back on a cluster that is not there is the same refusal,
    // and so is reading an add-on name the cluster does not hold.
    let missing_cluster = describe_addon(&router, "ghost", "vpc-cni").await;
    assert_eq!(
        missing_cluster.error_type.as_deref(),
        Some("ResourceNotFoundException"),
        "DescribeAddon on a cluster this region does not hold is a 404, got \
         {missing_cluster:?}"
    );
    active_cluster(&router, "real").await;
    let missing_addon = describe_addon(&router, "real", "vpc-cni").await;
    assert_eq!(
        missing_addon.status, 404,
        "DescribeAddon for an add-on that is not installed is a 404, got {missing_addon:?}"
    );
    assert_eq!(
        missing_addon.error_type.as_deref(),
        Some("ResourceNotFoundException"),
        "expected ResourceNotFoundException, got {missing_addon:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: an add-on install is ASYNCHRONOUS, so a cluster Amazon EKS is still
// provisioning takes one — the add-on is recorded in `CREATING` and its install
// settles only once that cluster is running. What is refused 400
// `InvalidRequestException` ("the request is invalid given the state of the
// cluster") is a cluster whose state no add-on of it can outlive.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_cluster_still_being_provisioned_takes_an_addon_that_settles_after_it() {
    let (router, engine) = emulator_with_engine();
    // Rows differ along the add-on name, which is the point: the cluster's state
    // decides the answer, not the body.
    let names = ["vpc-cni", "coredns", "kube-proxy"];

    create_cluster_in(&router, REGION, &cluster_request("settling")).await;
    // The instant the cluster is owed `ACTIVE` is held a KNOWN span out rather than
    // waited out, so what the rows below exercise is the state of the cluster rather
    // than how fast the test runs: every install lands while it is still `CREATING`.
    let cluster_hold = 2 * ADDON_ACTIVATION_DELAY;
    set_cluster_activation(&engine, "settling", Some(SystemTime::now() + cluster_hold));

    for name in names {
        let answer = create_addon(
            &router,
            "settling",
            &serde_json::json!({ "addonName": name }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: a cluster still being provisioned takes an add-on, got {answer:?}"
        );
        assert_eq!(
            answer.addon()["status"],
            serde_json::json!("CREATING"),
            "case {name:?}: the install has not settled, got {:?}",
            answer.addon()["status"]
        );
    }

    // The install cannot settle before its cluster does: a span after the install —
    // which is still inside the cluster's own span — every add-on still reports
    // `CREATING`.
    tokio::time::sleep(ADDON_ACTIVATION_DELAY + Duration::from_millis(100)).await;
    for name in names {
        let described = describe_addon(&router, "settling", name).await;
        assert_eq!(
            described.addon()["status"],
            serde_json::json!("CREATING"),
            "case {name:?}: an add-on of a cluster that is still CREATING has not \
             settled, got {:?}",
            described.addon()["status"]
        );
    }

    // A span after the cluster is owed `ACTIVE`, the add-ons have settled.
    tokio::time::sleep(cluster_hold + ADDON_ACTIVATION_DELAY + Duration::from_millis(250)).await;
    for name in names {
        let described = describe_addon(&router, "settling", name).await;
        assert_ne!(
            described.addon()["status"],
            serde_json::json!("CREATING"),
            "case {name:?}: the install settles once its cluster is running, got {:?}",
            described.addon()["status"]
        );
    }

    // A cluster whose state no add-on of it can outlive refuses the same request.
    // `FAILED` and `DELETING` are the two such states a cluster record can carry
    // (`smithy_query query="shape" shape="ClusterStatus"` reports
    // `CREATING | ACTIVE | DELETING | FAILED | UPDATING | PENDING`; a `PENDING`
    // cluster is a connected one, which the connected-cluster gate refuses first).
    for (case, status) in [("a failed cluster", "FAILED"), ("a deleting cluster", "DELETING")] {
        let name = format!("terminal-{status}");
        create_cluster_in(&router, REGION, &cluster_request(&name)).await;
        {
            let store = engine.state::<ComAmazonawsEksAwswesleyfrontendState>();
            let mut guard = store.lock();
            let record = guard
                .clusters
                .get_mut(&(REGION.to_string(), name.clone()))
                .unwrap_or_else(|| panic!("case {case}: the cluster must be in the store"));
            record.active_at = None;
            record.status = status.to_string();
        }
        let answer = create_addon(&router, &name, &serde_json::json!({ "addonName": "vpc-cni" })).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidRequestException"),
            "case {case}: expected InvalidRequestException, got {answer:?}"
        );
        let described = describe_addon(&router, &name, "vpc-cni").await;
        assert_eq!(
            described.status, 404,
            "case {case}: a refused create must store nothing, got {described:?}"
        );
    }
}

/// Hold the instant one cluster's move to `ACTIVE` is owed at, so a suite drives
/// the cluster's STATE rather than the wall clock.
fn set_cluster_activation(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    cluster: &str,
    active_at: Option<SystemTime>,
) {
    let store = engine.state::<ComAmazonawsEksAwswesleyfrontendState>();
    let mut guard = store.lock();
    let record = guard
        .clusters
        .get_mut(&(REGION.to_string(), cluster.to_string()))
        .unwrap_or_else(|| panic!("the cluster {cluster:?} must be in the store"));
    record.active_at = active_at;
    record.status = "CREATING".to_string();
}

// ---------------------------------------------------------------------------
// RULE: a member whose value cannot be what the model says it is is refused 400
// `InvalidParameterException`, and nothing is written.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn members_the_model_cannot_constrain_are_refused_as_invalid_parameters() {
    let router = emulator();
    active_cluster(&router, "validated").await;

    // Source for the rows: `CreateAddon`'s input members, from
    // `smithy_query query="operation" operation="CreateAddon"` — `addonName` is a
    // plain required `String` (no length, no pattern), `configurationValues` is a
    // free-form document, and `podIdentityAssociations` maps a service account to a
    // role. Each row is a value those constraints accept but the service cannot.
    // `serviceAccountRoleArn` is NOT here: it is a role the request asks Amazon EKS to
    // pass, so it is judged by the pass-role rule ahead of the body
    // ([`crate::pass_role_authorization`], [`crate::addon_request_precedence`]).
    let refused: Vec<(&str, serde_json::Value)> = vec![
        ("an empty addonName", serde_json::json!({ "addonName": "" })),
        (
            "an addonName with a space",
            serde_json::json!({ "addonName": "vpc cni" }),
        ),
        (
            "an addonName with a path separator",
            serde_json::json!({ "addonName": "vpc/cni" }),
        ),
        (
            "an addonName one character past the length ceiling",
            serde_json::json!({ "addonName": "a".repeat(MAX_KUBERNETES_NAME_LEN + 1) }),
        ),
        (
            "configurationValues that open a JSON object and do not close it",
            serde_json::json!({ "addonName": "vpc-cni", "configurationValues": "{\"a\":" }),
        ),
        (
            "configurationValues that open a JSON array and do not close it",
            serde_json::json!({ "addonName": "vpc-cni", "configurationValues": "[1," }),
        ),
        (
            "configurationValues written in YAML that set a key the schema does not declare",
            serde_json::json!({
                "addonName": "vpc-cni",
                "configurationValues": "notAProperty:\n  LOG_LEVEL: debug\n",
            }),
        ),
        (
            "configurationValues written in YAML that state no object at all",
            serde_json::json!({ "addonName": "vpc-cni", "configurationValues": "useFIPS" }),
        ),
        (
            "an association roleArn that is not a role ARN",
            serde_json::json!({
                "addonName": "vpc-cni",
                "podIdentityAssociations": [
                    { "serviceAccount": "sa", "roleArn": "arn:aws:iam::000000000000:role/" },
                ],
            }),
        ),
        (
            "the same serviceAccount mapped twice",
            serde_json::json!({
                "addonName": "vpc-cni",
                "podIdentityAssociations": [
                    { "serviceAccount": "sa", "roleArn": ROLE_ARN },
                    { "serviceAccount": "sa", "roleArn": ROLE_ARN },
                ],
            }),
        ),
        (
            "a serviceAccount no Kubernetes object can be named",
            serde_json::json!({
                "addonName": "vpc-cni",
                "podIdentityAssociations": [
                    { "serviceAccount": "Not A Name", "roleArn": ROLE_ARN },
                ],
            }),
        ),
        (
            "a namespace no Kubernetes object can be named",
            serde_json::json!({
                "addonName": "vpc-cni",
                "namespaceConfig": { "namespace": "-kube-system" },
            }),
        ),
    ];

    for (case, request) in &refused {
        let answer = create_addon(&router, "validated", request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
        // A refused request writes nothing.
        let name = request["addonName"].as_str().unwrap_or_default();
        if !name.is_empty() {
            let described = describe_addon(&router, "validated", name).await;
            assert_eq!(
                described.status, 404,
                "case {case:?}: a refused create must store nothing, got {described:?}"
            );
        }
    }

    // The accepted side of the same members: every form the service does take. Each
    // row names a DIFFERENT add-on the catalog publishes, because one add-on installs
    // at most once per cluster, and the rows differ along the member whose form they
    // exercise. `priorityClassName` is one of the keys every published add-on's
    // configuration schema declares (source: the common properties
    // `DescribeAddonConfiguration` publishes, read in this crate's `addon_versions`
    // suite).
    let accepted: Vec<(&str, serde_json::Value)> = vec![
        (
            "an add-on name with an underscore (a Marketplace add-on name)",
            serde_json::json!({ "addonName": "dynatrace_dynatrace-operator" }),
        ),
        (
            "a role ARN with a path",
            serde_json::json!({
                "addonName": "aws-ebs-csi-driver",
                "serviceAccountRoleArn":
                    "arn:aws:iam::000000000000:role/aws-service-role/eks.amazonaws.com/Role",
            }),
        ),
        (
            "configurationValues that are a JSON object",
            serde_json::json!({
                "addonName": "metrics-server",
                "configurationValues": "{\"priorityClassName\":\"system-cluster-critical\"}",
            }),
        ),
        (
            "configurationValues written in YAML rather than JSON",
            serde_json::json!({
                "addonName": "kube-state-metrics",
                "configurationValues": "priorityClassName: system-cluster-critical\n",
            }),
        ),
        (
            "an empty configurationValues document",
            serde_json::json!({ "addonName": "snapshot-controller", "configurationValues": "" }),
        ),
    ];

    for (case, request) in &accepted {
        let answer = create_addon(&router, "validated", request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the version an add-on reports is the build the caller named out of the
// ones the catalog offers that cluster; with none named it is the build the
// catalog publishes as the DEFAULT for the cluster's own Kubernetes version.
// ---------------------------------------------------------------------------

/// Every build the catalog offers one add-on on one Kubernetes version, newest
/// first — the builds `DescribeAddonVersions` reports when it is narrowed to that
/// version, so this suite checks `CreateAddon` against the catalog a caller reads
/// rather than against a repeated constant.
pub(crate) async fn catalog_builds(
    router: &MultiServiceRouter,
    addon_name: &str,
    kubernetes_version: &str,
) -> Vec<serde_json::Value> {
    let answer = call(
        router,
        REGION,
        "GET",
        &format!(
            "/addons/supported-versions?addonName={}&kubernetesVersion={}",
            encode_label(addon_name),
            encode_label(kubernetes_version)
        ),
        "",
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup DescribeAddonVersions {addon_name}/{kubernetes_version}: got {answer:?}"
    );
    answer.body["addons"]
        .as_array()
        .and_then(|addons| addons.first())
        .and_then(|addon| addon.get("addonVersions"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `addonVersion` of one reported build.
fn build_version(build: &serde_json::Value) -> String {
    build["addonVersion"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an addonVersion, got {build:?}"))
        .to_owned()
}

/// The build the catalog publishes as the default one of an add-on for one
/// Kubernetes version — the one `CreateAddon` installs when the caller names none.
pub(crate) async fn catalog_default_version(
    router: &MultiServiceRouter,
    addon_name: &str,
    kubernetes_version: &str,
) -> Option<String> {
    catalog_builds(router, addon_name, kubernetes_version)
        .await
        .iter()
        .find(|build| {
            build["compatibilities"]
                .as_array()
                .is_some_and(|compatibilities| {
                    compatibilities
                        .iter()
                        .any(|compatibility| compatibility["defaultVersion"].as_bool() == Some(true))
                })
        })
        .map(build_version)
}

/// The Kubernetes versions `DescribeClusterVersions` publishes for a cluster to be
/// created on, newest first: the published rows the clock has not carried past
/// their own end of extended support, which is the set `CreateCluster` accepts —
/// the listing publishes the whole calendar, out-of-support rows included, and
/// each row's `versionStatus` is what says which of them a cluster can run.
pub(crate) async fn published_cluster_versions(router: &MultiServiceRouter) -> Vec<String> {
    published_calendar_rows(router)
        .await
        .iter()
        .filter(|row| row["versionStatus"].as_str() != Some("UNSUPPORTED"))
        .map(|row| {
            row["clusterVersion"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a clusterVersion, got {row:?}"))
                .to_owned()
        })
        .collect()
}

/// EVERY row the published Kubernetes version calendar carries, newest first —
/// the out-of-support rows included, which is what a suite needs when it asks
/// about the versions the add-on catalog's compatibilities name rather than about
/// the versions a cluster can be created on ([`published_cluster_versions`] is
/// that narrower reading of this same listing).
pub(crate) async fn published_calendar_rows(
    router: &MultiServiceRouter,
) -> Vec<serde_json::Value> {
    let answer = call(router, REGION, "GET", "/cluster-versions?maxResults=100", "").await;
    assert_eq!(
        answer.status, 200,
        "setup DescribeClusterVersions: got {answer:?}"
    );
    answer.body["clusterVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a clusterVersions list, got {:?}", answer.body))
        .clone()
}

/// Every add-on name the catalog publishes, in the order it publishes them — the
/// names `CreateAddon` installs, read out of the catalog itself so a suite that
/// needs several distinct add-ons names no constant.
pub(crate) async fn published_addon_names(router: &MultiServiceRouter) -> Vec<String> {
    let answer = call(
        router,
        REGION,
        "GET",
        "/addons/supported-versions?maxResults=100",
        "",
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup DescribeAddonVersions: got {answer:?}"
    );
    answer.body["addons"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an addons list, got {:?}", answer.body))
        .iter()
        .map(|addon| {
            addon["addonName"]
                .as_str()
                .unwrap_or_else(|| panic!("expected an addonName, got {addon:?}"))
                .to_owned()
        })
        .collect()
}

/// The Kubernetes version a cluster created without one lands on — the version
/// `DescribeClusterVersions` reports as the default, and so the version whose
/// default add-on builds a cluster in this suite is offered.
pub(crate) async fn default_cluster_version(router: &MultiServiceRouter) -> String {
    let answer = call(router, REGION, "GET", "/cluster-versions?defaultOnly=true", "").await;
    assert_eq!(
        answer.status, 200,
        "setup DescribeClusterVersions(defaultOnly): got {answer:?}"
    );
    answer.body["clusterVersions"]
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(|row| row["clusterVersion"].as_str())
        .unwrap_or_else(|| panic!("expected one default cluster version, got {:?}", answer.body))
        .to_owned()
}

#[tokio::test]
async fn the_addon_version_is_the_one_named_or_the_cluster_default() {
    let router = emulator();
    // Rows differ along the cluster's Kubernetes version — which is what both the
    // default build and the set of builds the cluster is offered are taken from —
    // taking the newest version the published calendar carries and the oldest, so a
    // version a newer build exists for and one none does are both covered.
    let calendar = published_cluster_versions(&router).await;
    let versions = [
        calendar
            .first()
            .expect("the calendar publishes a version")
            .clone(),
        calendar
            .last()
            .expect("the calendar publishes a version")
            .clone(),
    ];
    for version in &versions {
        let mut request = cluster_request(&format!("k8s-{}", version.replace('.', "-")));
        request["version"] = serde_json::json!(version);
        create_cluster_in(&router, REGION, &request).await;
    }
    wait_for_cluster_active().await;

    for version in &versions {
        let cluster = format!("k8s-{}", version.replace('.', "-"));
        let expected = catalog_default_version(&router, "vpc-cni", version)
            .await
            .unwrap_or_else(|| panic!("setup {version}: the catalog publishes a default build"));
        let defaulted = create_addon(
            &router,
            &cluster,
            &serde_json::json!({ "addonName": "vpc-cni" }),
        )
        .await;
        assert_eq!(
            defaulted.status, 200,
            "case {version:?}: expected 200, got {defaulted:?}"
        );
        assert_eq!(
            defaulted.addon()["addonVersion"],
            serde_json::json!(expected),
            "case {version:?}: expected the default version for the cluster's Kubernetes \
             version, got {:?}",
            defaulted.addon()["addonVersion"]
        );

        // A named build is kept verbatim, for every add-on the request can name it
        // for: the rows differ along the add-on (each carries its own version line)
        // and along which of the builds offered for this Kubernetes version is named
        // — the newest offered one, which for a version a newer build exists for is
        // NOT the default, and the default one.
        for addon in ["coredns", "kube-proxy", "metrics-server"] {
            let offered = catalog_builds(&router, addon, version).await;
            assert!(
                !offered.is_empty(),
                "setup {addon}/{version}: the catalog must offer a build"
            );
            let named = build_version(&offered[0]);
            let answer = create_addon(
                &router,
                &cluster,
                &serde_json::json!({ "addonName": addon, "addonVersion": named }),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {version:?}/{addon}/{named:?}: expected 200, got {answer:?}"
            );
            assert_eq!(
                answer.addon()["addonVersion"],
                serde_json::json!(named),
                "case {version:?}/{addon}/{named:?}: a named build is kept, got {:?}",
                answer.addon()["addonVersion"]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: one add-on name installs at most once per cluster — a repeat conflicts
// 409 `ResourceInUseException`, unless it replays the `@idempotencyToken` that
// created it, in which case that same add-on is answered again.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_retry_with_the_same_idempotency_token_is_absorbed() {
    /// One row of the table: the case name, the two requests' tokens, the status
    /// the second call must answer, and whether the add-on also asks for an EKS
    /// Pod Identity association.
    type Retry = (String, Option<&'static str>, Option<&'static str>, u16, bool);
    // Rows differ along the relationship between the two requests' tokens: equal,
    // different, present-then-absent, absent-then-present, absent on both, and
    // blank on both (a blank token identifies nothing, so it is not a retry
    // marker). Each row is run twice: once for an add-on with no optional members,
    // and once for one that CREATES child resources (an EKS Pod Identity
    // association), because a retry must be absorbed rather than collide with the
    // children its own first call created.
    let cases: Vec<(&str, Option<&str>, Option<&str>, u16)> = vec![
        ("the same token twice", Some("tok-1"), Some("tok-1"), 200),
        ("a different token", Some("tok-1"), Some("tok-2"), 409),
        ("a token, then none", Some("tok-1"), None, 409),
        ("no token, then one", None, Some("tok-1"), 409),
        ("no token either time", None, None, 409),
        ("an empty token twice", Some(""), Some(""), 409),
        ("a blank token twice", Some("   "), Some("   "), 409),
    ];

    let router = emulator();
    active_cluster(&router, "idempotent").await;

    let expanded: Vec<Retry> = cases
        .iter()
        .flat_map(|(case, first, second, expected)| {
            [
                ((*case).to_string(), *first, *second, *expected, false),
                (
                    format!("{case} (with an association)"),
                    *first,
                    *second,
                    *expected,
                    true,
                ),
            ]
        })
        .collect();

    // One add-on name installs at most once per cluster, so each row drives a
    // DIFFERENT add-on of the published catalog.
    let published = published_addon_names(&router).await;
    assert!(
        published.len() >= expanded.len(),
        "setup: the catalog must publish at least {} add-ons, got {}",
        expanded.len(),
        published.len()
    );

    for (index, (case, first_token, second_token, expected, with_associations)) in
        expanded.iter().enumerate()
    {
        let with_associations = *with_associations;
        let name = published[index].clone();
        let request = |token: &Option<&str>| {
            let mut body = serde_json::json!({ "addonName": name });
            if let Some(token) = token {
                body["clientRequestToken"] = serde_json::json!(token);
            }
            if with_associations {
                body["podIdentityAssociations"] = serde_json::json!([{
                    "serviceAccount": format!("sa-{index}"),
                    "roleArn": ROLE_ARN,
                }]);
            }
            body
        };

        let first = create_addon(&router, "idempotent", &request(first_token)).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first CreateAddon succeeds, got {first:?}"
        );
        let second = create_addon(&router, "idempotent", &request(second_token)).await;
        assert_eq!(
            second.status, *expected,
            "case {case:?}: expected {expected}, got {second:?}"
        );
        if *expected == 200 {
            assert_eq!(
                second.addon()["addonArn"],
                first.addon()["addonArn"],
                "case {case:?}: an absorbed retry answers with the add-on the token \
                 created, got {:?}",
                second.addon()["addonArn"]
            );
            assert_eq!(
                second.addon()["createdAt"],
                first.addon()["createdAt"],
                "case {case:?}: an absorbed retry creates nothing new, got {:?}",
                second.addon()["createdAt"]
            );
            assert_eq!(
                second.addon()["podIdentityAssociations"],
                first.addon()["podIdentityAssociations"],
                "case {case:?}: an absorbed retry mints no second association, got {:?}",
                second.addon()["podIdentityAssociations"]
            );
        } else {
            assert_eq!(
                second.error_type.as_deref(),
                Some("ResourceInUseException"),
                "case {case:?}: expected ResourceInUseException, got {second:?}"
            );
            // The conflict changed nothing: the stored record is still the first one.
            let described = describe_addon(&router, "idempotent", &name).await;
            assert_eq!(
                described.addon()["createdAt"],
                first.addon()["createdAt"],
                "case {case:?}: a refused repeat must not rewrite the record"
            );
            assert_eq!(
                described.addon()["podIdentityAssociations"],
                first.addon()["podIdentityAssociations"],
                "case {case:?}: a refused repeat must not mint another association"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: an add-on belongs to ONE cluster in ONE region, so the same add-on name
// on two clusters is two add-ons, and removing a cluster removes its add-ons.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn addons_are_scoped_to_their_own_cluster_and_region() {
    // Rows differ along what distinguishes the two clusters: the name, and the
    // region a request is signed for (the only place a request states one).
    let cases = [
        ("two clusters in one region", REGION, "left", REGION, "right"),
        ("the same name in two regions", "us-east-2", "same", "eu-west-3", "same"),
    ];

    for (case, left_region, left, right_region, right) in cases {
        let router = emulator();
        create_cluster_in(&router, left_region, &cluster_request(left)).await;
        create_cluster_in(&router, right_region, &cluster_request(right)).await;
        wait_for_cluster_active().await;

        let addon = "vpc-cni";
        let body = serde_json::json!({ "addonName": addon, "tags": { "side": "left" } });
        let on_left = create_addon_in(&router, left_region, left, &body).await;
        assert_eq!(
            on_left.status, 200,
            "case {case:?}: expected 200 on the left cluster, got {on_left:?}"
        );

        // The right-hand cluster does not have it...
        let before = describe_addon_in(&router, right_region, right, addon).await;
        assert_eq!(
            before.status, 404,
            "case {case:?}: the other cluster does not hold the add-on, got {before:?}"
        );
        // ...so creating the same name there is not a conflict, and it is a
        // different add-on.
        let on_right = create_addon_in(
            &router,
            right_region,
            right,
            &serde_json::json!({ "addonName": addon, "tags": { "side": "right" } }),
        )
        .await;
        assert_eq!(
            on_right.status, 200,
            "case {case:?}: the same add-on name on another cluster is a new add-on, \
             got {on_right:?}"
        );
        assert_ne!(
            on_right.addon()["addonArn"],
            on_left.addon()["addonArn"],
            "case {case:?}: two add-ons must not share an ARN, got {:?}",
            on_right.addon()["addonArn"]
        );
        assert_eq!(
            on_right.addon()["tags"],
            serde_json::json!({ "side": "right" }),
            "case {case:?}: each add-on keeps its own members, got {:?}",
            on_right.addon()["tags"]
        );

        // Removing the left cluster removes the add-ons installed on it, and leaves
        // the other cluster's alone.
        let removed = call(
            &router,
            left_region,
            "DELETE",
            &format!("/clusters/{}", encode_label(left)),
            "",
        )
        .await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: setup DeleteCluster, got {removed:?}"
        );
        let after = describe_addon_in(&router, left_region, left, addon).await;
        assert_eq!(
            after.status, 404,
            "case {case:?}: a deleted cluster holds no add-ons, got {after:?}"
        );
        let survivor = describe_addon_in(&router, right_region, right, addon).await;
        assert_eq!(
            survivor.status, 200,
            "case {case:?}: the other cluster's add-on survives, got {survivor:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a status move the clock owes is materialised by whichever read first runs
// after it is due — and not before.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_new_addon_reports_creating_until_the_clock_owes_it_active() {
    let router = emulator();
    active_cluster_with_nodes(&router, "settling-addons").await;

    let names = ["vpc-cni", "coredns"];
    for name in names {
        let created = create_addon(
            &router,
            "settling-addons",
            &serde_json::json!({ "addonName": name }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected 200 from CreateAddon, got {created:?}"
        );
        assert_eq!(
            created.addon()["status"],
            serde_json::json!("CREATING"),
            "case {name:?}: CreateAddon reports CREATING, got {:?}",
            created.addon()["status"]
        );
        // A caller acting straight after the create still observes the transient
        // state: the span is never zero.
        let immediately = describe_addon(&router, "settling-addons", name).await;
        assert_eq!(
            immediately.addon()["status"],
            serde_json::json!("CREATING"),
            "case {name:?}: a describe straight after the create still sees CREATING, \
             got {:?}",
            immediately.addon()["status"]
        );
    }

    tokio::time::sleep(ADDON_ACTIVATION_DELAY + Duration::from_millis(250)).await;

    for name in names {
        let described = describe_addon(&router, "settling-addons", name).await;
        assert_eq!(
            described.addon()["status"],
            serde_json::json!("ACTIVE"),
            "case {name:?}: once the activation delay has passed the add-on is ACTIVE, \
             got {:?}",
            described.addon()["status"]
        );
        // Re-reading does not move it again or disturb anything else.
        let twice = describe_addon(&router, "settling-addons", name).await;
        assert_eq!(
            twice.addon(),
            described.addon(),
            "case {name:?}: a second read must answer identically"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the add-ons one cluster can hold are the ones the catalog publishes for
// its Kubernetes version — every one of them installs, a name outside that set
// never does, and a refused create writes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_cluster_holds_exactly_the_addons_the_catalog_publishes_for_it() {
    let router = emulator();
    active_cluster(&router, "saturated").await;

    // What bounds this part of the store: the catalog, which is a fixed table, so
    // one cluster can hold no more add-ons than it publishes — well inside the
    // ceiling `CreateAddon` keeps as its backstop.
    let published = published_addon_names(&router).await;
    assert!(
        !published.is_empty() && published.len() <= MAX_ADDONS_PER_CLUSTER,
        "setup: the catalog must publish between one and {MAX_ADDONS_PER_CLUSTER} add-ons, \
         got {}",
        published.len()
    );
    for name in &published {
        let answer = create_addon(
            &router,
            "saturated",
            &serde_json::json!({ "addonName": name }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: every add-on the catalog publishes installs, got {answer:?}"
        );
    }

    // Rows differ only in the name, which is the point: with the catalog exhausted
    // no further name is accepted, whatever it is.
    for case in ["one-more", "another", "yet-another"] {
        let answer = create_addon(
            &router,
            "saturated",
            &serde_json::json!({ "addonName": case }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for a name the catalog does not publish, got \
             {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
        let described = describe_addon(&router, "saturated", case).await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: the refused create must store nothing, got {described:?}"
        );
    }

    // The set is per cluster, so another cluster takes the same add-ons again.
    active_cluster(&router, "roomy").await;
    let answer = create_addon(
        &router,
        "roomy",
        &serde_json::json!({ "addonName": published[0] }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the add-ons a cluster holds are its own, so another cluster still accepts one, \
         got {answer:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: the EKS Pod Identity associations an add-on creates are associations of
// its cluster like any others — they count against the cluster's association
// ceiling, and a `(namespace, service account)` pair that already holds one
// cannot take a second (409 `ResourceInUseException`). A request that cannot be
// carried out in full writes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_associations_an_addon_creates_obey_the_cluster_association_rules() {
    let router = emulator();
    active_cluster(&router, "associated").await;

    // The association list is refused by LENGTH before any entry is looked at:
    // the model puts no bound on `AddonPodIdentityAssociationsList`, so a list
    // larger than a cluster can ever hold is refused on its size alone. Rows: the
    // ceiling itself (accepted on count, below), one past it, and far past it.
    let association_list = |count: usize| -> serde_json::Value {
        (0..count)
            .map(|index| {
                serde_json::json!({
                    "serviceAccount": format!("sa-{index}"),
                    "roleArn": ROLE_ARN,
                })
            })
            .collect()
    };
    for over in [
        MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER + 1,
        MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER * 2,
    ] {
        let answer = create_addon(
            &router,
            "associated",
            &serde_json::json!({
                "addonName": "vpc-cni",
                "podIdentityAssociations": association_list(over),
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {over} associations: expected 400 past the ceiling, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidRequestException"),
            "case {over} associations: expected InvalidRequestException, got {answer:?}"
        );
        let described = describe_addon(&router, "associated", "vpc-cni").await;
        assert_eq!(
            described.status, 404,
            "case {over} associations: the refused create must store nothing, got \
             {described:?}"
        );
    }

    // A pair that already holds an association cannot take a second. The first
    // association is created through the pod identity slice's own API, in the
    // namespace an add-on installs into by default.
    let taken = call(
        &router,
        REGION,
        "POST",
        "/clusters/associated/pod-identity-associations",
        &serde_json::json!({
            "namespace": ADDON_DEFAULT_NAMESPACE,
            "serviceAccount": "shared-sa",
            "roleArn": ROLE_ARN,
        })
        .to_string(),
    )
    .await;
    assert_eq!(taken.status, 200, "setup CreatePodIdentityAssociation: got {taken:?}");

    // Rows differ in where the conflicting entry sits in the list: alone, first,
    // and last after an entry that would otherwise be created.
    let cases: Vec<(&str, serde_json::Value)> = vec![
        (
            "the conflicting pair alone",
            serde_json::json!([{ "serviceAccount": "shared-sa", "roleArn": ROLE_ARN }]),
        ),
        (
            "the conflicting pair first",
            serde_json::json!([
                { "serviceAccount": "shared-sa", "roleArn": ROLE_ARN },
                { "serviceAccount": "other-sa", "roleArn": ROLE_ARN },
            ]),
        ),
        (
            "the conflicting pair last",
            serde_json::json!([
                { "serviceAccount": "other-sa", "roleArn": ROLE_ARN },
                { "serviceAccount": "shared-sa", "roleArn": ROLE_ARN },
            ]),
        ),
    ];
    for (case, associations) in &cases {
        let answer = create_addon(
            &router,
            "associated",
            &serde_json::json!({
                "addonName": "coredns",
                "podIdentityAssociations": associations,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 409,
            "case {case:?}: expected 409 for a pair that already holds an association, \
             got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("ResourceInUseException"),
            "case {case:?}: expected ResourceInUseException, got {answer:?}"
        );
        // Neither the add-on nor the association that WOULD have been created is
        // written: a request that cannot be carried out in full writes nothing.
        let described = describe_addon(&router, "associated", "coredns").await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: the refused create must store no add-on, got {described:?}"
        );
        // The association created in setup is untouched.
        let setup_id = taken.body["association"]["associationId"]
            .as_str()
            .unwrap_or_default();
        let still_there = describe_association(&router, "associated", setup_id).await;
        assert_eq!(
            still_there.status, 200,
            "case {case:?}: the association created in setup must survive, got \
             {still_there:?}"
        );
    }

    // Nothing partial was written by those refusals: the pair the refused lists also
    // named is still free, so an add-on can take it.
    let freed = create_addon(
        &router,
        "associated",
        &serde_json::json!({
            "addonName": "kube-proxy",
            "podIdentityAssociations": [{ "serviceAccount": "other-sa", "roleArn": ROLE_ARN }],
        }),
    )
    .await;
    assert_eq!(
        freed.status, 200,
        "a refused create must leave the pairs it also named free, got {freed:?}"
    );

    // A disjoint pair is accepted, and the association it creates is the add-on's.
    let accepted = create_addon(
        &router,
        "associated",
        &serde_json::json!({
            "addonName": "metrics-server",
            "podIdentityAssociations": [{ "serviceAccount": "own-sa", "roleArn": ROLE_ARN }],
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "a disjoint (namespace, service account) pair is accepted, got {accepted:?}"
    );
    let association_arn = accepted.addon()["podIdentityAssociations"][0]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let association_id = association_arn.rsplit('/').next().unwrap_or_default();
    let association = describe_association(&router, "associated", association_id).await;
    assert_eq!(
        association.body["association"]["ownerArn"],
        accepted.addon()["addonArn"],
        "the association the add-on created is owned by it, got {:?}",
        association.body["association"]["ownerArn"]
    );
}
