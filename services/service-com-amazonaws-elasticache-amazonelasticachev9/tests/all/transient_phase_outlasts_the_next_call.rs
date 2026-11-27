// SPDX-License-Identifier: CC0-1.0

//! That the transient phase a call starts out-lasts the call that FOLLOWS it.
//!
//! The rule under test: a lifecycle transition this service owes a resource falls due a
//! fixed, non-zero span after the call that started the work
//! ([`state::CLUSTER_TRANSITION`]), and that span out-lasts the next request — so a client
//! that creates, changes or removes a resource and acts on it straight away meets the
//! resource in its transient phase, and the modelled state fault AWS serves for a resource
//! that is still being worked on is reachable over the wire. A span shorter than one
//! request round trip leaves every lifecycle state gate of this service unreachable: the
//! record is settled again by the time the next request arrives, so a mutation AWS refuses
//! is accepted, and a client's guard against that refusal is never exercised.
//!
//! Nothing here touches the store and nothing sleeps: every row is two or three ordinary
//! `awsQuery` calls in a row, through the same router the production emulator mounts. The
//! rows span what the rule generalises over — WHICH resource (cache cluster, replication
//! group, serverless cache, user group, snapshot), WHICH phase (being created, changed,
//! restarted, snapshotted, migrated, removed) and WHICH gate the phase closes (a delete, a
//! modify, a replica-count change, a snapshot, a migration).
//!
//! The companion of [`crate::first_read_sees_the_settled_state`], which asserts the other
//! half: once that owed change falls due, the next read reports the settled state.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{assert_refusal, params, poll_until, Emulator, Reply, REGION};

/// The cache node type every row creates with: the smallest the catalog publishes.
const NODE_TYPE: &str = "cache.t3.micro";

/// Create one cache cluster and answer the create's reply.
async fn create_cluster(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .call(
            "CreateCacheCluster",
            &params(&[
                ("CacheClusterId", id),
                ("Engine", "redis"),
                ("CacheNodeType", NODE_TYPE),
                ("NumCacheNodes", "1"),
            ]),
        )
        .await
}

/// Create one replication group and answer the create's reply.
async fn create_group(emulator: &Emulator, id: &str, extra: &[(&str, &str)]) -> Reply {
    let mut pairs = params(&[
        ("ReplicationGroupId", id),
        ("ReplicationGroupDescription", "a group this suite drives"),
        ("Engine", "redis"),
        ("CacheNodeType", NODE_TYPE),
    ]);
    pairs.extend(params(extra));
    emulator.call("CreateReplicationGroup", &pairs).await
}

/// Create one cache cluster and look again until the clock has made it `available`.
async fn available_cluster(emulator: &Emulator, case: &str, id: &str) {
    let created = create_cluster(emulator, id).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the cluster {id} must be created, got {} {}",
        created.status, created.body
    );
    poll_until(
        emulator,
        REGION,
        case,
        &format!("the cluster {id} reaching available"),
        "DescribeCacheClusters",
        &params(&[("CacheClusterId", id)]),
        |reply| reply.text("CacheCluster/CacheClusterStatus") == Some(state::STATUS_AVAILABLE),
    )
    .await;
}

/// Create one replication group and look again until the clock has made it `available`.
async fn available_group(emulator: &Emulator, case: &str, id: &str, extra: &[(&str, &str)]) {
    let created = create_group(emulator, id, extra).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the group {id} must be created, got {} {}",
        created.status, created.body
    );
    poll_until(
        emulator,
        REGION,
        case,
        &format!("the group {id} reaching available"),
        "DescribeReplicationGroups",
        &params(&[("ReplicationGroupId", id)]),
        |reply| reply.text("ReplicationGroup/Status") == Some(state::STATUS_AVAILABLE),
    )
    .await;
}

/// One phase row: the call that starts work on a resource, where its answer carries the
/// resource's status and the transient value it must report there, and the read that
/// FOLLOWS it — which must report the same transient value, because the span the clock owes
/// the change out-lasts that read.
struct Phase {
    case: &'static str,
    start: &'static str,
    start_request: Vec<(String, String)>,
    status_path: &'static str,
    transient: &'static str,
    read: &'static str,
    read_request: Vec<(String, String)>,
    /// Where the read reports the status, when the reader is not the starting call's own
    /// projection (a snapshot's cluster, for instance, is read as a cluster).
    read_path: &'static str,
}

