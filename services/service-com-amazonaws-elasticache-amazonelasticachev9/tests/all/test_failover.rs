// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `TestFailover`.
//!
//! The rule under test: `TestFailover` moves the primary role of the ONE shard
//! the request names to a replica of that shard, demoting the primary it had,
//! and leaves the replication group in the transient status the node
//! replacement runs in until the clock settles it — whatever the group's shape
//! is. It is refused with the modelled fault for the thing that is wrong: the
//! group, the shard, the group's state, or there being no automatic failover to
//! test.
//!
//! Every assertion goes over the wire through the same router the production
//! emulator mounts, using the harness in [`crate::harness`]; nothing reaches
//! into the service's state struct, so a response that is not computed from
//! persisted state cannot pass.
//!
//! The tests are tables: each row is one input along the axis the rule
//! generalises over (how the group is partitioned, how many clusters a shard
//! holds, which shard is named, which part of the request is wrong), and the
//! recorded two-node case is one row.

use crate::harness::{params, Emulator, Reply, PAST_TRANSITION};

/// Create one replication group and assert the create itself was accepted.
async fn create_group(emulator: &Emulator, case: &str, id: &str, extra: &[(&str, &str)]) {
    let mut pairs = params(&[
        ("ReplicationGroupId", id),
        ("ReplicationGroupDescription", "the TestFailover suite"),
    ]);
    pairs.extend(params(extra));
    let created = emulator.call("CreateReplicationGroup", &pairs).await;
    assert_eq!(
        created.status, 200,
        "case {case}: CreateReplicationGroup must answer 200, got {} {}",
        created.status, created.body
    );
}

/// Call `TestFailover` on one shard of one group.
async fn test_failover(emulator: &Emulator, id: &str, node_group_id: &str) -> Reply {
    emulator
        .call(
            "TestFailover",
            &params(&[
                ("ReplicationGroupId", id),
                ("NodeGroupId", node_group_id),
            ]),
        )
        .await
}

/// Read one replication group back, asserting the describe itself succeeded.
async fn describe_group(emulator: &Emulator, case: &str, id: &str) -> Reply {
    let described = emulator
        .call(
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", id)]),
        )
        .await;
    assert_eq!(
        described.status, 200,
        "case {case}: DescribeReplicationGroups must answer 200, got {} {}",
        described.status, described.body
    );
    described
}

/// The member cluster ids of the shard `node_group_id` of the group `id`, in
/// the order the answer reports them.
///
/// The wire answer carries no index per shard, so the shard a member belongs to
/// is read off the id ElastiCache derives for it — `<group>-<shard>-<n>` for a
/// partitioned group, `<group>-<n>` for one that is not — in document order,
/// which is shard order and then member order.
fn shard_members<'a>(described: &'a Reply, id: &str, node_group_id: &str, sharded: bool) -> Vec<&'a str> {
    let prefix = if sharded {
        format!("{id}-{node_group_id}-")
    } else {
        format!("{id}-")
    };
    described
        .all("NodeGroupMember/CacheClusterId")
        .into_iter()
        .filter(|member| member.starts_with(&prefix))
        .collect()
}

