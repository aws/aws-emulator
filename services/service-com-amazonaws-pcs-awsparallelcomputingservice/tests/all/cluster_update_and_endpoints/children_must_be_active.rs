// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the second half of `UpdateCluster`'s documented precondition,
//! driven over the awsJson1_0 wire through the same router the emulator serves from, on the
//! harness [`crate::cluster_lifecycle`] builds.
//!
//! RULE under test: an update of a cluster is served only while every resource stored inside
//! that cluster — each of its compute node groups and each of its queues — is `ACTIVE`; a
//! child in any other status the service can report it in makes the update a modelled
//! `ConflictException` naming that child, and the cluster keeps the configuration and the
//! status it had.
//!
//! Nothing here waits on the wall clock: a transition this service owes is materialised
//! before the next request is answered.

use serde_json::{json, Value};
use service_com_amazonaws_pcs_awsparallelcomputingservice::ops::types::{
    COMPUTE_NODE_GROUP_STATUS_VALUES, QUEUE_STATUS_VALUES,
};
use service_com_amazonaws_pcs_awsparallelcomputingservice::service::wire::CLIENT_ERROR_STATUS;
use service_com_amazonaws_pcs_awsparallelcomputingservice::state::{service_state, STATUS_ACTIVE};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{
    call, create_cluster, create_compute_node_group, create_queue, emulator, emulator_with_engine,
    stored_cluster,
};

/// The value the cluster of each case starts out with, so a refusal that stored something
/// is visible as a changed `scaleDownIdleTimeInSeconds`.
const SETTLED_SCALE_DOWN: i64 = 600;

/// The value every update in this suite asks for, which only an accepted one may persist.
const REQUESTED_SCALE_DOWN: i64 = 700;

/// Which kind of child a case holds in a non-`ACTIVE` status: how a refusal names it in
/// `resourceType`, the noun its sentence uses, and the status members the model declares for
/// it.
struct ChildKind {
    /// The case label, and the name the child is created under.
    label: &'static str,
    /// The `resourceType` a refusal about this kind of child carries.
    resource_type: &'static str,
    /// The sentence a refusal about this kind of child states.
    message: &'static str,
    /// Every member of this child's own status enum, the model's own list.
    statuses: &'static [&'static str],
}

/// The compute node group kind, with the members of `ComputeNodeGroupStatus`
/// (`smithy_query(query="shape",shape="ComputeNodeGroupStatus")`, exported by the crate as
/// `COMPUTE_NODE_GROUP_STATUS_VALUES`).
const GROUP_KIND: ChildKind = ChildKind {
    label: "compute-node-group",
    resource_type: "computenodegroup",
    message: "A compute node group isn't in ACTIVE status.",
    statuses: COMPUTE_NODE_GROUP_STATUS_VALUES,
};

/// The queue kind, with the members of `QueueStatus`
/// (`smithy_query(query="shape",shape="QueueStatus")`, exported by the crate as
/// `QUEUE_STATUS_VALUES`).
const QUEUE_KIND: ChildKind = ChildKind {
    label: "queue",
    resource_type: "queue",
    message: "A queue isn't in ACTIVE status.",
    statuses: QUEUE_STATUS_VALUES,
};

/// An `UpdateCluster` request asking for `seconds`.
fn update_cluster(cluster: &str, seconds: i64) -> Value {
    json!({
        "clusterIdentifier": cluster,
        "slurmConfiguration": {"scaleDownIdleTimeInSeconds": seconds},
    })
}