/// Drive one row: the starting call, then the read that follows it with nothing in between.
async fn assert_phase_outlasts_the_next_call(emulator: &Emulator, phase: &Phase) {
    let started = emulator.call(phase.start, &phase.start_request).await;
    assert_eq!(
        started.status, 200,
        "case {:?}: {} must answer 200, got {} {}",
        phase.case, phase.start, started.status, started.body
    );
    assert_eq!(
        started.text(phase.status_path),
        Some(phase.transient),
        "case {:?}: {} must answer with the transient status {:?}, got {:?}",
        phase.case,
        phase.start,
        phase.transient,
        started.text(phase.status_path)
    );

    let read = emulator.call(phase.read, &phase.read_request).await;
    assert_eq!(
        read.status, 200,
        "case {:?}: the {} after {} must answer 200, got {} {}",
        phase.case, phase.read, phase.start, read.status, read.body
    );
    assert_eq!(
        read.text(phase.read_path),
        Some(phase.transient),
        "case {:?}: the {} issued straight after {} must still report {:?}, got {:?} in {}",
        phase.case,
        phase.read,
        phase.start,
        phase.transient,
        read.text(phase.read_path),
        read.body
    );
}

#[tokio::test]
async fn the_call_after_a_create_still_meets_the_resource_being_created() {
    let emulator = Emulator::new();

    // The axis: which resource was created, and so which transient status its own status
    // member documents for a resource that is not ready yet.
    let rows = [
        Phase {
            case: "a cache cluster",
            start: "CreateCacheCluster",
            start_request: params(&[
                ("CacheClusterId", "phase-cluster"),
                ("Engine", "redis"),
                ("CacheNodeType", NODE_TYPE),
                ("NumCacheNodes", "1"),
            ]),
            status_path: "CacheClusterStatus",
            transient: state::STATUS_CREATING,
            read: "DescribeCacheClusters",
            read_request: params(&[("CacheClusterId", "phase-cluster")]),
            read_path: "CacheClusterStatus",
        },
        Phase {
            case: "a replication group",
            start: "CreateReplicationGroup",
            start_request: params(&[
                ("ReplicationGroupId", "phase-group"),
                ("ReplicationGroupDescription", "two calls, one phase"),
                ("Engine", "redis"),
                ("CacheNodeType", NODE_TYPE),
            ]),
            status_path: "ReplicationGroup/Status",
            transient: state::STATUS_CREATING,
            read: "DescribeReplicationGroups",
            read_request: params(&[("ReplicationGroupId", "phase-group")]),
            read_path: "ReplicationGroup/Status",
        },
        Phase {
            case: "a serverless cache",
            start: "CreateServerlessCache",
            start_request: params(&[
                ("ServerlessCacheName", "phase-serverless"),
                ("Engine", "valkey"),
            ]),
            status_path: "ServerlessCache/Status",
            transient: state::SERVERLESS_STATUS_CREATING,
            read: "DescribeServerlessCaches",
            read_request: params(&[("ServerlessCacheName", "phase-serverless")]),
            read_path: "Status",
        },
        Phase {
            case: "a user group",
            start: "CreateUserGroup",
            start_request: params(&[
                ("UserGroupId", "phase-rbac"),
                ("Engine", "redis"),
                ("UserIds.member.1", "default"),
            ]),
            status_path: "Status",
            transient: state::STATUS_CREATING,
            read: "DescribeUserGroups",
            read_request: params(&[("UserGroupId", "phase-rbac")]),
            read_path: "Status",
        },
    ];

    for row in &rows {
        assert_phase_outlasts_the_next_call(&emulator, row).await;
    }
}

