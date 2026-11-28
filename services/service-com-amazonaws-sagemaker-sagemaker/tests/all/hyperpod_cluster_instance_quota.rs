// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for what `BatchAddClusterNodes` REFUSES and how it words it: the
//! ACCOUNT-level bound on the instances a client may put into its SageMaker HyperPod
//! clusters, and the per-entry sentence a specification naming an instance group the cluster
//! does not hold is reported with.
//!
//! The quota suite asserts a RULE over a TABLE of accounts rather than one recorded call: an
//! addition is weighed against what every LIVE cluster of the account already holds, not
//! against the cluster it names, so it is refused with the modelled `ResourceLimitExceeded`
//! exactly when that utilisation plus the request's own delta passes the applied value, and
//! a refusal leaves the group's target count and the cluster's node list untouched. The rows
//! differ along the delta, the utilisation and how it is spread over the account's clusters,
//! and whether the clusters holding it are still live.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses.

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::state as sm;

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The most nodes ONE request may ask for: `AddClusterNodeSpecification`'s
/// `IncrementTargetCountBy` is modelled `@range 1..50`, and its own doc caps a whole request
/// at "50" nodes across every instance group (smithy_query
/// shape=AddClusterNodeSpecification).
const MOST_NODES_PER_REQUEST: i64 = 50;

/// One instance group of a HyperPod cluster. `InstanceGroupName`, `InstanceCount`,
/// `InstanceType` and `ExecutionRole` are the members a create gives a group (smithy_query
/// shape=ClusterInstanceGroupSpecification).
fn instance_group(name: &str, instances: i64) -> Value {
    json!({
        "InstanceGroupName": name,
        "InstanceCount": instances,
        "InstanceType": "ml.c5.xlarge",
        "ExecutionRole": format!("arn:aws:iam::{ACCOUNT}:role/hyperpod-{name}"),
    })
}

/// Create a cluster holding one group named `pool` with `instances` instances, and settle its
/// provisioning so the account's utilisation is the instances it holds. Answers its
/// `ClusterArn`.
async fn create_cluster(emulator: &Emulator, name: &str, instances: i64) -> String {
    let created = emulator
        .ok(
            "CreateCluster",
            &json!({
                "ClusterName": name,
                "InstanceGroups": [instance_group("pool", instances)],
            }),
        )
        .await;
    // The create owes the cluster its `InService` from the instant it answers, and that is
    // when its groups' instances are minted, so one read settles it.
    emulator
        .ok("DescribeCluster", &json!({ "ClusterName": name }))
        .await;
    str_field(&created, "ClusterArn")
}

/// Add `delta` nodes to the `pool` group of `cluster`.
async fn add_nodes(emulator: &Emulator, cluster: &str, delta: i64) -> (u16, Value) {
    emulator
        .call(
            "BatchAddClusterNodes",
            &json!({
                "ClusterName": cluster,
                "NodesToAdd": [{ "InstanceGroupName": "pool", "IncrementTargetCountBy": delta }],
            }),
        )
        .await
}

/// How SageMaker words a refusal past this quota: the published quota by name, its applied
/// value, what the account is already using and what the request asks on top, each counted in
/// instances, then how to ask for more.
///
/// A literal on purpose: this suite is what pins the wording, so it does not ask the
/// implementation what it is. Only the three numbers vary, and each comes from the account's
/// own state and the request.
fn refusal(limit: i64, in_use: i64, delta: i64) -> String {
    format!(
        "The account-level service limit 'Total number of instances allowed across SageMaker \
         Clusters' is {limit} Instances, with current utilization of {in_use} Instances and a \
         request delta of {delta} Instances. Please use AWS Service Quotas to request an \
         increase for this quota. If AWS Service Quotas is not available, contact AWS support \
         to request an increase for this quota."
    )
}

