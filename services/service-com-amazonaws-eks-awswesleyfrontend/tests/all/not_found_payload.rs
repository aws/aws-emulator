// SPDX-License-Identifier: CC0-1.0

//! The SHAPE of this service's 404 body: which members a
//! `ResourceNotFoundException` carries, across the operations that can raise one.
//!
//! RULE: EKS never echoes the identifier of a resource INSIDE a cluster into a
//! not-found payload. The exception's optional identity members
//! (`clusterName`, `nodegroupName`, `fargateProfileName`, `addonName`,
//! `subscriptionId` — `smithy_query query="shape"
//! shape="ResourceNotFoundException"`, none required) report at most the ONE resource
//! the operation says is missing:
//!
//! * an operation that resolves a resource inside a cluster addresses ONE resource
//!   (the `(cluster, child)` pair), so it reports the miss generically — the generic
//!   sentence and NO identity member, whichever half is absent;
//! * an operation that only validates the cluster before creating in it or
//!   enumerating it reports the CLUSTER — `clusterName` set and the message that names
//!   the same cluster — and no sub-resource member.
//!
//! The tables below span the operations the rule divides, their HTTP shapes (a
//! path-only GET and DELETE, a POST carrying a body), the name forms a request can
//! carry, the region it is signed for, and WHICH half of the pair is missing (a
//! cluster that was never created, and a real cluster that holds no such child). The
//! recorded corpus's one request is a single row of this.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    cluster_not_found_message, CLUSTER_ACTIVATION_DELAY, RESOURCE_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{access_entry_cluster_request, REGION};
use crate::wire::{call, emulator, encode_label, Answer};

/// Every resource-identity member `ResourceNotFoundException` declares beside
/// `message` (`smithy_query query="shape" shape="ResourceNotFoundException"`).
pub(crate) const IDENTITY_MEMBERS: &[&str] = &[
    "clusterName",
    "nodegroupName",
    "fargateProfileName",
    "addonName",
    "subscriptionId",
];

/// The SUB-RESOURCE identity members: the ones no EKS not-found payload carries, on
/// either side of the rule.
const CHILD_IDENTITY_MEMBERS: &[&str] = &[
    "nodegroupName",
    "fargateProfileName",
    "addonName",
    "subscriptionId",
];

/// The IAM principal the access-entry rows address, and the role a capability uses.
const PRINCIPAL: &str = "arn:aws:iam::000000000000:role/not-found-probe";
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/eksCapabilityRole";
const VIEW_POLICY: &str = "arn:aws:eks::aws:cluster-access-policy/AmazonEKSViewPolicy";
/// The node role a `CreateNodegroup` row passes to its nodes.
const NODE_ROLE: &str = "arn:aws:iam::000000000000:role/eksNodeRole";

/// One request, as a row of the tables: the operation it calls and the HTTP shape an
/// SDK would send for it (`OPERATIONS.md` carries each route).
struct Row {
    operation: &'static str,
    method: &'static str,
    uri: String,
    body: String,
}

fn row(operation: &'static str, method: &'static str, uri: String, body: serde_json::Value) -> Row {
    Row {
        operation,
        method,
        uri,
        body: if body.is_null() {
            String::new()
        } else {
            body.to_string()
        },
    }
}

/// The operations that resolve a resource INSIDE a cluster: each addresses the
/// `(cluster, child)` pair, named by path labels or by the body.
fn pair_resolving(cluster: &str) -> Vec<Row> {
    let name = encode_label(cluster);
    let principal = encode_label(PRINCIPAL);
    vec![
        row(
            "DescribeAccessEntry",
            "GET",
            format!("/clusters/{name}/access-entries/{principal}"),
            serde_json::Value::Null,
        ),
        row(
            "AssociateAccessPolicy",
            "POST",
            format!("/clusters/{name}/access-entries/{principal}/access-policies"),
            serde_json::json!({
                "policyArn": VIEW_POLICY,
                "accessScope": { "type": "cluster" },
            }),
        ),
        row(
            "ListAssociatedAccessPolicies",
            "GET",
            format!("/clusters/{name}/access-entries/{principal}/access-policies"),
            serde_json::Value::Null,
        ),
        row(
            "DescribeAddon",
            "GET",
            format!("/clusters/{name}/addons/vpc-cni"),
            serde_json::Value::Null,
        ),
        row(
            "DescribeCapability",
            "GET",
            format!("/clusters/{name}/capabilities/probe-capability"),
            serde_json::Value::Null,
        ),
        row(
            "DeleteCapability",
            "DELETE",
            format!("/clusters/{name}/capabilities/probe-capability"),
            serde_json::Value::Null,
        ),
        row(
            "DescribeNodegroup",
            "GET",
            format!("/clusters/{name}/node-groups/probe-nodegroup"),
            serde_json::Value::Null,
        ),
        row(
            "DescribeIdentityProviderConfig",
            "POST",
            format!("/clusters/{name}/identity-provider-configs/describe"),
            serde_json::json!({
                "identityProviderConfig": { "type": "oidc", "name": "probe-config" },
            }),
        ),
    ]
}

