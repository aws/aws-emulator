// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the ERROR CLASS a HyperPod node reference is refused with when
//! it resolves to no node.
//!
//! RULE: a request that names a node of a cluster addresses a RESOURCE, so a reference that
//! resolves to no node — because the account holds no such cluster, because the cluster holds
//! no such node, or because the node has been deleted — is refused with the operation's
//! modelled not-found error and never with a request-validation failure: `ResourceNotFound`
//! (HTTP 400, "Resource being access is not found") is the ONLY error either node operation
//! declares (smithy_query query="operation"
//! operation="DescribeClusterNode,AttachClusterNodeVolume"). Which half of the reference was
//! unresolvable is told by the MESSAGE, not by the code: the node lookup's own fixed sentence
//! for the node half, and the cluster lookup's own for a cluster the attach could not resolve.
//!
//! Why it matters on the wire: a client that polls a node until it is gone treats the modelled
//! not-found as "deleted" and re-raises everything else, so a refusal of another class turns a
//! successful delete into an unhandled error.
//!
//! The rows span what the rule generalises over: the operation resolving the reference, which
//! half of the reference is unresolvable, the selector the node is named by (`NodeId`,
//! `NodeLogicalId`), the FORM the cluster is addressed in (name, ARN), the length of the
//! unknown instance id (both forms `ClusterNodeId` admits) and whether the node was never
//! there or was DELETED after having been read back.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`, `awsJson1_1`)
//! on the harness the `inference_deployment_lifecycle` suite stands up, and the cluster comes
//! from the node suites' own fixture, which polls for the CONDITION the clock owes rather than
//! waiting a span out.

use std::time::Instant;

use serde_json::{json, Value};

use crate::hyperpod_cluster_node_replacement::{
    in_service_cluster, nodes_once_all_running, Node, POLL_INTERVAL, SETTLE_DEADLINE,
};
use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The region every call below signs for.
const REGION: &str = "us-east-1";

/// The sentence the node lookup states for a reference that resolves to no node. Spelled out
/// here rather than read off the crate under test, so the rows assert the wording AWS uses.
const NODE_NOT_FOUND: &str = "Specified cluster node not found";

/// The sentence the volume-attachment path states for a cluster this account does not hold:
/// the fixed one an operation whose subject hangs off the cluster words that condition with.
const ATTACH_CLUSTER_NOT_FOUND: &str = "Specified cluster not found";

/// A well-formed `ClusterArn` (pattern
/// `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:cluster/[a-z0-9]{12}$`) this account holds
/// no cluster for.
fn absent_cluster_arn() -> String {
    format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:cluster/abcdef123456")
}

/// A well-formed `VolumeId` (pattern `^vol-[a-f0-9]{8}(?:[a-f0-9]{9})?$`).
const VOLUME: &str = "vol-0123456789abcdef0";

/// Stand up an `InService` cluster of `nodes` compute instances and answer its ARN beside its
/// nodes, each already `Running`.
async fn cluster_with_nodes(emulator: &Emulator, name: &str, nodes: i64) -> (String, Vec<Node>) {
    let (arn, _) = in_service_cluster(emulator, name, &[("workers", "ml.m5.large", nodes)]).await;
    let running = nodes_once_all_running(emulator, name).await;
    (arn, running)
}