/// The `TargetCount` the group `pool` of `cluster` reports, and how many nodes the cluster
/// holds — the two things a refused addition must leave alone.
async fn group_state(emulator: &Emulator, case: &str, cluster: &str) -> (i64, usize) {
    let described = emulator
        .ok("DescribeCluster", &json!({ "ClusterName": cluster }))
        .await;
    let target = described
        .get("InstanceGroups")
        .and_then(Value::as_array)
        .and_then(|groups| groups.first())
        .and_then(|group| group.get("TargetCount"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a group TargetCount in {described}"));
    let listed = emulator
        .ok(
            "ListClusterNodes",
            &json!({ "ClusterName": cluster, "IncludeNodeLogicalIds": true }),
        )
        .await;
    let held = listed
        .get("ClusterNodeSummaries")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    (target, held)
}

/// RULE: a request that would put more instances into the account's HyperPod clusters than
/// its applied account-level quota allows is refused with `ResourceLimitExceeded` naming that
/// quota, its value, the instances the account's live clusters already hold and the request's
/// own delta — and nothing is added; a request that fits inside the bound is served.
#[tokio::test]
async fn an_addition_is_weighed_against_the_accounts_whole_cluster_utilisation() {
    let limit = sm::CLUSTER_INSTANCES_QUOTA_DEFAULT;
    // The applied value has to be reachable by one request for the rows below to mean
    // anything: an account that has asked for no increase cannot absorb a maximal scale-out.
    assert!(
        limit < MOST_NODES_PER_REQUEST,
        "the applied quota ({limit}) must be below the {MOST_NODES_PER_REQUEST} nodes one \
         request may ask for"
    );

    // One cluster holding a single instance, asked for a maximal scale-out: past the bound,
    // so nothing is added.
    let emulator = Emulator::new();
    create_cluster(&emulator, "small", 1).await;
    let before = group_state(&emulator, "maximal scale-out", "small").await;
    let (status, refused) = add_nodes(&emulator, "small", MOST_NODES_PER_REQUEST).await;
    assert!(
        status == 400 && error_code(&refused) == "ResourceLimitExceeded",
        "case \"maximal scale-out\": expected 400 ResourceLimitExceeded, got {status} with \
         {refused}"
    );
    let expected = refusal(limit, 1, MOST_NODES_PER_REQUEST);
    assert!(
        message(&refused) == expected,
        "case \"maximal scale-out\": expected the message {expected:?}, got {:?}",
        message(&refused)
    );
    let after = group_state(&emulator, "maximal scale-out", "small").await;
    assert!(
        after == before,
        "case \"maximal scale-out\": a refused addition must leave the group's target count \
         and node list untouched, got {after:?} after {before:?}"
    );

    // The boundary: the addition that exactly fills the applied value is served, and the
    // next instance past it is refused with the utilisation the account now holds.
    let (status, served) = add_nodes(&emulator, "small", limit - 1).await;
    assert!(
        status == 200,
        "case \"exactly at the bound\": expected the addition to be served, got {status} with \
         {served}"
    );
    let (target, _) = group_state(&emulator, "exactly at the bound", "small").await;
    assert!(
        target == limit,
        "case \"exactly at the bound\": the group's target count must have moved with the \
         nodes added, expected {limit}, got {target}"
    );
    let (status, refused) = add_nodes(&emulator, "small", 1).await;
    assert!(
        status == 400 && message(&refused) == refusal(limit, limit, 1),
        "case \"one past the bound\": expected the refusal {:?}, got {status} with {refused}",
        refusal(limit, limit, 1)
    );

    // The bound is the ACCOUNT's, not one cluster's: utilisation spread over two clusters is
    // weighed the same way, and an addition to the smaller one is refused for what the other
    // holds.
    let emulator = Emulator::new();
    create_cluster(&emulator, "bulk", limit - 8).await;
    create_cluster(&emulator, "side", 4).await;
    let in_use = limit - 4;
    let (status, refused) = add_nodes(&emulator, "side", 8).await;
    assert!(
        status == 400 && message(&refused) == refusal(limit, in_use, 8),
        "case \"utilisation across two clusters\": expected the refusal {:?}, got {status} \
         with {refused}",
        refusal(limit, in_use, 8)
    );

    // A cluster whose teardown has begun holds no capacity any more, so the same addition is
    // served once the other cluster is deleted.
    emulator
        .ok("DeleteCluster", &json!({ "ClusterName": "bulk" }))
        .await;
    let (status, served) = add_nodes(&emulator, "side", 8).await;
    assert!(
        status == 200,
        "case \"after the other cluster is deleted\": expected the addition to be served, got \
         {status} with {served}"
    );
    let (target, _) = group_state(&emulator, "after the other cluster is deleted", "side").await;
    assert!(
        target == 12,
        "case \"after the other cluster is deleted\": the group's target count must have \
         moved with the 8 nodes added to its 4, got {target}"
    );
}

/// RULE: a specification naming an instance group the cluster does not hold is one `Failed`
/// entry whose sentence names the group THE REQUEST named, in fixed text that says nothing
/// about the cluster; the call itself is served, and the specifications naming groups the
/// cluster does hold are still added.
#[tokio::test]
async fn a_specification_naming_an_unheld_group_states_the_group_the_request_named() {
    let emulator = Emulator::new();
    create_cluster(&emulator, "grouped", 0).await;

    // Rows differ along the group name's own shape — the sentence quotes it as given and
    // carries nothing else that varies.
    let unheld = ["no-such-group", "Pool-2", "g", "sim-grp-a"];
    for group in unheld {
        let added = emulator
            .ok(
                "BatchAddClusterNodes",
                &json!({
                    "ClusterName": "grouped",
                    "NodesToAdd": [
                        { "InstanceGroupName": group, "IncrementTargetCountBy": 3 },
                        { "InstanceGroupName": "pool", "IncrementTargetCountBy": 1 },
                    ],
                }),
            )
            .await;
        let failed = added
            .get("Failed")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            failed.len() == 1,
            "case {group:?}: expected one Failed entry for the group the cluster does not \
             hold, got {added}"
        );
        let entry = &failed[0];
        // A literal on purpose: this suite is what pins the wording, and the group is the
        // only part of it that comes from the request.
        let expected = format!(
            "Instance group provided as part of the request ({group}) does not exist. Please \
             check the input and try again."
        );
        assert!(
            str_field(entry, "Message") == expected,
            "case {group:?}: expected the sentence {expected:?}, got {entry}"
        );
        assert!(
            str_field(entry, "ErrorCode") == "InstanceGroupNotFound"
                && entry.get("FailedCount").and_then(Value::as_i64) == Some(3)
                && str_field(entry, "InstanceGroupName") == group,
            "case {group:?}: the entry must name the group, the modelled code and how many \
             of its nodes were not added, got {entry}"
        );
        // The specification the cluster CAN satisfy was still served, so the per-entry
        // failure is one entry rather than a refusal of the whole call.
        let successful = added
            .get("Successful")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        assert!(
            successful == 1,
            "case {group:?}: the group the cluster holds must still have its node added, got \
             {added}"
        );
    }
}