/// The operations that only validate the cluster: they create in it or enumerate it,
/// so the cluster is the one resource they can report missing.
fn cluster_resolving(cluster: &str) -> Vec<Row> {
    let name = encode_label(cluster);
    vec![
        row(
            "DescribeCluster",
            "GET",
            format!("/clusters/{name}"),
            serde_json::Value::Null,
        ),
        row(
            "ListAccessEntries",
            "GET",
            format!("/clusters/{name}/access-entries"),
            serde_json::Value::Null,
        ),
        row(
            "CreateAccessEntry",
            "POST",
            format!("/clusters/{name}/access-entries"),
            serde_json::json!({ "principalArn": PRINCIPAL }),
        ),
        row(
            "CreateAddon",
            "POST",
            format!("/clusters/{name}/addons"),
            serde_json::json!({ "addonName": "vpc-cni" }),
        ),
        row(
            "CreateNodegroup",
            "POST",
            format!("/clusters/{name}/node-groups"),
            serde_json::json!({
                "nodegroupName": "probe-nodegroup",
                "nodeRole": NODE_ROLE,
                "subnets": ["subnet-aaaa1111"],
            }),
        ),
        row(
            "ListCapabilities",
            "GET",
            format!("/clusters/{name}/capabilities"),
            serde_json::Value::Null,
        ),
        row(
            "CreateCapability",
            "POST",
            format!("/clusters/{name}/capabilities"),
            serde_json::json!({
                "capabilityName": "probe-capability",
                "type": "KRO",
                "roleArn": ROLE_ARN,
                "deletePropagationPolicy": "RETAIN",
            }),
        ),
        row(
            "CreateNodegroup",
            "POST",
            format!("/clusters/{name}/node-groups"),
            serde_json::json!({
                "nodegroupName": "probe-nodegroup",
                "nodeRole": ROLE_ARN,
                "subnets": ["subnet-aaaa1111", "subnet-bbbb2222"],
            }),
        ),
        row(
            "ListPodIdentityAssociations",
            "GET",
            format!("/clusters/{name}/pod-identity-associations"),
            serde_json::Value::Null,
        ),
        row(
            "CreatePodIdentityAssociation",
            "POST",
            format!("/clusters/{name}/pod-identity-associations"),
            serde_json::json!({
                "namespace": "default",
                "serviceAccount": "probe",
                "roleArn": ROLE_ARN,
            }),
        ),
    ]
}

async fn send(router: &MultiServiceRouter, region: &str, request: &Row) -> Answer {
    call(router, region, request.method, &request.uri, &request.body).await
}

/// The 404 every row must answer with: the status and the modelled error code.
fn assert_not_found(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected HTTP 404, got {answer:?}"
    );
    assert!(
        answer
            .error_type
            .as_deref()
            .is_some_and(|actual| actual.ends_with("ResourceNotFoundException")),
        "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
    );
}

/// Create `name` and wait out the span the clock owes it, so it is `ACTIVE`: a real
/// cluster, so a not-found below is about the CHILD rather than about an empty store.
///
/// The body is the one whose `authenticationMode` serves the access-entry API, since
/// the pairs below include an access entry: a cluster that serves no access entries
/// refuses those calls for its mode instead of reporting the missing child.
async fn active_cluster(router: &MultiServiceRouter, name: &str) {
    let created = call(
        router,
        REGION,
        "POST",
        "/clusters",
        &access_entry_cluster_request(name).to_string(),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: creating cluster {name:?} must succeed, got {created:?}"
    );
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
}