/// One row of the promotion table.
#[derive(Debug, Clone)]
struct FailoverCase {
    /// The group id, which is also the case name.
    id: &'static str,
    /// The form pairs sent beside the id and the description.
    extra: &'static [(&'static str, &'static str)],
    /// Whether the keyspace is partitioned across shards, which is what decides
    /// both the member id form and whether `CurrentRole` is reported at all.
    sharded: bool,
    /// The shard `TestFailover` is called on.
    node_group_id: &'static str,
    /// That shard's member clusters before the call, the primary first.
    members_before: &'static [&'static str],
    /// A shard of the same group that is NOT failed over, with its members, or
    /// `None` for a group that has only one shard.
    untouched_shard: Option<(&'static str, &'static [&'static str])>,
}

#[tokio::test]
async fn test_failover_moves_the_primary_role_to_a_replica_of_the_named_shard() {
    // The axis: how the group is partitioned (`NumNodeGroups` / `ClusterMode`),
    // how many clusters the shard holds (`NumCacheClusters` /
    // `ReplicasPerNodeGroup`), and WHICH shard is named — the recorded
    // two-cluster, one-shard case is the first row.
    let cases = [
        FailoverCase {
            id: "flat-two",
            extra: &[
                ("NumCacheClusters", "2"),
                ("AutomaticFailoverEnabled", "true"),
                ("MultiAZEnabled", "true"),
            ],
            sharded: false,
            node_group_id: "0001",
            members_before: &["flat-two-001", "flat-two-002"],
            untouched_shard: None,
        },
        FailoverCase {
            id: "flat-three",
            extra: &[
                ("NumCacheClusters", "3"),
                ("AutomaticFailoverEnabled", "true"),
            ],
            sharded: false,
            node_group_id: "0001",
            members_before: &["flat-three-001", "flat-three-002", "flat-three-003"],
            untouched_shard: None,
        },
        FailoverCase {
            id: "sharded-first",
            // "AutomaticFailoverEnabled must be enabled for Valkey or Redis OSS
            // (cluster mode enabled) replication groups", so a partitioned group
            // with a replica per shard already has the failover this operation
            // tests.
            extra: &[("NumNodeGroups", "2"), ("ReplicasPerNodeGroup", "1")],
            sharded: true,
            node_group_id: "0001",
            members_before: &["sharded-first-0001-001", "sharded-first-0001-002"],
            untouched_shard: Some((
                "0002",
                &["sharded-first-0002-001", "sharded-first-0002-002"],
            )),
        },
        FailoverCase {
            id: "sharded-second",
            extra: &[("NumNodeGroups", "2"), ("ReplicasPerNodeGroup", "2")],
            sharded: true,
            // The SECOND shard, so the promotion is not keyed to the first one.
            node_group_id: "0002",
            members_before: &[
                "sharded-second-0002-001",
                "sharded-second-0002-002",
                "sharded-second-0002-003",
            ],
            untouched_shard: Some((
                "0001",
                &[
                    "sharded-second-0001-001",
                    "sharded-second-0001-002",
                    "sharded-second-0001-003",
                ],
            )),
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        create_group(&emulator, case.id, case.id, case.extra).await;

        // Before: the first member of the shard is its primary. This read is
        // answered once the create has settled the group, which is what lets
        // the failover below be accepted.
        let described = describe_group(&emulator, case.id, case.id).await;
        assert_eq!(
            shard_members(&described, case.id, case.node_group_id, case.sharded),
            case.members_before.to_vec(),
            "case {case:?}: the shard's members must be the stored ones before the failover"
        );

        let answered = test_failover(&emulator, case.id, case.node_group_id).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: TestFailover must answer 200, got {} {}",
            answered.status, answered.body
        );

        // The node replacement is under way, so the group is not available — the
        // call that started it answers with the work in progress.
        assert_eq!(
            answered.text("ReplicationGroup/Status"),
            Some("modifying"),
            "case {case:?}: the group must report the status the failover runs in, got {:?}",
            answered.text("ReplicationGroup/Status")
        );

        // The promotion: the shard's first replica now holds the primary role
        // and the primary it had is a replica. `TestFailover`'s own answer and a
        // later describe must agree, because both are built from the one stored
        // group; the describe is answered once the failover the clock owed is
        // over.
        let promoted = case.members_before[1];
        let demoted = case.members_before[0];
        let mut expected_after = vec![promoted, demoted];
        expected_after.extend(case.members_before.iter().skip(2).copied());
        let settled = describe_group(&emulator, case.id, case.id).await;
        assert_eq!(
            settled.text("ReplicationGroup/Status"),
            Some("available"),
            "case {case:?}: the read after the failover is answered once it is over, got {:?}",
            settled.text("ReplicationGroup/Status")
        );
        for (source, reply) in [("the answer", &answered), ("the describe", &settled)] {
            assert_eq!(
                shard_members(reply, case.id, case.node_group_id, case.sharded),
                expected_after,
                "case {case:?}: {source} must report {promoted} as the shard's primary, with \
                 {demoted} demoted"
            );
        }

        // `CurrentRole` is "only applicable for Valkey or Redis OSS (cluster
        // mode disabled)" groups, so the roles themselves are reported only for
        // an unpartitioned one; a partitioned group's promotion shows in the
        // member order above, which is the primary-first order this service
        // stores a shard's members in.
        let roles = settled.all("NodeGroupMember/CurrentRole");
        if case.sharded {
            assert!(
                roles.is_empty(),
                "case {case:?}: a cluster mode enabled group reports no CurrentRole, got {roles:?}"
            );
        } else {
            let mut expected_roles = vec!["primary"];
            expected_roles.extend(case.members_before.iter().skip(1).map(|_| "replica"));
            assert_eq!(
                roles, expected_roles,
                "case {case:?}: exactly the promoted member holds the primary role"
            );
        }

        // The failover outlives the transition: the group settles back to
        // available with the promotion still in place, rather than reverting.
        assert_eq!(
            settled.text("ReplicationGroup/Status"),
            Some("available"),
            "case {case:?}: the group must settle back to available, got {:?}",
            settled.text("ReplicationGroup/Status")
        );

        // The shards the request did not name are untouched: one call fails one
        // shard over.
        if let Some((other, members)) = case.untouched_shard {
            assert_eq!(
                shard_members(&settled, case.id, other, case.sharded),
                members.to_vec(),
                "case {case:?}: shard {other} must be left as it was"
            );
        }

        // The first of the events this operation documents, "listed here in
        // order of occurrance: Replication group message: Test Failover API
        // called for node group <id>".
        let events = emulator
            .call(
                "DescribeEvents",
                &params(&[("SourceType", "replication-group")]),
            )
            .await;
        assert_eq!(
            events.status, 200,
            "case {case:?}: DescribeEvents must answer 200, got {}",
            events.body
        );
        let called = format!("Test Failover API called for node group {}", case.node_group_id);
        assert!(
            events
                .all("Event/Message")
                .iter()
                .any(|message| message.starts_with(&called)),
            "case {case:?}: the call must record {called:?}, got {:?}",
            events.all("Event/Message")
        );
    }
}

/// One row of the shard-not-found table: the case name, the form pairs the
/// group is created with, and the `NodeGroupId` that names no shard of it.
type MissingShardCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static str,
);

