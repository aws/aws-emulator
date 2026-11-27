// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rule that an online migration needs a replication group that can
//! fail over.
//!
//! The rule under test: migrating live data into a replication group replaces what its
//! primary holds, and ElastiCache only carries that out on a group that can promote a
//! replica in the primary's place — so a group whose automatic failover is off hosts no
//! migration at all, and both operations that touch one (`StartMigration`, `TestMigration`)
//! refuse such a group with `InvalidParameterValue` (HTTP 400) and leave it untouched.
//!
//! The tables span what the rule generalises over: how the group came to have failover off
//! (the default of an unpartitioned group, or the setting named outright), how many clusters
//! and shards it holds, and which of the two operations is called — and, as the positive
//! control, the same calls against groups whose failover is on, reached both by naming the
//! setting and by partitioning the keyspace.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, and each refused group is read back through `DescribeReplicationGroups`.

use service_com_amazonaws_elasticache_amazonelasticachev9::service::replication_group_not_found;
use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, poll_until, Emulator, Reply, REGION};

/// The node type every group here runs; this suite is not about node types.
const NODE_TYPE: &str = "cache.m7g.large";

/// The two operations that touch an online migration of a replication group, with the source
/// endpoint list each takes.
fn migration_request(id: &str) -> Vec<(String, String)> {
    params(&[
        ("ReplicationGroupId", id),
        ("CustomerNodeEndpointList.member.1.Address", "203.0.113.7"),
        ("CustomerNodeEndpointList.member.1.Port", "6379"),
    ])
}

/// Create one replication group and look again until it reports `available`.
async fn settled_group(emulator: &Emulator, case: &str, id: &str, extra: &[(&str, &str)]) {
    let mut pairs = params(&[
        ("ReplicationGroupId", id),
        ("ReplicationGroupDescription", "a migration precondition row"),
        ("Engine", state::DEFAULT_ENGINE),
        ("CacheNodeType", NODE_TYPE),
    ]);
    pairs.extend(params(extra));
    let created = emulator.call("CreateReplicationGroup", &pairs).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the group must be created, got {} {}",
        created.status, created.body
    );
    poll_until(
        emulator,
        REGION,
        case,
        &format!("the replication group {id} settling at available"),
        "DescribeReplicationGroups",
        &params(&[("ReplicationGroupId", id)]),
        |reply| reply.text("ReplicationGroup/Status") == Some(state::STATUS_AVAILABLE),
    )
    .await;
}

/// `DescribeReplicationGroups` for one group.
async fn describe_group(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .call(
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", id)]),
        )
        .await
}

/// One row: the case name, the members the group is created with, and whether that group's
/// automatic failover ends up on.
type Topology = (
    &'static str,
    &'static [(&'static str, &'static str)],
    bool,
);

/// The topology table.
const TOPOLOGIES: &[Topology] = &[
    (
        "an unpartitioned group of one cluster, failover off by default",
        &[("NumCacheClusters", "1")],
        false,
    ),
    (
        "an unpartitioned group of two clusters with failover named off",
        &[("NumCacheClusters", "2"), ("AutomaticFailoverEnabled", "false")],
        false,
    ),
    (
        "a single-shard group with a replica and failover off",
        &[("NumNodeGroups", "1"), ("ReplicasPerNodeGroup", "1")],
        false,
    ),
    (
        "an unpartitioned group of two clusters with failover named on",
        &[("NumCacheClusters", "2"), ("AutomaticFailoverEnabled", "true")],
        true,
    ),
    (
        "a single-shard group with a replica and failover named on",
        &[
            ("NumNodeGroups", "1"),
            ("ReplicasPerNodeGroup", "1"),
            ("AutomaticFailoverEnabled", "true"),
        ],
        true,
    ),
    (
        "a partitioned group, whose mode carries failover with it",
        &[("NumNodeGroups", "2"), ("ReplicasPerNodeGroup", "1")],
        true,
    ),
];

#[tokio::test]
async fn a_migration_is_refused_on_a_group_that_cannot_fail_over() {
    let emulator = Emulator::new();
    for (index, (case, extra, failover_on)) in TOPOLOGIES.iter().enumerate() {
        // One group per row, and one per OPERATION: a start that is served changes the
        // group, so the two operations are asked of groups of their own.
        for (operation, suffix) in [("StartMigration", "start"), ("TestMigration", "test")] {
            let id = format!("failover-{suffix}-{index}");
            settled_group(&emulator, case, &id, extra).await;
            let before = describe_group(&emulator, &id).await;
            assert_eq!(
                before.text("ReplicationGroup/AutomaticFailover"),
                Some(if *failover_on {
                    state::SETTING_ENABLED
                } else {
                    state::SETTING_DISABLED
                }),
                "case {case:?}: the row's own topology must report its failover setting, got {}",
                before.body
            );

            let reply = emulator.call(operation, &migration_request(&id)).await;
            if *failover_on {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?} through {operation}: a group that can fail over takes the \
                     call, got {} {}",
                    reply.status, reply.body
                );
                continue;
            }
            assert_eq!(
                (reply.status, reply.error_code()),
                (400, "InvalidParameterValue"),
                "case {case:?} through {operation}: expected 400 InvalidParameterValue, got {} {}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.error_message(),
                state::MIGRATION_NEEDS_AUTO_FAILOVER_MESSAGE,
                "case {case:?} through {operation}: expected the message {:?}, got {:?}",
                state::MIGRATION_NEEDS_AUTO_FAILOVER_MESSAGE,
                reply.error_message()
            );
            // The refused call left the group exactly as it was: still available, with no
            // migration status of its own and the same configuration.
            let after = describe_group(&emulator, &id).await;
            assert_eq!(
                after.text("ReplicationGroup/Status"),
                Some(state::STATUS_AVAILABLE),
                "case {case:?} through {operation}: the group must still be available, got {}",
                after.body
            );
            assert_eq!(
                after.all("ReplicationGroup/MemberClusters/ClusterId"),
                before.all("ReplicationGroup/MemberClusters/ClusterId"),
                "case {case:?} through {operation}: the group's members must be unchanged, got {}",
                after.body
            );
        }
    }
}

#[tokio::test]
async fn a_group_that_does_not_exist_is_answered_before_the_precondition() {
    // The precondition is about the group the request RESOLVES, so a request that resolves
    // none is still the modelled not-found — stated in the words each operation words it
    // with, read off the one place those templates live.
    let emulator = Emulator::new();
    let absent = "never-created-group";
    for (operation, request, expected) in [
        (
            "StartMigration",
            migration_request(absent),
            replication_group_not_found::quoted_sentence(absent),
        ),
        (
            "TestMigration",
            migration_request(absent),
            replication_group_not_found::quoted_sentence(absent),
        ),
        (
            // The cutover takes no source list: the migration it finishes is the one the
            // group already carries.
            "CompleteMigration",
            params(&[("ReplicationGroupId", absent)]),
            replication_group_not_found::quoted_message(absent),
        ),
    ] {
        let reply = emulator.call(operation, &request).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (404, "ReplicationGroupNotFoundFault"),
            "case {operation}: expected 404 ReplicationGroupNotFoundFault, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.error_message(),
            expected,
            "case {operation}: expected the message {expected:?}, got {:?}",
            reply.error_message()
        );
    }
}