// ---------------------------------------------------------------------------
// RULE (half one): an operation that resolves a resource inside a cluster reports a
// miss with the generic sentence and NO identity member at all — whichever half of
// the pair is absent.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_pair_resolving_not_found_carries_no_identity_member() {
    let router = emulator();
    // A real, ACTIVE cluster that holds none of the children the rows name, plus one
    // whose name is only valid in another region.
    active_cluster(&router, "nf-present").await;

    // (case, region, cluster) — rows differ along WHY the pair cannot be resolved and
    // along the form of the name the request carries (its two documented length
    // bounds among them: `clusterName` is `length 1..100`).
    let longest = "n".repeat(100);
    let cases: Vec<(&str, &str, &str)> = vec![
        ("a cluster that was never created", REGION, "nf-absent"),
        (
            "a cluster name differing only in case",
            REGION,
            "NF-PRESENT",
        ),
        (
            "a cluster name at the documented length ceiling",
            REGION,
            longest.as_str(),
        ),
        ("a one-character cluster name", REGION, "x"),
        (
            "a real cluster that holds no such child",
            REGION,
            "nf-present",
        ),
        (
            "the real cluster name, signed for another region",
            "eu-west-1",
            "nf-present",
        ),
    ];

    for (case, region, cluster) in cases {
        for request in pair_resolving(cluster) {
            let case = format!("{case} / {}", request.operation);
            let answer = send(&router, region, &request).await;
            assert_not_found(&case, &answer);
            assert_eq!(
                answer.body["message"],
                serde_json::json!(RESOURCE_NOT_FOUND_MESSAGE),
                "case {case:?}: expected the generic not-found sentence, got {:?}",
                answer.body["message"]
            );
            // The sentence is a CONSTANT, so it echoes nothing the request named:
            // the equality above is what pins that.
            for member in IDENTITY_MEMBERS {
                assert_eq!(
                    answer.body[member],
                    serde_json::Value::Null,
                    "case {case:?}: the payload must not carry {member}, got {:?}",
                    answer.body[member]
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RULE (half two): an operation that only validates the cluster reports the CLUSTER —
// `clusterName` and the message that names it — and never a sub-resource member.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_cluster_resolving_not_found_names_only_the_cluster() {
    let router = emulator();
    // One real cluster, so "not found" is about the name a row carries.
    active_cluster(&router, "nf-real").await;

    // (case, region, cluster) — the two documented length bounds of `clusterName`
    // (`length 1..100`) are rows of their own.
    let longest = "c".repeat(100);
    let cases: Vec<(&str, &str, &str)> = vec![
        ("a cluster that was never created", REGION, "nf-ghost"),
        ("a cluster name differing only in case", REGION, "NF-REAL"),
        (
            "a cluster name at the documented length ceiling",
            REGION,
            longest.as_str(),
        ),
        ("a one-character cluster name", REGION, "z"),
        (
            "the real cluster name, signed for another region",
            "eu-west-1",
            "nf-real",
        ),
    ];

    for (case, region, cluster) in cases {
        for request in cluster_resolving(cluster) {
            let case = format!("{case} / {}", request.operation);
            let answer = send(&router, region, &request).await;
            assert_not_found(&case, &answer);
            assert_eq!(
                answer.body["clusterName"].as_str(),
                Some(cluster),
                "case {case:?}: expected clusterName to name the missing cluster, got {:?}",
                answer.body["clusterName"]
            );
            assert_eq!(
                answer.body["message"].as_str(),
                Some(cluster_not_found_message(cluster).as_str()),
                "case {case:?}: expected the message that names the cluster, got {:?}",
                answer.body["message"]
            );
            for member in CHILD_IDENTITY_MEMBERS {
                assert_eq!(
                    answer.body[member],
                    serde_json::Value::Null,
                    "case {case:?}: the payload must not echo the request's {member}, got {:?}",
                    answer.body[member]
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: the payload shape is about ABSENCE, not about the shape of the request — the
// same operations answer 200 for a child that IS there.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_rule_is_about_absence_not_about_the_request() {
    let router = emulator();
    active_cluster(&router, "nf-roundtrip").await;

    // The child is missing first, is created, and is then found: the not-found
    // payload above is what the store says, not a fixed answer.
    let before = call(
        &router,
        REGION,
        "GET",
        "/clusters/nf-roundtrip/addons/vpc-cni",
        "",
    )
    .await;
    assert_not_found("DescribeAddon before the create", &before);

    let created = call(
        &router,
        REGION,
        "POST",
        "/clusters/nf-roundtrip/addons",
        &serde_json::json!({ "addonName": "vpc-cni" }).to_string(),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "creating the add-on on a real cluster must succeed, got {created:?}"
    );

    let after = call(
        &router,
        REGION,
        "GET",
        "/clusters/nf-roundtrip/addons/vpc-cni",
        "",
    )
    .await;
    assert_eq!(
        after.status, 200,
        "the stored add-on must read back, got {after:?}"
    );
    assert_eq!(
        after.body["addon"]["addonName"],
        serde_json::json!("vpc-cni"),
        "the add-on read back must be the one that was created, got {after:?}"
    );
    assert_eq!(
        after.body["addon"]["clusterName"],
        serde_json::json!("nf-roundtrip"),
        "the add-on read back must name its cluster, got {after:?}"
    );
}