#[tokio::test]
async fn a_node_group_the_group_does_not_have_is_refused_as_not_found() {
    // The axis: the group's shard count, and the FORM of the id that names no
    // shard of it — a shard beyond the last one, a far higher one, and the
    // unpadded spelling of a shard the group does have (`NodeGroupId` is "the
    // identifier for the node group (shard)", and a shard's identifier is the
    // 4-digit one the group reports).
    let cases: [MissingShardCase; 6] = [
        (
            "one shard, the second asked for",
            &[("NumCacheClusters", "2"), ("AutomaticFailoverEnabled", "true")],
            "0002",
        ),
        (
            "one shard, a far higher one asked for",
            &[("NumCacheClusters", "2"), ("AutomaticFailoverEnabled", "true")],
            "9999",
        ),
        (
            "one shard, asked for unpadded",
            &[("NumCacheClusters", "2"), ("AutomaticFailoverEnabled", "true")],
            "1",
        ),
        (
            "three shards, the fourth asked for",
            &[("NumNodeGroups", "3"), ("ReplicasPerNodeGroup", "1")],
            "0004",
        ),
        (
            "three shards, a single digit asked for",
            &[("NumNodeGroups", "3"), ("ReplicasPerNodeGroup", "1")],
            "2",
        ),
        (
            "three shards, zero asked for",
            &[("NumNodeGroups", "3"), ("ReplicasPerNodeGroup", "1")],
            "0000",
        ),
    ];

    for (index, (case, extra, node_group_id)) in cases.into_iter().enumerate() {
        let emulator = Emulator::new();
        let id = format!("missing-shard-{index}");
        create_group(&emulator, case, &id, extra).await;
        std::thread::sleep(PAST_TRANSITION);

        let refused = test_failover(&emulator, &id, node_group_id).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (404, "NodeGroupNotFoundFault"),
            "case {case}: expected 404 NodeGroupNotFoundFault, got {} {} {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        assert!(
            refused.error_message().contains(node_group_id),
            "case {case}: the refusal must name the node group asked for, got {:?}",
            refused.error_message()
        );

        // Nothing was failed over: the group is still available with the
        // primary it had.
        let described = describe_group(&emulator, case, &id).await;
        assert_eq!(
            described.text("ReplicationGroup/Status"),
            Some("available"),
            "case {case}: a refused call must leave the group as it was, got {:?}",
            described.text("ReplicationGroup/Status")
        );
    }
}

