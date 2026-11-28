// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the PRECONDITION a HyperPod node's volume attachment is held
//! to: `AttachClusterNodeVolume$ClusterArn` documents "Your cluster must use EKS as the
//! orchestration and be in the `InService` state" (smithy_query query="doc"
//! shape=AttachClusterNodeVolumeRequest member=ClusterArn), so a cluster orchestrated by
//! anything else cannot take a volume change at all.
//!
//! The suite asserts a RULE over a TABLE of clusters rather than one recorded call: the node
//! REFERENCE is resolved first — a `NodeId` that names no node of the cluster is answered the
//! modelled `ResourceNotFound` whatever the cluster is orchestrated by — and only a request
//! whose every reference resolves is then held to what the cluster can do: served for an
//! EKS-orchestrated cluster, refused with a 400 `ValidationException` naming the cluster for
//! one that is not, with nothing attached. The rows differ along the orchestration (EKS,
//! Slurm, none at all) and along whether the node named exists.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses.

use std::time::Instant;

use serde_json::{json, Value};

use crate::hyperpod_cluster_clock::store;
use crate::hyperpod_cluster_node_replacement::{POLL_INTERVAL, SETTLE_DEADLINE};
use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The Region every case signs for.
const REGION: &str = "us-east-1";

/// A well-formed `VolumeId` (pattern `^vol-[a-f0-9]{8}(?:[a-f0-9]{9})?$`).
const VOLUME: &str = "vol-0123456789abcdef0";

/// A well-formed `ClusterNodeId` (pattern `^i-[a-f0-9]{8}(?:[a-f0-9]{9})?$`) no cluster of
/// this account holds a node for.
const ABSENT_NODE: &str = "i-0000000000000000f";

/// The sentence the node lookup states for a reference that resolves to no node. Spelled out
/// here rather than read off the crate under test.
const NODE_NOT_FOUND: &str = "Specified cluster node not found";

/// Create a cluster with one instance group of one instance and the orchestration `row`
/// names, and answer its ARN beside the instance id of the node its group came up with.
///
/// The instance comes up WITH the cluster, so the node exists only once the
/// `Creating` -> `InService` transition the clock owes has fallen due: that CONDITION is
/// polled for rather than waited out, so no case depends on how fast the host runs.
async fn cluster_with_a_node(
    emulator: &Emulator,
    name: &str,
    orchestrator: Option<Value>,
) -> (String, String) {
    let mut request = json!({
        "ClusterName": name,
        "InstanceGroups": [{
            "InstanceGroupName": "workers",
            "InstanceCount": 1,
            "InstanceType": "ml.m5.large",
            "ExecutionRole": format!("arn:aws:iam::{ACCOUNT}:role/hyperpod-workers"),
        }],
    });
    if let Some(orchestrator) = orchestrator {
        request["Orchestrator"] = orchestrator;
    }
    let created = emulator.ok("CreateCluster", &request).await;
    let arn = str_field(&created, "ClusterArn");
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let listed = emulator
            .ok(
                "ListClusterNodes",
                &json!({ "ClusterName": name, "IncludeNodeLogicalIds": true }),
            )
            .await;
        let instance_id = listed
            .get("ClusterNodeSummaries")
            .and_then(Value::as_array)
            .and_then(|summaries| summaries.first())
            .map(|summary| str_field(summary, "InstanceId"))
            .unwrap_or_default();
        if !instance_id.is_empty() {
            return (arn, instance_id);
        }
        assert!(
            Instant::now() < deadline,
            "case {name}: the cluster must hold the instance its group asked for, got {listed}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// How the attach path states that a cluster's orchestration cannot take a volume change. A
/// literal on purpose: this suite is what pins the wording, which is entirely fixed text —
/// the refusal is about the CLASS of cluster, so nothing from the request appears in it.
const VOLUME_OPERATIONS_NOT_SUPPORTED: &str =
    "Volume operations are not supported for Slurm clusters";

/// RULE: a volume attaches only to a node of an EKS-orchestrated cluster; a cluster
/// orchestrated otherwise is refused a 400 `ValidationException` naming it, and nothing is
/// attached — while a node reference that resolves to nothing is answered the modelled
/// `ResourceNotFound` first, whatever the orchestration.
#[tokio::test]
async fn a_volume_attaches_only_inside_an_eks_orchestrated_cluster() {
    // Rows differ along the orchestration: the one the attach needs, the other the model
    // declares (smithy_query shape=ClusterOrchestrator), and none at all.
    let rows: Vec<(&str, Option<Value>, bool)> = vec![
        (
            "an EKS orchestrator",
            Some(json!({
                "Eks": { "ClusterArn": format!("arn:aws:eks:{REGION}:{ACCOUNT}:cluster/hyperpod") },
            })),
            true,
        ),
        (
            "a Slurm orchestrator",
            Some(json!({ "Slurm": { "SlurmConfigStrategy": "Managed" } })),
            false,
        ),
        ("no orchestrator at all", None, false),
    ];

    for (index, (case, orchestrator, attachable)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("hyperpod-{index}");
        let (cluster_arn, node) = cluster_with_a_node(&emulator, &name, orchestrator).await;

        // The node reference is resolved before the cluster's orchestration is weighed, so a
        // node this cluster holds none of is the modelled not-found in every row.
        let (status, refused) = emulator
            .call(
                "AttachClusterNodeVolume",
                &json!({
                    "ClusterArn": cluster_arn,
                    "NodeId": ABSENT_NODE,
                    "VolumeId": VOLUME,
                }),
            )
            .await;
        assert!(
            status == 400
                && error_code(&refused) == "ResourceNotFound"
                && message(&refused) == NODE_NOT_FOUND,
            "case {case:?}: a node the cluster does not hold must be refused 400 \
             ResourceNotFound with {NODE_NOT_FOUND:?}, got {status} with {refused}"
        );

        let (status, answered) = emulator
            .call(
                "AttachClusterNodeVolume",
                &json!({ "ClusterArn": cluster_arn, "NodeId": node, "VolumeId": VOLUME }),
            )
            .await;
        if attachable {
            assert!(
                status == 200 && str_field(&answered, "VolumeId") == VOLUME,
                "case {case:?}: the attach must be served, got {status} with {answered}"
            );
            continue;
        }
        assert!(
            status == 400
                && error_code(&answered) == "ValidationException"
                && message(&answered) == VOLUME_OPERATIONS_NOT_SUPPORTED,
            "case {case:?}: expected 400 ValidationException with \
             {VOLUME_OPERATIONS_NOT_SUPPORTED:?}, got {status} with {answered}"
        );
        // Nothing was attached: the node the refused call named holds no volume, read out of
        // the one store every handler reaches, so a later caller sees a node the refusal left
        // exactly as it was.
        let held = store(&emulator.engine);
        let guard = held.lock();
        let attachments: usize = guard
            .clusters
            .get(&name)
            .unwrap_or_else(|| panic!("case {case:?}: expected a stored cluster {name:?}"))
            .nodes
            .values()
            .map(|node| node.volume_attachments.len())
            .sum();
        assert!(
            attachments == 0,
            "case {case:?}: a refused attach must leave the node's volumes alone, got \
             {attachments} attachment(s)"
        );
    }
}