#[tokio::test]
async fn the_call_after_a_change_still_meets_the_resource_being_changed() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "a reboot", "phase-reboot").await;
    available_cluster(&emulator, "a snapshot", "phase-snapped").await;
    available_cluster(&emulator, "a modify", "phase-modified").await;
    let cache = emulator
        .call(
            "CreateServerlessCache",
            &params(&[
                ("ServerlessCacheName", "phase-doomed"),
                ("Engine", "valkey"),
            ]),
        )
        .await;
    assert_eq!(
        cache.status, 200,
        "the serverless cache must be created, got {} {}",
        cache.status, cache.body
    );
    poll_until(
        &emulator,
        REGION,
        "a removal",
        "the serverless cache reaching available",
        "DescribeServerlessCaches",
        &params(&[("ServerlessCacheName", "phase-doomed")]),
        |reply| reply.text("Status") == Some(state::SERVERLESS_STATUS_AVAILABLE),
    )
    .await;

    // The axis: WHICH kind of work the call started on a settled resource — node work, a
    // restart of its nodes, a backup of its keyspace, a removal — and so which transient
    // status the resource carries while that work is owed.
    let rows = [
        Phase {
            case: "node work on a cache cluster",
            start: "ModifyCacheCluster",
            start_request: params(&[
                ("CacheClusterId", "phase-modified"),
                ("EngineVersion", "7.0"),
                ("ApplyImmediately", "true"),
            ]),
            status_path: "CacheClusterStatus",
            transient: state::STATUS_MODIFYING,
            read: "DescribeCacheClusters",
            read_request: params(&[("CacheClusterId", "phase-modified")]),
            read_path: "CacheClusterStatus",
        },
        Phase {
            case: "a reboot of a cluster's nodes",
            start: "RebootCacheCluster",
            start_request: params(&[
                ("CacheClusterId", "phase-reboot"),
                ("CacheNodeIdsToReboot.CacheNodeId.1", "0001"),
            ]),
            status_path: "CacheClusterStatus",
            transient: state::STATUS_REBOOTING_NODES,
            read: "DescribeCacheClusters",
            read_request: params(&[("CacheClusterId", "phase-reboot")]),
            read_path: "CacheClusterStatus",
        },
        Phase {
            case: "a snapshot being taken of a cluster",
            start: "CreateSnapshot",
            start_request: params(&[
                ("SnapshotName", "phase-snap"),
                ("CacheClusterId", "phase-snapped"),
            ]),
            status_path: "SnapshotStatus",
            transient: state::STATUS_CREATING,
            read: "DescribeSnapshots",
            read_request: params(&[("SnapshotName", "phase-snap")]),
            read_path: "SnapshotStatus",
        },
        Phase {
            case: "a serverless cache on its way out",
            start: "DeleteServerlessCache",
            start_request: params(&[("ServerlessCacheName", "phase-doomed")]),
            status_path: "ServerlessCache/Status",
            transient: state::SERVERLESS_STATUS_DELETING,
            read: "DescribeServerlessCaches",
            read_request: params(&[("ServerlessCacheName", "phase-doomed")]),
            read_path: "Status",
        },
    ];

    for row in &rows {
        assert_phase_outlasts_the_next_call(&emulator, row).await;
    }

    // The cluster whose keyspace is being captured is itself busy: the snapshot's own
    // transient status is reported beside the cluster's, by the cluster's reader.
    let busy = emulator
        .call(
            "DescribeCacheClusters",
            &params(&[("CacheClusterId", "phase-snapped")]),
        )
        .await;
    assert_eq!(
        busy.text("CacheCluster/CacheClusterStatus"),
        Some(state::STATUS_SNAPSHOTTING),
        "the cluster a snapshot is being taken of must still report {:?}, got {}",
        state::STATUS_SNAPSHOTTING,
        busy.body
    );
}

/// One gate row: the case, the call that puts a FRESH resource in its transient phase, the
/// mutation issued straight after it, and the refusal that mutation must be answered with.
struct Gate {
    case: &'static str,
    start: &'static str,
    start_request: Vec<(String, String)>,
    refused: &'static str,
    refused_request: Vec<(String, String)>,
    code: &'static str,
    /// The sentence the refusal carries, when the service states one for that gate; `None`
    /// where only the status and the code are the subject of the row.
    message: Option<String>,
}