/// Assert one `TestFailover` call is refused with `status` and `code`.
async fn assert_refused(
    emulator: &Emulator,
    case: &str,
    id: &str,
    node_group_id: &str,
    status: u16,
    code: &str,
) {
    let refused = test_failover(emulator, id, node_group_id).await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (status, code),
        "case {case}: expected {status} {code}, got {} {} {}",
        refused.status,
        refused.error_code(),
        refused.body
    );
}

#[tokio::test]
async fn a_call_this_service_cannot_satisfy_is_refused_with_its_modelled_error() {
    let emulator = Emulator::new();

    // A group this account does not hold is not found; a value no replication
    // group could ever be called is a bad parameter value instead.
    assert_refused(
        &emulator,
        "no such replication group",
        "absent-group",
        "0001",
        404,
        "ReplicationGroupNotFoundFault",
    )
    .await;
    assert_refused(
        &emulator,
        "a group id no group can carry",
        "9-not-an-id",
        "0001",
        400,
        "InvalidParameterValue",
    )
    .await;

    // "The requested replication group is not in the available state": the work
    // the create started has to finish first.
    create_group(
        &emulator,
        "group still creating",
        "still-creating",
        &[("NumCacheClusters", "2"), ("AutomaticFailoverEnabled", "true")],
    )
    .await;
    assert_refused(
        &emulator,
        "group still creating",
        "still-creating",
        "0001",
        400,
        "InvalidReplicationGroupState",
    )
    .await;

    // "If calling this operation multiple times on different shards in the same
    // … replication group, the first node replacement must complete before a
    // subsequent call can be made": the group a failover is running on is not
    // available either.
    create_group(
        &emulator,
        "a failover already running",
        "already-failing",
        &[("NumNodeGroups", "2"), ("ReplicasPerNodeGroup", "1")],
    )
    .await;
    std::thread::sleep(PAST_TRANSITION);
    let accepted = test_failover(&emulator, "already-failing", "0001").await;
    assert_eq!(
        accepted.status, 200,
        "the first TestFailover must answer 200, got {} {}",
        accepted.status, accepted.body
    );
    assert_refused(
        &emulator,
        "a failover already running",
        "already-failing",
        "0002",
        400,
        "InvalidReplicationGroupState",
    )
    .await;

    // "Tests automatic failover on a specified node group": a group whose
    // `AutomaticFailover` is disabled has none to test, however many replicas
    // its shard holds.
    create_group(
        &emulator,
        "automatic failover disabled",
        "no-auto-failover",
        &[("NumCacheClusters", "3")],
    )
    .await;
    std::thread::sleep(PAST_TRANSITION);
    let described = describe_group(&emulator, "automatic failover disabled", "no-auto-failover").await;
    assert_eq!(
        described.text("ReplicationGroup/AutomaticFailover"),
        Some("disabled"),
        "the row must be set up on a group with automatic failover off, got {:?}",
        described.text("ReplicationGroup/AutomaticFailover")
    );
    assert_refused(
        &emulator,
        "automatic failover disabled",
        "no-auto-failover",
        "0001",
        400,
        "TestFailoverNotAvailableFault",
    )
    .await;

    // A shard with no replica has nothing for the primary role to move to. A
    // partitioned group may be scaled down to none ("Valkey or Redis OSS
    // (cluster mode enabled): 0"), which is how this row reaches a shard with
    // automatic failover on and no replica.
    create_group(
        &emulator,
        "no replica to promote",
        "no-replica",
        &[("NumNodeGroups", "2"), ("ReplicasPerNodeGroup", "1")],
    )
    .await;
    std::thread::sleep(PAST_TRANSITION);
    let decreased = emulator
        .call(
            "DecreaseReplicaCount",
            &params(&[
                ("ReplicationGroupId", "no-replica"),
                ("ApplyImmediately", "true"),
                ("NewReplicaCount", "0"),
            ]),
        )
        .await;
    assert_eq!(
        decreased.status, 200,
        "DecreaseReplicaCount must answer 200, got {} {}",
        decreased.status, decreased.body
    );
    std::thread::sleep(PAST_TRANSITION);
    assert_refused(
        &emulator,
        "no replica to promote",
        "no-replica",
        "0001",
        400,
        "TestFailoverNotAvailableFault",
    )
    .await;
}