/// THE RULE for a reference that never resolved: whichever operation resolves it and whichever
/// half of it is unknown, the refusal is the modelled not-found.
#[tokio::test]
async fn an_unresolvable_node_reference_is_refused_the_modelled_not_found() {
    let emulator = Emulator::new();
    let (cluster_arn, nodes) = cluster_with_nodes(&emulator, "hyperpod-refs", 1).await;
    let node = nodes
        .first()
        .expect("the fixture's cluster holds one node")
        .clone();
    let absent_arn = absent_cluster_arn();

    // (case, operation, request, the sentence the refusal has to carry)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "the reader, a short instance id of a cluster addressed by name",
            "DescribeClusterNode",
            json!({ "ClusterName": "hyperpod-refs", "NodeId": "i-00000000" }),
            NODE_NOT_FOUND,
        ),
        (
            "the reader, a long instance id of a cluster addressed by ARN",
            "DescribeClusterNode",
            json!({ "ClusterName": cluster_arn, "NodeId": "i-0000000000000000f" }),
            NODE_NOT_FOUND,
        ),
        (
            "the reader, a logical id no node carries",
            "DescribeClusterNode",
            json!({ "ClusterName": "hyperpod-refs", "NodeLogicalId": "no-such-node" }),
            NODE_NOT_FOUND,
        ),
        (
            "the reader, a cluster name this account does not hold",
            "DescribeClusterNode",
            json!({ "ClusterName": "no-such-cluster", "NodeId": &node.instance_id }),
            NODE_NOT_FOUND,
        ),
        (
            "the reader, a cluster ARN this account does not hold",
            "DescribeClusterNode",
            json!({ "ClusterName": absent_arn, "NodeId": &node.instance_id }),
            NODE_NOT_FOUND,
        ),
        (
            "the attach, a short instance id the cluster does not own",
            "AttachClusterNodeVolume",
            json!({
                "ClusterArn": cluster_arn,
                "NodeId": "i-00000000",
                "VolumeId": VOLUME,
            }),
            NODE_NOT_FOUND,
        ),
        (
            "the attach, a long instance id the cluster does not own",
            "AttachClusterNodeVolume",
            json!({
                "ClusterArn": cluster_arn,
                "NodeId": "i-0000000000000000f",
                "VolumeId": VOLUME,
            }),
            NODE_NOT_FOUND,
        ),
        (
            "the attach, a cluster ARN this account does not hold",
            "AttachClusterNodeVolume",
            json!({
                "ClusterArn": absent_arn,
                "NodeId": &node.instance_id,
                "VolumeId": VOLUME,
            }),
            ATTACH_CLUSTER_NOT_FOUND,
        ),
    ];

    for (case, operation, body, expected) in rows {
        let (status, refused) = emulator.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: expected 400 ResourceNotFound, got {status} with {refused}"
        );
        let reported = message(&refused);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // The reader still SERVES the node the cluster does own, so the refusals above are the
    // reference failing to resolve and not the operation refusing everything.
    let described = emulator
        .ok(
            "DescribeClusterNode",
            &json!({ "ClusterName": "hyperpod-refs", "NodeId": &node.instance_id }),
        )
        .await;
    assert_eq!(
        str_field(&described["NodeDetails"], "InstanceId"),
        node.instance_id,
        "case served read: the reader answers the node the cluster owns, got {described}"
    );
}

/// THE RULE for a node that WAS there: once `BatchDeleteClusterNodes` has taken it, every
/// selector that named it reads as gone — the modelled not-found carrying the node sentence —
/// while the nodes beside it are still read back.
#[tokio::test]
async fn a_deleted_node_reads_as_the_modelled_not_found() {
    let emulator = Emulator::new();
    let (_, nodes) = cluster_with_nodes(&emulator, "hyperpod-gone", 2).await;
    let doomed = nodes.first().expect("two nodes were created").clone();
    let kept = nodes.get(1).expect("two nodes were created").clone();

    // Before the delete, both selectors of the doomed node resolve: so what the rows below
    // report is the delete having taken it.
    for (selector, body) in [
        (
            "NodeLogicalId",
            json!({ "ClusterName": "hyperpod-gone", "NodeLogicalId": &doomed.logical_id }),
        ),
        (
            "NodeId",
            json!({ "ClusterName": "hyperpod-gone", "NodeId": &doomed.instance_id }),
        ),
    ] {
        let read = emulator.ok("DescribeClusterNode", &body).await;
        assert_eq!(
            str_field(&read["NodeDetails"], "NodeLogicalId"),
            doomed.logical_id,
            "case before the delete by {selector}: the node must be readable, got {read}"
        );
    }

    let deleted = emulator
        .ok(
            "BatchDeleteClusterNodes",
            &json!({
                "ClusterName": "hyperpod-gone",
                "NodeLogicalIds": [&doomed.logical_id],
            }),
        )
        .await;
    assert_eq!(
        deleted
            .get("SuccessfulNodeLogicalIds")
            .and_then(Value::as_array)
            .map(|ids| ids.len()),
        Some(1),
        "the delete must report the node it took, got {deleted}"
    );

    // A client waiting for the node to stop resolving polls the reader and reads the modelled
    // not-found as "gone": the CONDITION is polled for, never a span of time waited out.
    for (selector, body) in [
        (
            "NodeLogicalId",
            json!({ "ClusterName": "hyperpod-gone", "NodeLogicalId": &doomed.logical_id }),
        ),
        (
            "NodeId",
            json!({ "ClusterName": "hyperpod-gone", "NodeId": &doomed.instance_id }),
        ),
    ] {
        let deadline = Instant::now() + SETTLE_DEADLINE;
        loop {
            let (status, answered) = emulator.call("DescribeClusterNode", &body).await;
            if status != 200 {
                assert_eq!(
                    (status, error_code(&answered).as_str(), message(&answered).as_str()),
                    (400, "ResourceNotFound", NODE_NOT_FOUND),
                    "case the deleted node by {selector}: expected the modelled not-found, got \
                     {status} with {answered}"
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case the deleted node by {selector}: the reader must stop resolving it, last \
                 read {answered}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    // The sibling node is untouched, so the refusals above are about the node that was taken.
    let read = emulator
        .ok(
            "DescribeClusterNode",
            &json!({ "ClusterName": "hyperpod-gone", "NodeLogicalId": &kept.logical_id }),
        )
        .await;
    assert_eq!(
        str_field(&read["NodeDetails"], "NodeLogicalId"),
        kept.logical_id,
        "case the kept node: it must still be readable, got {read}"
    );
}
