// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker HyperPod's `BatchReplaceClusterNodes`: the nodes a
//! request names are replaced in place, with `DescribeClusterNode` and `ListClusterNodes`
//! as the readers each replacement is checked against.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so what
//! these tests exercise is what a real SDK client gets. The harness is the one
//! `inference_deployment_lifecycle` declares.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call:
//!
//! * every identifier a call accepts is answered in the position the request listed it,
//!   whichever order that is and whichever identifier kind named it;
//! * a replaced node keeps its `NodeLogicalId` and its configuration while its instance is
//!   terminated, and comes back on a freshly minted instance id;
//! * replacing a node destroys the volumes it held;
//! * an identifier the cluster does not hold, and one whose node is mid-change, is
//!   reported in the matching failure list with the modelled per-node error code, leaving
//!   the identifiers beside it in the same batch replaced;
//! * a request naming no node at all, or more nodes than one call may replace, is refused,
//!   and so is one naming a cluster this account does not hold.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::hyperpod_cluster_clock::{owe_nodes_running_at, FAR_FROM_DUE};
use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// How long a poll loop keeps looking for a transition the clock owes before giving up:
/// far more room than the span itself, so a slow or loaded host cannot decide a case.
/// Nothing is asserted about how long a transition actually took.
pub(crate) const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How often a poll loop looks again while it waits for the condition it is waiting on.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The per-node failure messages, as `BatchReplaceClusterNodesErrorCode` accounts for each
/// code (smithy_query shape=BatchReplaceClusterNodesError,
/// shape=BatchReplaceClusterNodeLogicalIdsError): the instance-id list says "instance" and
/// the logical-id list says "node", the terms their own error shapes are documented in.
const INSTANCE_NOT_FOUND: &str = "The instance does not exist in the specified cluster.";
/// See [`INSTANCE_NOT_FOUND`].
pub(crate) const NODE_NOT_FOUND: &str = "The node does not exist in the specified cluster.";
/// See [`INSTANCE_NOT_FOUND`].
const NODE_INVALID_STATUS: &str = "The node is in a state that does not allow replacement. \
     Wait for the node to finish any ongoing changes before retrying.";
/// See [`INSTANCE_NOT_FOUND`].
pub(crate) const IN_USE: &str = "Another operation is already in progress for this node. Wait \
     for the operation to complete before retrying.";

/// How SageMaker refuses a request none of whose identifiers names a node of the cluster:
/// fixed text with no variable part.
const NONE_EXIST: &str = "All provided nodes do not exist. Please check the input and try \
     again.";

/// An instance group of a HyperPod cluster. `InstanceCount`, `InstanceGroupName` and
/// `ExecutionRole` are the group's required members (smithy_query
/// shape=ClusterInstanceGroupSpecification); the types come from `ClusterInstanceType`
/// (smithy_query shape=ClusterInstanceType).
pub(crate) fn instance_group(name: &str, instance_type: &str, instance_count: i64) -> Value {
    json!({
        "InstanceGroupName": name,
        "InstanceCount": instance_count,
        "InstanceType": instance_type,
        "ExecutionRole": format!("arn:aws:iam::{ACCOUNT}:role/hyperpod-{name}"),
    })
}

/// One node of a cluster, as the listing reports it: the two identifiers a replacement can
/// name it by, plus the configuration a replacement must keep.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Node {
    pub(crate) logical_id: String,
    pub(crate) instance_id: String,
    pub(crate) instance_group: String,
    pub(crate) instance_type: String,
    pub(crate) status: String,
    pub(crate) launch_time: f64,
}

/// Every node of `cluster`, in the order `ListClusterNodes` reports them, read through the
/// listing that carries `NodeLogicalId` so both identifiers of each node are known.
pub(crate) async fn nodes_of(emulator: &Emulator, cluster: &str) -> Vec<Node> {
    let listed = emulator
        .ok(
            "ListClusterNodes",
            &json!({ "ClusterName": cluster, "IncludeNodeLogicalIds": true }),
        )
        .await;
    listed
        .get("ClusterNodeSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected ClusterNodeSummaries in {listed}"))
        .iter()
        .map(|summary| Node {
            logical_id: str_field(summary, "NodeLogicalId"),
            instance_id: summary
                .get("InstanceId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            instance_group: str_field(summary, "InstanceGroupName"),
            instance_type: str_field(summary, "InstanceType"),
            status: str_field(&summary["InstanceStatus"], "Status"),
            launch_time: summary
                .get("LaunchTime")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("expected LaunchTime in {summary}")),
        })
        .collect()
}