/// Create a cluster carrying [`SETTLED_SCALE_DOWN`], with `groups` compute node groups and
/// `queues` queues inside it, and answer the cluster's id with the ids of the children in
/// creation order. Every resource is settled `ACTIVE` by the time this returns, because the
/// request that follows a create observes it settled.
///
/// A stored queue assigns jobs to at least one compute node group, so each queue associates
/// the first group — a cluster holding a queue holds a group as well.
async fn cluster_with_children(
    router: &MultiServiceRouter,
    name: &str,
    groups: usize,
    queues: usize,
) -> (String, Vec<String>, Vec<String>) {
    assert!(
        queues == 0 || groups > 0,
        "setup {name}: a queue associates a compute node group, so {queues} queue(s) need \
         at least one group, got {groups}"
    );
    let mut request = create_cluster(name, "SMALL");
    request["slurmConfiguration"] = json!({"scaleDownIdleTimeInSeconds": SETTLED_SCALE_DOWN});
    let (status, created) = call(router, "CreateCluster", &request).await;
    assert_eq!(
        status, 200,
        "setup {name}: CreateCluster got {status} with {created}"
    );
    let cluster_id = created["cluster"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let mut group_ids = Vec::new();
    for index in 0..groups {
        let (status, created) = call(
            router,
            "CreateComputeNodeGroup",
            &create_compute_node_group(&cluster_id, &group_name(index)),
        )
        .await;
        assert_eq!(
            status, 200,
            "setup {name}: CreateComputeNodeGroup got {status} with {created}"
        );
        group_ids.push(
            created["computeNodeGroup"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    let mut queue_ids = Vec::new();
    for index in 0..queues {
        let (status, created) = call(
            router,
            "CreateQueue",
            &create_queue(&cluster_id, &queue_name(index), &[group_name(0).as_str()]),
        )
        .await;
        assert_eq!(
            status, 200,
            "setup {name}: CreateQueue got {status} with {created}"
        );
        queue_ids.push(
            created["queue"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    // One more request, so the last create's transition is materialised and every child is
    // reported ACTIVE before a case starts.
    let settled = stored_cluster(router, &cluster_id).await;
    assert_eq!(
        settled["status"],
        json!(STATUS_ACTIVE),
        "setup {name}: expected the cluster settled, got {settled}"
    );
    (cluster_id, group_ids, queue_ids)
}

/// The name the `index`th compute node group of a case is created under.
fn group_name(index: usize) -> String {
    format!("nodes-{index}")
}

/// The name the `index`th queue of a case is created under.
fn queue_name(index: usize) -> String {
    format!("queue-{index}")
}

/// Hold the child named `name` of the cluster `cluster` in `status` with nothing owed. No
/// sequence of requests leaves a child in most of these, so this is written through the
/// service's own store, the way the sibling suites hold a resource in a status.
fn hold_child(
    engine: &engine::InMemoryEngine,
    cluster: &str,
    kind: &ChildKind,
    name: &str,
    status: &str,
) {
    let store = service_state(engine).expect("this service's own store");
    let mut state = store.lock();
    let held = state
        .cluster_mut(cluster)
        .expect("the cluster the create persisted");
    if kind.resource_type == GROUP_KIND.resource_type {
        let group = held
            .compute_node_groups
            .values_mut()
            .find(|group| group.name == name)
            .expect("the compute node group the create persisted");
        group.status = status.to_string();
        group.pending = None;
    } else {
        let queue = held
            .queues
            .values_mut()
            .find(|queue| queue.name == name)
            .expect("the queue the create persisted");
        queue.status = status.to_string();
        queue.pending = None;
    }
}

/// RULE: a cluster holding a child in any status other than `ACTIVE` takes no update — the
/// call is refused with the modelled `ConflictException` naming that child, and the stored
/// cluster keeps its configuration and its status.
///
/// The rows span both kinds of child the documentation names and EVERY member of each
/// child's own status enum other than `ACTIVE`, so the class is covered rather than the one
/// status a delete happens to leave behind.
#[tokio::test]
async fn an_update_is_refused_while_a_child_of_the_cluster_is_not_active() {
    for kind in [&GROUP_KIND, &QUEUE_KIND] {
        let unsettled = kind
            .statuses
            .iter()
            .filter(|status| **status != STATUS_ACTIVE);
        for held in unsettled {
            let case = format!("{} held {held}", kind.label);
            let (router, engine) = emulator_with_engine();
            // One child of each kind, so a refusal has to name the one that is not ACTIVE
            // rather than the only child there is.
            let (cluster_id, group_ids, queue_ids) =
                cluster_with_children(&router, "gated-cluster", 1, 1).await;
            let (name, expected_id) = if kind.resource_type == GROUP_KIND.resource_type {
                (group_name(0), group_ids[0].clone())
            } else {
                (queue_name(0), queue_ids[0].clone())
            };
            hold_child(&engine, &cluster_id, kind, &name, held);

            let (status, refused) = call(
                &router,
                "UpdateCluster",
                &update_cluster(&cluster_id, REQUESTED_SCALE_DOWN),
            )
            .await;
            assert_eq!(
                status, CLIENT_ERROR_STATUS,
                "case {case}: expected the client-error status, got {status} with {refused}"
            );
            assert_eq!(
                refused["__type"],
                json!("ConflictException"),
                "case {case}: expected a conflict, got {refused}"
            );
            assert_eq!(
                refused["message"],
                json!(kind.message),
                "case {case}: expected the service's own sentence, got {refused}"
            );
            assert_eq!(
                (
                    refused["resourceId"].as_str(),
                    refused["resourceType"].as_str()
                ),
                (Some(expected_id.as_str()), Some(kind.resource_type)),
                "case {case}: expected the refusal to name the child that is not ACTIVE, \
                 got {refused}"
            );

            let read = stored_cluster(&router, &cluster_id).await;
            assert_eq!(
                read["slurmConfiguration"]["scaleDownIdleTimeInSeconds"],
                json!(SETTLED_SCALE_DOWN),
                "case {case}: expected the refusal to store nothing, got {read}"
            );
            assert_eq!(
                read["status"],
                json!(STATUS_ACTIVE),
                "case {case}: expected the cluster left where it was, got {read}"
            );
        }
    }
}

/// RULE, the other way round: however many children a cluster holds, an update is served
/// while every one of them is `ACTIVE`.
#[tokio::test]
async fn an_update_is_served_while_every_child_of_the_cluster_is_active() {
    // (case, compute node groups, queues)
    let rows: [(&str, usize, usize); 4] = [
        ("no child at all", 0, 0),
        ("one compute node group", 1, 0),
        ("one queue, with the group it assigns jobs to", 1, 1),
        ("two children of each kind", 2, 2),
    ];
    for (case, groups, queues) in rows {
        let router = emulator();
        let (cluster_id, _, _) =
            cluster_with_children(&router, "open-cluster", groups, queues).await;

        let (status, updated) = call(
            &router,
            "UpdateCluster",
            &update_cluster(&cluster_id, REQUESTED_SCALE_DOWN),
        )
        .await;
        assert_eq!(status, 200, "case {case}: got {status} with {updated}");
        assert_eq!(
            updated["cluster"]["slurmConfiguration"]["scaleDownIdleTimeInSeconds"],
            json!(REQUESTED_SCALE_DOWN),
            "case {case}: expected the requested value applied, got {updated}"
        );

        let read = stored_cluster(&router, &cluster_id).await;
        assert_eq!(
            read["slurmConfiguration"]["scaleDownIdleTimeInSeconds"],
            json!(REQUESTED_SCALE_DOWN),
            "case {case}: expected the value to persist, got {read}"
        );
    }
}

/// RULE applied to the one non-`ACTIVE` child a plain sequence of requests leaves behind: a
/// deleted compute node group is reported `DELETED` for one more request, and an update
/// issued in that window is refused — then served once the record is gone.
#[tokio::test]
async fn an_update_is_refused_while_a_deleted_compute_node_group_is_still_reported() {
    let router = emulator();
    let (cluster_id, group_ids, _) =
        cluster_with_children(&router, "deleting-cluster", 1, 0).await;

    let read_group = json!({
        "clusterIdentifier": &cluster_id,
        "computeNodeGroupIdentifier": &group_ids[0],
    });
    let (status, deleted) = call(&router, "DeleteComputeNodeGroup", &read_group).await;
    assert_eq!(
        status, 200,
        "case delete the group: got {status} with {deleted}"
    );

    // The request that follows the delete finds the group DELETED and still stored, so it
    // is a child that is not ACTIVE.
    let (status, refused) = call(
        &router,
        "UpdateCluster",
        &update_cluster(&cluster_id, REQUESTED_SCALE_DOWN),
    )
    .await;
    assert_eq!(
        status, CLIENT_ERROR_STATUS,
        "case update while the group is still reported: got {status} with {refused}"
    );
    assert_eq!(
        refused["__type"],
        json!("ConflictException"),
        "case update while the group is still reported: got {refused}"
    );
    assert_eq!(
        refused["resourceId"],
        json!(&group_ids[0]),
        "case update while the group is still reported: expected the refusal to name the \
         group, got {refused}"
    );

    let read = stored_cluster(&router, &cluster_id).await;
    assert_eq!(
        read["slurmConfiguration"]["scaleDownIdleTimeInSeconds"],
        json!(SETTLED_SCALE_DOWN),
        "case update while the group is still reported: expected the refusal to store \
         nothing, got {read}"
    );

    // The record is forgotten the request after that, and then the cluster holds no child
    // at all: the same update is served.
    let (status, served) = call(
        &router,
        "UpdateCluster",
        &update_cluster(&cluster_id, REQUESTED_SCALE_DOWN),
    )
    .await;
    assert_eq!(
        status, 200,
        "case update once the group is gone: got {status} with {served}"
    );
    assert_eq!(
        served["cluster"]["slurmConfiguration"]["scaleDownIdleTimeInSeconds"],
        json!(REQUESTED_SCALE_DOWN),
        "case update once the group is gone: expected the requested value applied, got \
         {served}"
    );
}