#[tokio::test]
async fn the_rolling_window_bounds_how_many_shards_may_be_tested() {
    // "A customer can use this operation to test automatic failover on up to 15
    // shards … in any rolling 24-hour period." The bound is checked AT it and
    // one past it: the 15th call is accepted and the 16th is refused.
    let emulator = Emulator::new();
    let bound =
        service_com_amazonaws_elasticache_amazonelasticachev9::state::MAX_TEST_FAILOVERS_PER_WINDOW;
    // The VALUE the operation documents, not just whatever the constant says: a
    // bound set to anything else is a defect this row has to fail on.
    assert_eq!(
        bound, 15,
        "TestFailover documents a bound of 15 shards per rolling 24-hour period, got {bound}"
    );
    let ids: Vec<String> = (0..=bound).map(|index| format!("rolling-{index}")).collect();
    for id in &ids {
        create_group(
            &emulator,
            id,
            id,
            &[("NumCacheClusters", "2"), ("AutomaticFailoverEnabled", "true")],
        )
        .await;
    }
    std::thread::sleep(PAST_TRANSITION);

    // One shard per group, so the state gate never stands in for the rate
    // bound: each call is on a group nothing else is happening to.
    for (index, id) in ids.iter().enumerate() {
        let answered = test_failover(&emulator, id, "0001").await;
        if index < bound {
            assert_eq!(
                answered.status, 200,
                "call {} of {bound} must be accepted, got {} {}",
                index + 1,
                answered.status,
                answered.body
            );
        } else {
            assert_eq!(
                (answered.status, answered.error_code()),
                (400, "APICallRateForCustomerExceeded"),
                "call {} is past the bound of {bound} and must be refused, got {} {} {}",
                index + 1,
                answered.status,
                answered.error_code(),
                answered.body
            );
            // Refused, so nothing moved: the group keeps the primary it had.
            let described = describe_group(&emulator, id, id).await;
            assert_eq!(
                described.all("NodeGroupMember/CurrentRole").first(),
                Some(&"primary"),
                "the refused group's first member must still be its primary, got {:?}",
                described.all("NodeGroupMember/CurrentRole")
            );
            let first_member = format!("{id}-001");
            assert_eq!(
                described.all("NodeGroupMember/CacheClusterId").first(),
                Some(&first_member.as_str()),
                "the refused group must keep the primary it had, got {:?}",
                described.all("NodeGroupMember/CacheClusterId")
            );
        }
    }
}