#[tokio::test]
async fn a_mutation_issued_while_a_resource_is_still_being_created_is_refused() {
    let emulator = Emulator::new();

    // The axis: WHICH gate the transient phase closes — a delete, a modify, a replica-count
    // change, a backup, a membership change — over the three resources whose create leaves
    // the resource not ready. Each row's mutation is the very next call after the create.
    let rows = [
        Gate {
            case: "a delete of a cluster that is still being created",
            start: "CreateCacheCluster",
            start_request: params(&[
                ("CacheClusterId", "gate-del-cluster"),
                ("Engine", "redis"),
                ("CacheNodeType", NODE_TYPE),
                ("NumCacheNodes", "1"),
            ]),
            refused: "DeleteCacheCluster",
            refused_request: params(&[("CacheClusterId", "gate-del-cluster")]),
            code: "InvalidCacheClusterState",
            message: None,
        },
        Gate {
            case: "a snapshot of a cluster that is still being created",
            start: "CreateCacheCluster",
            start_request: params(&[
                ("CacheClusterId", "gate-snap-cluster"),
                ("Engine", "redis"),
                ("CacheNodeType", NODE_TYPE),
                ("NumCacheNodes", "1"),
            ]),
            refused: "CreateSnapshot",
            refused_request: params(&[
                ("SnapshotName", "gate-snap"),
                ("CacheClusterId", "gate-snap-cluster"),
            ]),
            code: "InvalidCacheClusterState",
            // AWS states the requirement a backup has of its source cluster, naming no
            // value of the request.
            message: Some("Cluster must be in available state in order to create a snapshot.".to_string()),
        },
        Gate {
            case: "a modify of a group that is still being created",
            start: "CreateReplicationGroup",
            start_request: params(&[
                ("ReplicationGroupId", "gate-mod-group"),
                ("ReplicationGroupDescription", "the description this modify would replace"),
                ("Engine", "redis"),
                ("CacheNodeType", NODE_TYPE),
            ]),
            refused: "ModifyReplicationGroup",
            refused_request: params(&[
                ("ReplicationGroupId", "gate-mod-group"),
                ("ReplicationGroupDescription", "nope"),
                ("ApplyImmediately", "true"),
            ]),
            code: "InvalidReplicationGroupState",
            // AWS states the state a modify requires and names neither the group nor the
            // status it carries.
            message: Some("Replication group must be in available state to modify.".to_string()),
        },
        Gate {
            case: "a replica-count increase on a group that is still being created",
            start: "CreateReplicationGroup",
            start_request: params(&[
                ("ReplicationGroupId", "gate-scale-group"),
                ("ReplicationGroupDescription", "a group that is not ready to scale"),
                ("Engine", "redis"),
                ("CacheNodeType", NODE_TYPE),
                ("ReplicasPerNodeGroup", "0"),
            ]),
            refused: "IncreaseReplicaCount",
            refused_request: params(&[
                ("ReplicationGroupId", "gate-scale-group"),
                ("NewReplicaCount", "1"),
                ("ApplyImmediately", "true"),
            ]),
            code: "InvalidReplicationGroupState",
            message: Some("Replication group must be in available state to modify.".to_string()),
        },
        Gate {
            case: "a delete of a group that is still being created",
            start: "CreateReplicationGroup",
            start_request: params(&[
                ("ReplicationGroupId", "gate-del-group"),
                ("ReplicationGroupDescription", "a group deleted too early"),
                ("Engine", "redis"),
                ("CacheNodeType", NODE_TYPE),
            ]),
            refused: "DeleteReplicationGroup",
            refused_request: params(&[("ReplicationGroupId", "gate-del-group")]),
            code: "InvalidReplicationGroupState",
            // A delete takes the group's member clusters with it, so the member that is
            // still being provisioned is the resource the refusal names.
            message: Some(
                "Cache cluster gate-del-group-001 is not in a valid state to be deleted."
                    .to_string(),
            ),
        },
        Gate {
            case: "a membership change to a user group that is still being created",
            start: "CreateUserGroup",
            start_request: params(&[
                ("UserGroupId", "gate-rbac"),
                ("Engine", "redis"),
                ("UserIds.member.1", "default"),
            ]),
            refused: "ModifyUserGroup",
            refused_request: params(&[
                ("UserGroupId", "gate-rbac"),
                ("UserIdsToRemove.member.1", "default"),
            ]),
            code: "InvalidUserGroupState",
            // The refusal names the group the request addressed and the status it carries,
            // then the status a change admits.
            message: Some(format!(
                "User group gate-rbac has status {}. Only {} user groups can be modified.",
                state::STATUS_CREATING,
                state::USER_STATUS_ACTIVE
            )),
        },
        Gate {
            case: "a delete of a user group that is still being created",
            start: "CreateUserGroup",
            start_request: params(&[
                ("UserGroupId", "gate-rbac-del"),
                ("Engine", "redis"),
                ("UserIds.member.1", "default"),
            ]),
            refused: "DeleteUserGroup",
            refused_request: params(&[("UserGroupId", "gate-rbac-del")]),
            code: "InvalidUserGroupState",
            message: None,
        },
    ];

    for row in &rows {
        let started = emulator.call(row.start, &row.start_request).await;
        assert_eq!(
            started.status, 200,
            "case {:?}: {} must answer 200, got {} {}",
            row.case, row.start, started.status, started.body
        );
        let refused = emulator.call(row.refused, &row.refused_request).await;
        match &row.message {
            Some(message) => assert_refusal(row.case, &refused, 400, row.code, message),
            None => assert_eq!(
                (refused.status, refused.error_code()),
                (400, row.code),
                "case {:?}: {} must be refused 400 {}, got {} {}",
                row.case,
                row.refused,
                row.code,
                refused.status,
                refused.body
            ),
        }
    }
}