/// The node `logical_id` names, once it reports `Running`.
///
/// Polled for the CONDITION — the status the clock owes the node — not for a span of time,
/// so no case depends on how fast the host runs. Shared with the sibling node suites, so
/// there is one way to wait for a node to come back.
pub(crate) async fn node_once_running(emulator: &Emulator, cluster: &str, logical_id: &str) -> Node {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let found = nodes_of(emulator, cluster)
            .await
            .into_iter()
            .find(|node| node.logical_id == logical_id);
        match found {
            Some(node) if node.status == "Running" => return node,
            other => assert!(
                Instant::now() < deadline,
                "case {logical_id:?}: the node must report Running, got {other:?}"
            ),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every node of `cluster`, once they ALL report `Running`, i.e. no change the clock owes
/// one is still in flight.
///
/// The sibling of [`node_once_running`] for a case that waits on the whole cluster, kept
/// beside it so there is one place that waits for a node condition.
pub(crate) async fn nodes_once_all_running(emulator: &Emulator, cluster: &str) -> Vec<Node> {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let held = nodes_of(emulator, cluster).await;
        if held.iter().all(|node| node.status == "Running") {
            return held;
        }
        assert!(
            Instant::now() < deadline,
            "cluster {cluster:?}: every node must reach Running, got {held:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Create `cluster` holding one instance group per `(name, type, count)` row, wait for the
/// instances its groups asked for, and answer its `ClusterArn` beside its nodes.
///
/// The instances come up WITH the cluster, so the nodes exist only once the
/// `Creating` -> `InService` transition the clock owes has fallen due: that CONDITION is
/// polled for rather than waited out, so no case depends on how fast the host runs.
pub(crate) async fn in_service_cluster(
    emulator: &Emulator,
    cluster: &str,
    groups: &[(&str, &str, i64)],
) -> (String, Vec<Node>) {
    let instance_groups: Vec<Value> = groups
        .iter()
        .map(|(group, instance_type, count)| instance_group(group, instance_type, *count))
        .collect();
    let created = emulator
        .ok(
            "CreateCluster",
            // Orchestrated by Amazon EKS: the node-storage operations the suites built on
            // this fixture exercise document "Your cluster must use EKS as the orchestration
            // and be in the InService state" on their `ClusterArn` (smithy_query query="doc"
            // shape=AttachClusterNodeVolumeRequest member=ClusterArn).
            &json!({
                "ClusterName": cluster,
                "InstanceGroups": instance_groups,
                "Orchestrator": {
                    "Eks": {
                        "ClusterArn": format!("arn:aws:eks:us-east-1:{ACCOUNT}:cluster/hyperpod"),
                    },
                },
            }),
        )
        .await;
    let arn = str_field(&created, "ClusterArn");
    let expected: i64 = groups.iter().map(|(_, _, count)| *count).sum();
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let nodes = nodes_of(emulator, cluster).await;
        if i64::try_from(nodes.len()).expect("a node count that fits") == expected {
            return (arn, nodes);
        }
        assert!(
            Instant::now() < deadline,
            "case {cluster}: the cluster must hold the {expected} instances its groups asked \
             for, got {nodes:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `NodeDetails` `DescribeClusterNode` replays for the node `logical_id` names.
pub(crate) async fn describe(emulator: &Emulator, cluster: &str, logical_id: &str) -> Value {
    let described = emulator
        .ok(
            "DescribeClusterNode",
            &json!({ "ClusterName": cluster, "NodeLogicalId": logical_id }),
        )
        .await;
    described
        .get("NodeDetails")
        .cloned()
        .unwrap_or_else(|| panic!("expected NodeDetails in {described}"))
}

/// A string-list member of a response, or `None` when the response does not carry it.
pub(crate) fn string_list(body: &Value, member: &str) -> Option<Vec<String>> {
    let entries = body.get(member)?.as_array()?;
    Some(
        entries
            .iter()
            .map(|entry| {
                entry
                    .as_str()
                    .unwrap_or_else(|| panic!("expected a string entry in {body}"))
                    .to_string()
            })
            .collect(),
    )
}

/// An `InstanceId` in the modelled form: `ClusterNodeId` is pattern
/// `^i-[a-f0-9]{8}(?:[a-f0-9]{9})?$` (smithy_query operation=DescribeClusterNode).
fn assert_instance_id(case: &str, instance_id: &str) {
    let digits = instance_id.strip_prefix("i-").unwrap_or_else(|| {
        panic!("case {case:?}: expected an InstanceId starting \"i-\", got {instance_id:?}")
    });
    assert!(
        (digits.len() == 8 || digits.len() == 17)
            && digits.chars().all(|glyph| glyph.is_ascii_hexdigit()),
        "case {case:?}: an InstanceId's tail must match [a-f0-9]{{8}} or [a-f0-9]{{17}}, got \
         {digits:?}"
    );
}

/// RULE: a batch operation answers the identifiers it accepted in the POSITION the request
/// listed them — `Successful` carries the `NodeIds` it initiated a replacement for and
/// `SuccessfulNodeLogicalIds` the `NodeLogicalIds`, in request order — so a caller can
/// line its request up against the answer without re-sorting either.
#[tokio::test]
async fn every_accepted_identifier_is_answered_in_the_position_the_request_listed_it() {
    // Rows differ along what the rule generalises over: how many nodes one call names, and
    // in which order relative to the order the cluster reports them.
    let rows: Vec<(&str, Vec<usize>)> = vec![
        ("one", vec![0]),
        ("reported-order", vec![0, 1, 2, 3]),
        ("reversed", vec![3, 2, 1, 0]),
        ("out-of-order-subset", vec![2, 0, 3]),
    ];
    for (case, picks) in rows {
        // By `NodeIds`, and then — on a cluster of its own, so no replacement is already
        // in flight — by `NodeLogicalIds`: the ordering rule is the same for both.
        let emulator = Emulator::new();
        let cluster = format!("replace-by-instance-id-{case}");
        let (_, nodes) = in_service_cluster(&emulator, &cluster, &[("pool", "ml.c5.large", 4)]).await;
        let node_ids: Vec<String> = picks
            .iter()
            .map(|at| nodes[*at].instance_id.clone())
            .collect();
        let replaced = emulator
            .ok(
                "BatchReplaceClusterNodes",
                &json!({ "ClusterName": cluster, "NodeIds": node_ids }),
            )
            .await;
        assert_eq!(
            string_list(&replaced, "Successful"),
            Some(node_ids.clone()),
            "case {case:?} by instance id: Successful must carry the requested ids in order, \
             got {replaced}"
        );
        assert_eq!(
            replaced.get("Failed"),
            Some(&json!([])),
            "case {case:?} by instance id: no requested id failed, got {replaced}"
        );
        assert_eq!(
            replaced.get("SuccessfulNodeLogicalIds"),
            None,
            "case {case:?} by instance id: the logical-id lists belong to a request that \
             named logical ids, got {replaced}"
        );
        assert_eq!(
            replaced.get("FailedNodeLogicalIds"),
            None,
            "case {case:?} by instance id: the logical-id lists belong to a request that \
             named logical ids, got {replaced}"
        );

        let emulator = Emulator::new();
        let cluster = format!("replace-by-logical-id-{case}");
        let (_, nodes) = in_service_cluster(&emulator, &cluster, &[("pool", "ml.c5.large", 4)]).await;
        let logical_ids: Vec<String> = picks
            .iter()
            .map(|at| nodes[*at].logical_id.clone())
            .collect();
        let replaced = emulator
            .ok(
                "BatchReplaceClusterNodes",
                &json!({ "ClusterName": cluster, "NodeLogicalIds": logical_ids }),
            )
            .await;
        assert_eq!(
            string_list(&replaced, "SuccessfulNodeLogicalIds"),
            Some(logical_ids.clone()),
            "case {case:?} by logical id: SuccessfulNodeLogicalIds must carry the requested \
             ids in order, got {replaced}"
        );
        assert_eq!(
            replaced.get("FailedNodeLogicalIds"),
            Some(&json!([])),
            "case {case:?} by logical id: no requested id failed, got {replaced}"
        );
        assert_eq!(
            replaced.get("Successful"),
            None,
            "case {case:?} by logical id: the instance-id lists belong to a request that \
             named instance ids, got {replaced}"
        );
        assert_eq!(
            replaced.get("Failed"),
            None,
            "case {case:?} by logical id: the instance-id lists belong to a request that \
             named instance ids, got {replaced}"
        );
    }
}

/// RULE: a request may name nodes by either identifier kind or by both, and each answer
/// list reports only the identifiers of its own kind.
#[tokio::test]
async fn a_request_naming_both_identifier_kinds_answers_each_list_for_its_own_kind() {
    let emulator = Emulator::new();
    let cluster = "replace-both-kinds";
    let (_, nodes) = in_service_cluster(
        &emulator,
        cluster,
        &[("controller", "ml.c5.large", 1), ("workers", "ml.g5.xlarge", 3)],
    )
    .await;
    let node_ids = vec![nodes[3].instance_id.clone(), nodes[1].instance_id.clone()];
    let logical_ids = vec![nodes[0].logical_id.clone(), nodes[2].logical_id.clone()];
    let replaced = emulator
        .ok(
            "BatchReplaceClusterNodes",
            &json!({
                "ClusterName": cluster,
                "NodeIds": node_ids,
                "NodeLogicalIds": logical_ids,
            }),
        )
        .await;
    assert_eq!(
        string_list(&replaced, "Successful"),
        Some(node_ids),
        "case both-kinds: Successful carries the instance ids in request order, got {replaced}"
    );
    assert_eq!(
        string_list(&replaced, "SuccessfulNodeLogicalIds"),
        Some(logical_ids),
        "case both-kinds: SuccessfulNodeLogicalIds carries the logical ids in request order, \
         got {replaced}"
    );
    for node in nodes_of(&emulator, cluster).await {
        assert_eq!(
            node.status, "ShuttingDown",
            "case both-kinds: every named node has its instance terminated, got {node:?}"
        );
    }
}

/// RULE: replacing a node gives it fresh hardware and nothing else — the `NodeLogicalId`
/// "persists throughout the node's lifecycle", the instance configuration is unchanged
/// ("the AMI and instance configuration remain the same"), and the instance the node runs
/// on is terminated and a new one provisioned, so the node reports a newly minted
/// `InstanceId` once the clock owes it.
#[tokio::test]
async fn a_replaced_node_keeps_its_identity_and_comes_back_on_fresh_hardware() {
    // Rows differ along the identifier kind the request names the node by and the group it
    // belongs to; the rule is the same whichever named it.
    for (case, by_logical_id, target) in [
        ("by-instance-id", false, 0_usize),
        ("by-logical-id", true, 2_usize),
    ] {
        let emulator = Emulator::new();
        let cluster = format!("replace-roundtrip-{case}");
        let (_, before) = in_service_cluster(
            &emulator,
            &cluster,
            &[("controller", "ml.c5.large", 1), ("workers", "ml.g5.xlarge", 2)],
        )
        .await;
        let node = before[target].clone();
        let request = if by_logical_id {
            json!({ "ClusterName": cluster, "NodeLogicalIds": [node.logical_id.clone()] })
        } else {
            json!({ "ClusterName": cluster, "NodeIds": [node.instance_id.clone()] })
        };
        emulator.ok("BatchReplaceClusterNodes", &request).await;

        // Straight after the call the replacement is still in flight: the node reports the
        // instance that is shutting down, under the identity it has always had.
        let details = describe(&emulator, &cluster, &node.logical_id).await;
        assert_eq!(
            str_field(&details["InstanceStatus"], "Status"),
            "ShuttingDown",
            "case {case:?}: a node whose replacement is in flight is shutting its instance \
             down, got {details}"
        );
        assert_eq!(
            str_field(&details, "InstanceId"),
            node.instance_id,
            "case {case:?}: the node runs on its old instance until the new one is up, got \
             {details}"
        );

        // Look again until the replacement hardware the clock owes the node is up: the
        // CONDITION, not a span of time.
        node_once_running(&emulator, &cluster, &node.logical_id).await;
        let details = describe(&emulator, &cluster, &node.logical_id).await;
        assert_eq!(
            str_field(&details, "NodeLogicalId"),
            node.logical_id,
            "case {case:?}: the NodeLogicalId persists across a replacement, got {details}"
        );
        assert_eq!(
            str_field(&details["InstanceStatus"], "Status"),
            "Running",
            "case {case:?}: the replacement hardware is up once the clock owes it, got \
             {details}"
        );
        let fresh = str_field(&details, "InstanceId");
        assert_ne!(
            fresh, node.instance_id,
            "case {case:?}: fresh hardware is a different instance, got {details}"
        );
        assert_instance_id(case, &fresh);
        assert_eq!(
            str_field(&details, "InstanceGroupName"),
            node.instance_group,
            "case {case:?}: the node stays in its instance group, got {details}"
        );
        assert_eq!(
            str_field(&details, "InstanceType"),
            node.instance_type,
            "case {case:?}: the instance configuration remains the same, got {details}"
        );
        let launched = details
            .get("LaunchTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected LaunchTime in {details}"));
        assert!(
            launched > node.launch_time,
            "case {case:?}: the replacement instance launched after the one it replaced, got \
             {launched} against {}",
            node.launch_time
        );

        // The cluster holds the same nodes it did, and every node the call did not name is
        // untouched: a replacement adds and removes nothing.
        let after = nodes_of(&emulator, &cluster).await;
        let identities: BTreeSet<String> = after.iter().map(|held| held.logical_id.clone()).collect();
        assert_eq!(
            identities,
            before
                .iter()
                .map(|held| held.logical_id.clone())
                .collect::<BTreeSet<String>>(),
            "case {case:?}: a replacement keeps the cluster's node identities, got {after:?}"
        );
        for held in &after {
            if held.logical_id == node.logical_id {
                continue;
            }
            let was = before
                .iter()
                .find(|earlier| earlier.logical_id == held.logical_id)
                .unwrap_or_else(|| panic!("case {case:?}: {held:?} was not listed before"));
            assert_eq!(
                held, was,
                "case {case:?}: a node the call did not name is unchanged, got {held:?}"
            );
        }
    }
}

/// RULE: replacing a node destroys every volume it held ("replacing nodes destroys all
/// instance volumes, including both root and secondary volumes"), so the node holds no
/// attachment from before the replacement and attaching the same volume again is a new
/// attachment rather than the earlier one replayed.
#[tokio::test]
async fn replacing_a_node_destroys_the_volumes_it_held() {
    let emulator = Emulator::new();
    let cluster = "replace-destroys-volumes";
    let (arn, nodes) = in_service_cluster(&emulator, cluster, &[("pool", "ml.m5.large", 1)]).await;
    let node = nodes[0].clone();
    // `VolumeId` is pattern `^vol-[a-f0-9]{8}(?:[a-f0-9]{9})?$` (smithy_query
    // operation=AttachClusterNodeVolume).
    let volume = "vol-0a1b2c3d4e5f60718";
    let attached = emulator
        .ok(
            "AttachClusterNodeVolume",
            &json!({ "ClusterArn": arn, "NodeId": node.instance_id, "VolumeId": volume }),
        )
        .await;
    let first_attach = attached
        .get("AttachTime")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected AttachTime in {attached}"));
    let first_device = str_field(&attached, "DeviceName");

    emulator
        .ok(
            "BatchReplaceClusterNodes",
            &json!({ "ClusterName": cluster, "NodeIds": [node.instance_id.clone()] }),
        )
        .await;
    node_once_running(&emulator, cluster, &node.logical_id).await;
    let replaced = nodes_of(&emulator, cluster).await;
    let fresh = replaced
        .iter()
        .find(|held| held.logical_id == node.logical_id)
        .unwrap_or_else(|| panic!("case destroyed-volumes: expected the node in {replaced:?}"));

    let reattached = emulator
        .ok(
            "AttachClusterNodeVolume",
            &json!({ "ClusterArn": arn, "NodeId": fresh.instance_id, "VolumeId": volume }),
        )
        .await;
    assert_eq!(
        str_field(&reattached, "Status"),
        "attaching",
        "case destroyed-volumes: the volume attaches to the replacement from scratch, got \
         {reattached}"
    );
    assert_eq!(
        str_field(&reattached, "DeviceName"),
        first_device,
        "case destroyed-volumes: the replacement has every device name free again, got \
         {reattached}"
    );
    let second_attach = reattached
        .get("AttachTime")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected AttachTime in {reattached}"));
    assert!(
        second_attach > first_attach,
        "case destroyed-volumes: the attachment is a new one, not the destroyed one replayed, \
         got {second_attach} against {first_attach}"
    );
}

/// One row of the unknown-identifier table: the case, the request, the instance ids and
/// the logical ids the call must replace, and the two failure lists it must report —
/// `Value::Null` where the response carries no list of that kind at all.
type FailureRow = (&'static str, Value, Vec<String>, Value, Vec<String>, Value);

/// RULE: a per-node failure is reported for the identifier it belongs to, with the
/// modelled error code for the condition — `InstanceIdNotFound` for an identifier the
/// cluster holds no node for — and leaves every other identifier of the same batch
/// replaced: a batch operation reports per-entry outcomes instead of failing whole.
#[tokio::test]
async fn an_identifier_the_cluster_does_not_hold_is_reported_in_the_failure_list() {
    let emulator = Emulator::new();
    let cluster = "replace-unknown-identifiers";
    let (_, nodes) = in_service_cluster(&emulator, cluster, &[("pool", "ml.c5.large", 3)]).await;
    // Rows differ along the identifier kind, the shape of the unknown identifier, and
    // whether a known identifier sits beside it in the same batch.
    let rows: Vec<FailureRow> = vec![
        (
            "known-beside-unknown",
            json!({
                "ClusterName": cluster,
                "NodeIds": [nodes[0].instance_id.clone(), "i-0123456789abcdef0"],
            }),
            vec![nodes[0].instance_id.clone()],
            json!([{
                "NodeId": "i-0123456789abcdef0",
                "ErrorCode": "InstanceIdNotFound",
                "Message": INSTANCE_NOT_FOUND,
            }]),
            vec![],
            Value::Null,
        ),
        (
            "known-logical-beside-unknown",
            json!({
                "ClusterName": cluster,
                "NodeLogicalIds": ["no-such-node", nodes[1].logical_id.clone()],
            }),
            vec![],
            Value::Null,
            vec![nodes[1].logical_id.clone()],
            json!([{
                "NodeLogicalId": "no-such-node",
                "ErrorCode": "InstanceIdNotFound",
                "Message": NODE_NOT_FOUND,
            }]),
        ),
    ];
    for (case, request, successful, failed, successful_logical, failed_logical) in rows {
        let replaced = emulator.ok("BatchReplaceClusterNodes", &request).await;
        assert_eq!(
            string_list(&replaced, "Successful").unwrap_or_default(),
            successful,
            "case {case:?}: the known instance ids are the ones replaced, got {replaced}"
        );
        assert_eq!(
            string_list(&replaced, "SuccessfulNodeLogicalIds").unwrap_or_default(),
            successful_logical,
            "case {case:?}: the known logical ids are the ones replaced, got {replaced}"
        );
        assert_eq!(
            replaced.get("Failed").unwrap_or(&Value::Null),
            &failed,
            "case {case:?}: an unknown instance id is reported with its modelled code, got \
             {replaced}"
        );
        assert_eq!(
            replaced.get("FailedNodeLogicalIds").unwrap_or(&Value::Null),
            &failed_logical,
            "case {case:?}: an unknown logical id is reported with its modelled code, got \
             {replaced}"
        );
    }
}

/// RULE: a request NONE of whose identifiers names a node of the cluster is refused
/// outright with a 400 `ValidationException` — there is no per-entry outcome to report
/// when the call can replace nothing — and it replaces nothing on the way to that refusal.
#[tokio::test]
async fn a_request_naming_only_nodes_the_cluster_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let cluster = "replace-only-unknown-identifiers";
    let (_, nodes) = in_service_cluster(&emulator, cluster, &[("pool", "ml.c5.large", 2)]).await;
    // A node that existed and was then DELETED: the stale reference a client works from
    // after reading the node list, and the one the refusal is about.
    let removed = nodes[1].logical_id.clone();
    emulator
        .ok(
            "BatchDeleteClusterNodes",
            &json!({ "ClusterName": cluster, "NodeLogicalIds": [removed.clone()] }),
        )
        .await;
    let surviving = nodes_of(&emulator, cluster).await;

    // Rows differ along what the rule generalises over: which identifier kind names the
    // missing nodes, how many of them one request names, the shape of the identifier (a
    // long and a short instance id, a minted logical id, a made-up one), and whether the
    // node never existed or was deleted after being listed.
    let rows: Vec<(&str, Value)> = vec![
        (
            "one-unknown-instance-id",
            json!({ "ClusterName": cluster, "NodeIds": ["i-0123456789abcdef0"] }),
        ),
        (
            "short-unknown-instance-id",
            json!({ "ClusterName": cluster, "NodeIds": ["i-abcdef01"] }),
        ),
        (
            "several-unknown-instance-ids",
            json!({
                "ClusterName": cluster,
                "NodeIds": ["i-0123456789abcdef0", "i-abcdef01", "i-00000000000000001"],
            }),
        ),
        (
            "one-unknown-logical-id",
            json!({ "ClusterName": cluster, "NodeLogicalIds": ["no-such-node"] }),
        ),
        (
            "stale-logical-id-of-a-deleted-node",
            json!({ "ClusterName": cluster, "NodeLogicalIds": [removed.clone()] }),
        ),
        (
            "both-kinds-unknown",
            json!({
                "ClusterName": cluster,
                "NodeIds": ["i-0123456789abcdef0"],
                "NodeLogicalIds": [removed.clone(), "no-such-node"],
            }),
        ),
    ];
    for (case, request) in rows {
        let (status, body) = emulator.call("BatchReplaceClusterNodes", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        assert_eq!(
            message(&body),
            NONE_EXIST,
            "case {case:?}: expected the all-missing refusal, got {body}"
        );
    }

    // None of the refusals touched the cluster: the surviving node is still the one it was.
    let after = nodes_of(&emulator, cluster).await;
    assert_eq!(
        after, surviving,
        "case refusals: a refused request replaces nothing, got {after:?}"
    );
}

/// RULE: a node the service is already changing cannot take a replacement — a node whose
/// own replacement is in flight is reported `InstanceIdInUse` ("another operation is
/// already in progress for this node") and one that has not finished coming up
/// `InvalidInstanceStatus` — so a replacement never interrupts a change already under way.
#[tokio::test]
async fn a_node_already_mid_change_is_reported_rather_than_replaced_again() {
    let emulator = Emulator::new();
    let cluster = "replace-mid-change";
    let (_, nodes) = in_service_cluster(&emulator, cluster, &[("pool", "ml.c5.large", 2)]).await;
    let node = nodes[0].clone();
    emulator
        .ok(
            "BatchReplaceClusterNodes",
            &json!({ "ClusterName": cluster, "NodeIds": [node.instance_id.clone()] }),
        )
        .await;
    // The same node again, by each identifier kind, while its first replacement is still
    // in flight.
    let again = emulator
        .ok(
            "BatchReplaceClusterNodes",
            &json!({
                "ClusterName": cluster,
                "NodeIds": [node.instance_id.clone()],
                "NodeLogicalIds": [node.logical_id.clone()],
            }),
        )
        .await;
    assert_eq!(
        again.get("Failed"),
        Some(&json!([{
            "NodeId": node.instance_id.clone(),
            "ErrorCode": "InstanceIdInUse",
            "Message": IN_USE,
        }])),
        "case in-flight: a second replacement of the same node is refused per node, got {again}"
    );
    assert_eq!(
        again.get("FailedNodeLogicalIds"),
        Some(&json!([{
            "NodeLogicalId": node.logical_id.clone(),
            "ErrorCode": "InstanceIdInUse",
            "Message": IN_USE,
        }])),
        "case in-flight: the logical id of the same node is refused the same way, got {again}"
    );
    assert_eq!(
        again.get("Successful"),
        None,
        "case in-flight: nothing was replaced a second time, and an empty Successful is not a \
         modelled value, got {again}"
    );
    assert_eq!(
        again.get("SuccessfulNodeLogicalIds"),
        None,
        "case in-flight: nothing was replaced a second time, and an empty \
         SuccessfulNodeLogicalIds is not a modelled value, got {again}"
    );

    // A node that is still coming up has not settled either, so it cannot be replaced
    // until it has. The add owes the node its `Running` from the instant it answers, so
    // the node is held `Pending` by seeding the instant the clock owes it.
    let added = emulator
        .ok(
            "BatchAddClusterNodes",
            &json!({
                "ClusterName": cluster,
                "NodesToAdd": [{ "InstanceGroupName": "pool", "IncrementTargetCountBy": 1 }],
            }),
        )
        .await;
    let provisioning = str_field(
        added
            .get("Successful")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .unwrap_or_else(|| panic!("expected an added node in {added}")),
        "NodeLogicalId",
    );
    owe_nodes_running_at(
        &emulator.engine,
        cluster,
        std::slice::from_ref(&provisioning),
        std::time::SystemTime::now() + FAR_FROM_DUE,
    );
    let refused = emulator
        .ok(
            "BatchReplaceClusterNodes",
            &json!({ "ClusterName": cluster, "NodeLogicalIds": [provisioning.clone()] }),
        )
        .await;
    assert_eq!(
        refused.get("FailedNodeLogicalIds"),
        Some(&json!([{
            "NodeLogicalId": provisioning,
            "ErrorCode": "InvalidInstanceStatus",
            "Message": NODE_INVALID_STATUS,
        }])),
        "case provisioning: a node that has not finished coming up cannot be replaced, got \
         {refused}"
    );
}

/// RULE: a request that cannot name the nodes to replace is refused outright rather than
/// answered — it must name at least one identifier, and no more identifiers than one call
/// may replace ("you can replace up to 25 nodes in a single request") — and a cluster this
/// account does not hold is the modelled `ResourceNotFound`, whichever form named it.
#[tokio::test]
async fn a_request_that_cannot_name_the_nodes_to_replace_is_refused() {
    let emulator = Emulator::new();
    let cluster = "replace-refusals";
    let (arn, nodes) = in_service_cluster(&emulator, cluster, &[("pool", "ml.c5.large", 1)]).await;
    let node = nodes[0].clone();
    // Thirteen of each kind: inside each list's own modelled bound of 25, past the 25
    // nodes one call may replace.
    let thirteen: Vec<String> = (0..13).map(|at| format!("i-0123456789abcde{at:x}")).collect();
    let thirteen_logical: Vec<String> = (0..13).map(|at| format!("no-such-node-{at}")).collect();
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "no-identifier",
            json!({ "ClusterName": cluster }),
            "ValidationException",
        ),
        (
            "past-the-batch-bound",
            json!({
                "ClusterName": cluster,
                "NodeIds": thirteen,
                "NodeLogicalIds": thirteen_logical,
            }),
            "ValidationException",
        ),
        (
            "unknown-cluster-by-name",
            json!({ "ClusterName": "no-such-cluster", "NodeIds": [node.instance_id.clone()] }),
            "ResourceNotFound",
        ),
        (
            "unknown-cluster-by-arn",
            json!({
                "ClusterName": format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:cluster/aaaaaaaaaaaa"),
                "NodeIds": [node.instance_id.clone()],
            }),
            "ResourceNotFound",
        ),
    ];
    for (case, request, expected) in rows {
        let (status, body) = emulator.call("BatchReplaceClusterNodes", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
        assert!(
            !message(&body).is_empty(),
            "case {case:?}: a refusal says what was wrong, got {body}"
        );
    }
    // None of the refusals touched the cluster: the node is still the one it was.
    let (_, after) = (arn, nodes_of(&emulator, cluster).await);
    assert_eq!(
        after,
        vec![node],
        "case refusals: a refused request replaces nothing, got {after:?}"
    );
}
