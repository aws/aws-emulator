// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the refusal a create earns when the cluster already holds
//! the name the request asks for, driven over the wire through the same
//! `MultiServiceRouter` the production binary mounts.
//!
//! RULE: each create answers a name that is already taken with ITS OWN fixed
//! sentence, with the request's own names filled into it — `CreateNodegroup` with
//! `NodeGroup already exists with name <nodegroupName> and cluster name
//! <clusterName>` (409 `ResourceInUseException`), whose fixed halves carry no
//! punctuation after either label, while `CreateCluster` words its own conflict
//! differently (`Cluster already exists with name: <name>`). The sentence belongs to
//! the operation, so it is never derived from a sibling create's.
//!
//! The suite asserts that rule over a TABLE whose rows differ along what it
//! generalises over: the length and the form of the node group's name, the cluster
//! the pair names, and whether the name is taken in THIS cluster at all (the same
//! name in another cluster is a different node group, which is created rather than
//! refused).
//!
//! The wire harness (`Answer`, `emulator`, `call`, `encode_label`) is the shared one
//! in `tests/all/wire.rs`; the cluster and node group requests are the shared ones in
//! `tests/all/nodegroup_lifecycle.rs`.

use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::REGION;
use crate::nodegroup_lifecycle::{create_cluster, create_nodegroup_in, minimal_request};
use crate::wire::{call, emulator, error_code, Answer};

async fn create_nodegroup(router: &MultiServiceRouter, cluster: &str, name: &str) -> Answer {
    create_nodegroup_in(router, REGION, cluster, &minimal_request(name)).await
}

/// The sentence Amazon EKS refuses a taken node group name with: the two fixed
/// halves, with the request's `nodegroupName` and `clusterName` filled in.
fn expected_sentence(nodegroup: &str, cluster: &str) -> String {
    format!("NodeGroup already exists with name {nodegroup} and cluster name {cluster}")
}

// ---------------------------------------------------------------------------
// RULE: a `CreateNodegroup` whose `(cluster, nodegroupName)` pair is already taken
// is refused 409 `ResourceInUseException` with that operation's own sentence, built
// from the request's two names.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_taken_nodegroup_name_is_refused_with_this_operations_sentence() {
    let router = emulator();

    // Two clusters, so a row can show the sentence naming the cluster the request
    // addressed — and that the same node group name is free in the other one.
    let first_cluster = "dup-cluster";
    let second_cluster = "dup-cluster-two";
    create_cluster(&router, first_cluster).await;
    create_cluster(&router, second_cluster).await;

    // The rows differ along the form and the length of the name, and along the
    // cluster the pair names. `CreateNodegroupRequest$nodegroupName` carries no
    // pattern, so a plain, a hyphenated, an underscored and a long name are all
    // names a caller may ask for.
    let rows: [(&str, &str, &str); 5] = [
        ("the shortest name", first_cluster, "n"),
        ("a hyphenated name", first_cluster, "ng-blue-2026"),
        ("an underscored name", first_cluster, "ng_with_underscores"),
        (
            "a long name",
            first_cluster,
            "ng-0123456789-0123456789-0123456789-0123456789-0123456789-abc",
        ),
        (
            "the same name in the other cluster",
            second_cluster,
            "ng-blue-2026",
        ),
    ];

    for (case, cluster, nodegroup) in rows {
        let created = create_nodegroup(&router, cluster, nodegroup).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the first create of {nodegroup:?} in {cluster:?} must be \
             accepted, got {created:?}"
        );

        let refused = create_nodegroup(&router, cluster, nodegroup).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected HTTP 409 for the taken name, got {refused:?}"
        );
        let code = error_code(&refused);
        assert_eq!(
            code, "ResourceInUseException",
            "case {case:?}: expected ResourceInUseException, got {code:?}"
        );
        let expected = expected_sentence(nodegroup, cluster);
        assert_eq!(
            refused.message(),
            expected,
            "case {case:?}: expected message {expected:?}, got {:?}",
            refused.message()
        );
        // The refusal names the resource it is about through the error's own
        // members, which carry the request's values.
        assert_eq!(
            refused.body["nodegroupName"],
            serde_json::json!(nodegroup),
            "case {case:?}: expected nodegroupName {nodegroup:?}, got {refused:?}"
        );
        assert_eq!(
            refused.body["clusterName"],
            serde_json::json!(cluster),
            "case {case:?}: expected clusterName {cluster:?}, got {refused:?}"
        );
    }
}

/// The node group a replayed `clientRequestToken` identifies is answered with that
/// node group rather than with the conflict — so the sentence above is what a SECOND
/// caller's create earns, not what a retry does.
#[tokio::test]
async fn a_replayed_idempotency_token_is_answered_with_the_node_group() {
    let router = emulator();
    create_cluster(&router, "dup-token-cluster").await;

    let body = {
        let mut request = minimal_request("ng-token");
        request["clientRequestToken"] = serde_json::json!("token-ng");
        request
    };
    let created = create_nodegroup_in(&router, REGION, "dup-token-cluster", &body).await;
    assert_eq!(
        created.status, 200,
        "case the first create: expected 200, got {created:?}"
    );

    let replayed = create_nodegroup_in(&router, REGION, "dup-token-cluster", &body).await;
    assert_eq!(
        replayed.status, 200,
        "case the retry with the same token: expected 200, got {replayed:?}"
    );
    assert_eq!(
        replayed.body["nodegroup"]["nodegroupArn"], created.body["nodegroup"]["nodegroupArn"],
        "case the retry with the same token: expected the node group the create made, got \
         {replayed:?}"
    );

    // Another caller's create of the same name, carrying its own token, is the
    // conflict — with the sentence this operation words it in.
    let other = {
        let mut request = minimal_request("ng-token");
        request["clientRequestToken"] = serde_json::json!("token-other");
        request
    };
    let refused = create_nodegroup_in(&router, REGION, "dup-token-cluster", &other).await;
    assert_eq!(
        refused.status, 409,
        "case another caller's create: expected HTTP 409, got {refused:?}"
    );
    let expected = expected_sentence("ng-token", "dup-token-cluster");
    assert_eq!(
        refused.message(),
        expected,
        "case another caller's create: expected message {expected:?}, got {:?}",
        refused.message()
    );
}

/// The sentence belongs to the OPERATION: a cluster whose name is taken is refused
/// with the cluster create's own wording, which is not the node group's.
#[tokio::test]
async fn each_create_keeps_its_own_conflict_sentence() {
    let router = emulator();
    let name = "dup-wording-cluster";
    create_cluster(&router, name).await;

    let refused = call(
        &router,
        REGION,
        "POST",
        "/clusters",
        &serde_json::json!({
            "name": name,
            "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
            "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        refused.status, 409,
        "case the cluster create: expected HTTP 409, got {refused:?}"
    );
    let expected = format!("Cluster already exists with name: {name}");
    assert_eq!(
        refused.message(),
        expected,
        "case the cluster create: expected message {expected:?}, got {:?}",
        refused.message()
    );

    // And the node group's sentence does not borrow that punctuation.
    create_nodegroup(&router, name, "ng-wording").await;
    let refused = create_nodegroup(&router, name, "ng-wording").await;
    let expected = expected_sentence("ng-wording", name);
    assert_eq!(
        refused.message(),
        expected,
        "case the node group create: expected message {expected:?}, got {:?}",
        refused.message()
    );
}