#[tokio::test]
async fn one_of_two_replica_count_changes_issued_back_to_back_is_refused() {
    let emulator = Emulator::new();
    available_group(
        &emulator,
        "contending replica-count calls",
        "contend-group",
        &[("ReplicasPerNodeGroup", "1"), ("NumNodeGroups", "1")],
    )
    .await;

    // The accepted change takes the group out of `available` for the span the clock owes
    // it, so the call that contends with it reads the group busy and is refused: a client
    // driving a controlled scale cannot have a competing target land behind its back.
    let increased = emulator
        .call(
            "IncreaseReplicaCount",
            &params(&[
                ("ReplicationGroupId", "contend-group"),
                ("NewReplicaCount", "2"),
                ("ApplyImmediately", "true"),
            ]),
        )
        .await;
    assert_eq!(
        (increased.status, increased.text("ReplicationGroup/Status")),
        (200, Some(state::STATUS_MODIFYING)),
        "the first replica-count call must be accepted and report the group modifying, got {} {}",
        increased.status,
        increased.body
    );
    let decreased = emulator
        .call(
            "DecreaseReplicaCount",
            &params(&[
                ("ReplicationGroupId", "contend-group"),
                ("NewReplicaCount", "0"),
                ("ApplyImmediately", "true"),
            ]),
        )
        .await;
    assert_refusal(
        "a replica-count decrease contending with an accepted increase",
        &decreased,
        400,
        "InvalidReplicationGroupState",
        "Replication group must be in available state to modify.",
    );

    // The count the group converges on is the accepted call's, not the refused one's.
    let settled = poll_until(
        &emulator,
        REGION,
        "contending replica-count calls",
        "the group reaching available again",
        "DescribeReplicationGroups",
        &params(&[("ReplicationGroupId", "contend-group")]),
        |reply| reply.text("ReplicationGroup/Status") == Some(state::STATUS_AVAILABLE),
    )
    .await;
    assert_eq!(
        settled.all("MemberClusters/ClusterId").len(),
        3,
        "the group must converge on the accepted increase's three members, got {}",
        settled.body
    );
}

#[tokio::test]
async fn a_migration_in_flight_closes_the_gates_that_follow_it() {
    let emulator = Emulator::new();
    available_group(
        &emulator,
        "a migration in flight",
        "migrating-group",
        &[
            ("ReplicasPerNodeGroup", "1"),
            ("NumNodeGroups", "1"),
            // A migration is only ever carried into a group that can promote a
            // replica in its primary's place.
            ("AutomaticFailoverEnabled", "true"),
        ],
    )
    .await;
    let start_request = params(&[
        ("ReplicationGroupId", "migrating-group"),
        ("CustomerNodeEndpointList.member.1.Address", "203.0.113.10"),
        ("CustomerNodeEndpointList.member.1.Port", "6379"),
    ]);
    let started = emulator.call("StartMigration", &start_request).await;
    assert_eq!(
        (started.status, started.text("ReplicationGroup/Status")),
        (200, Some(state::STATUS_MODIFYING)),
        "StartMigration must be accepted and take the group out of available, got {} {}",
        started.status,
        started.body
    );

    // A second start lands while the first is still applying: the group's STATE is what
    // refuses it, worded for the statuses a start admits.
    let again = emulator.call("StartMigration", &start_request).await;
    assert_refusal(
        "a second StartMigration while the first applies",
        &again,
        400,
        "InvalidReplicationGroupState",
        state::MIGRATION_START_STATE_MESSAGE,
    );

    // And a teardown issued during the migration is refused too, naming the status the
    // group carries.
    let deleted = emulator
        .call(
            "DeleteReplicationGroup",
            &params(&[("ReplicationGroupId", "migrating-group")]),
        )
        .await;
    assert_refusal(
        "a delete issued during a live migration",
        &deleted,
        400,
        "InvalidReplicationGroupState",
        &format!(
            "Replication group migrating-group has status {} which is not valid for deletion.",
            state::STATUS_MODIFYING
        ),
    );
    // Nothing of the group went: it is still there, and still migrating.
    let described = emulator
        .call(
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", "migrating-group")]),
        )
        .await;
    assert_eq!(
        (described.status, described.text("ReplicationGroup/Status")),
        (200, Some(state::STATUS_MODIFYING)),
        "the refused delete must leave the group where it was, got {} {}",
        described.status,
        described.body
    );
}

#[tokio::test]
async fn a_delete_is_refused_while_a_member_cluster_is_still_being_snapshotted() {
    let emulator = Emulator::new();
    available_group(
        &emulator,
        "a backup in flight",
        "backed-group",
        &[("ReplicasPerNodeGroup", "0"), ("NumNodeGroups", "1")],
    )
    .await;
    // The member cluster ids are the ones ElastiCache derives from the group id.
    let member = "backed-group-001";
    let snapshot = emulator
        .call(
            "CreateSnapshot",
            &params(&[
                ("SnapshotName", "backed-snap"),
                ("CacheClusterId", member),
            ]),
        )
        .await;
    assert_eq!(
        snapshot.status, 200,
        "the snapshot of {member} must be accepted, got {} {}",
        snapshot.status, snapshot.body
    );

    let refused = emulator
        .call(
            "DeleteReplicationGroup",
            &params(&[("ReplicationGroupId", "backed-group")]),
        )
        .await;
    assert_refusal(
        "a delete racing a backup of the group's member",
        &refused,
        400,
        "InvalidReplicationGroupState",
        &format!("Cache cluster {member} is not in a valid state to be deleted."),
    );

    // Once the backup is durable the same delete is accepted, which is the property the
    // refusal exists for.
    poll_until(
        &emulator,
        REGION,
        "a backup in flight",
        "the snapshot reaching available",
        "DescribeSnapshots",
        &params(&[("SnapshotName", "backed-snap")]),
        |reply| reply.text("Snapshot/SnapshotStatus") == Some(state::STATUS_AVAILABLE),
    )
    .await;
    let deleted = emulator
        .call(
            "DeleteReplicationGroup",
            &params(&[("ReplicationGroupId", "backed-group")]),
        )
        .await;
    assert_eq!(
        (deleted.status, deleted.text("ReplicationGroup/Status")),
        (200, Some(state::STATUS_DELETING)),
        "the delete must be accepted once the backup is durable, got {} {}",
        deleted.status,
        deleted.body
    );
}
