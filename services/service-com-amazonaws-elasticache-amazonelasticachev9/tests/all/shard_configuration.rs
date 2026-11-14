// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ModifyReplicationGroupShardConfiguration` and for the replica
//! count a cluster mode enabled `CreateReplicationGroup` DEFAULTS to.
//!
//! The rules under test:
//!
//! * an optional member the request omits takes the service's default for the topology the
//!   request asked for, and that default is applied BEFORE the combination checks — so a
//!   cluster mode enabled create that asks for automatic failover without naming
//!   `ReplicasPerNodeGroup` is served with the replica every shard needs to promote,
//!   rather than refused for a member it never sent;
//! * "Modifies a replication group's shards (node groups) by allowing you to add shards,
//!   remove shards, or rebalance the keyspaces among existing shards" — the reshard writes
//!   the shards of the ONE stored group, the keyspace is partitioned across the shards the
//!   group ends up with exactly once, and a shard's member CLUSTERS come and go with it,
//!   so `DescribeCacheClusters` and `DescribeReplicationGroups` cannot disagree about what
//!   the account runs;
//! * resharding is work the service carries out after the call, so the group is transient
//!   until the clock owes it `available` — which is what makes two concurrent reshards
//!   resolve to exactly one winner;
//! * every member of the request is held to the rule its own documentation states, and a
//!   refused reshard changes nothing.
//!
//! The tests are tables: the rows span the direction of the change (out, in), the counts
//! on both sides, which side a scale-in selects from (`NodeGroupsToRemove` /
//! `NodeGroupsToRetain`), whether the request places the new shards, and each way a
//! request can be wrong, so no single recorded request is what makes them pass.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, with the shared `harness` request builder and XML reader; nothing reaches into
//! the service's state struct.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, Reply, PAST_TRANSITION};

/// One row of the failover-default table: the case, the group id, the pairs beside the
/// required ones, and how many shards the group must end up with.
type FailoverCase = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    usize,
);

/// One row of the reshard table: the case, the shard count the group is created with, the
/// reshard's own form pairs, and the shard count it must end up with.
type ReshardCase = (&'static str, i32, Vec<(String, String)>, usize);

/// One row of the refusal table: the case, the group the request names, its form pairs, and
/// the status plus wire code it must be refused with.
type ReshardRefusal = (
    &'static str,
    &'static str,
    Vec<(String, String)>,
    u16,
    &'static str,
);

/// Create one replication group with the pairs a row sends and let the clock settle it at
/// `available`, which is the status every change to a group is gated on.
async fn create_settled_group(emulator: &Emulator, case: &str, id: &str, extra: &[(&str, &str)]) {
    let mut pairs = params(&[
        ("ReplicationGroupId", id),
        ("ReplicationGroupDescription", "a group to reshard"),
        ("Engine", state::DEFAULT_ENGINE),
        ("CacheNodeType", "cache.m7g.large"),
    ]);
    pairs.extend(params(extra));
    let created = emulator.call("CreateReplicationGroup", &pairs).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the create must answer 200, got {} {}",
        created.status, created.body
    );
    std::thread::sleep(PAST_TRANSITION);
}

/// `ModifyReplicationGroupShardConfiguration` with the pairs a row sends.
async fn reshard(emulator: &Emulator, id: &str, extra: &[(String, String)]) -> Reply {
    let mut pairs = params(&[("ReplicationGroupId", id), ("ApplyImmediately", "true")]);
    pairs.extend(extra.iter().cloned());
    emulator
        .call("ModifyReplicationGroupShardConfiguration", &pairs)
        .await
}

/// `DescribeReplicationGroups` for one group by id.
async fn describe_group(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .call(
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", id)]),
        )
        .await
}

/// Every `CacheClusterId` the account holds, as `DescribeCacheClusters` reports them.
async fn account_cluster_ids(emulator: &Emulator) -> Vec<String> {
    let reply = emulator
        .call("DescribeCacheClusters", &params(&[("MaxRecords", "100")]))
        .await;
    assert_eq!(
        reply.status, 200,
        "DescribeCacheClusters must answer 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .all("CacheClusters/CacheCluster/CacheClusterId")
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// The `(node group id, slots)` pairs a group reports, in document order.
fn reported_shards(reply: &Reply) -> Vec<(String, String)> {
    let ids = reply.all("NodeGroups/NodeGroup/NodeGroupId");
    let slots = reply.all("NodeGroups/NodeGroup/Slots");
    ids.into_iter()
        .zip(slots)
        .map(|(id, slots)| (id.to_string(), slots.to_string()))
        .collect()
}

/// Assert the shards a group reports partition the keyspace exactly once, with no gap and
/// no overlap — the invariant a reshard has to maintain whatever the count.
fn assert_partitions_keyspace(case: &str, reply: &Reply) {
    let shards = reported_shards(reply);
    let mut ranges: Vec<(i32, i32)> = shards
        .iter()
        .map(|(id, slots)| {
            let (start, end) = slots.split_once('-').unwrap_or_else(|| {
                panic!("case {case:?}: shard {id} must report a slot range, got {slots:?}")
            });
            (
                start.parse::<i32>().unwrap_or_else(|_| {
                    panic!("case {case:?}: shard {id} must report a numeric start, got {slots:?}")
                }),
                end.parse::<i32>().unwrap_or_else(|_| {
                    panic!("case {case:?}: shard {id} must report a numeric end, got {slots:?}")
                }),
            )
        })
        .collect();
    ranges.sort_unstable();
    assert_eq!(
        ranges.first().map(|(start, _)| *start),
        Some(0),
        "case {case:?}: the partition must start at slot 0, got {ranges:?}"
    );
    // Source of the keyspace size: `NodeGroupConfiguration$Slots` — "Keyspaces range from
    // 0 to 16,383".
    assert_eq!(
        ranges.last().map(|(_, end)| *end),
        Some(state::KEYSPACE_SLOTS - 1),
        "case {case:?}: the partition must end at slot {}, got {ranges:?}",
        state::KEYSPACE_SLOTS - 1
    );
    assert!(
        ranges.windows(2).all(|pair| pair[1].0 == pair[0].1 + 1),
        "case {case:?}: the partition must have no gap and no overlap, got {ranges:?}"
    );
}

/// A cluster mode enabled create that asks for automatic failover and names no replica
/// count is served with the mode's own default, and the group it builds has a replica in
/// every shard to promote.
///
/// The rows span how the request asks for cluster mode (the shard count, `ClusterMode`)
/// and how many shards it asks for.
#[tokio::test]
async fn automatic_failover_without_a_replica_count_takes_the_modes_default() {
    let emulator = Emulator::new();

    // Each row: the case, the group id, the pairs beside the required ones, and how many
    // shards the group must end up with.
    let rows: [FailoverCase; 4] = [
        (
            "two shards, failover asked for, no replica count",
            "failover-two",
            &[("NumNodeGroups", "2"), ("AutomaticFailoverEnabled", "true")],
            2,
        ),
        (
            "three shards, failover asked for, no replica count",
            "failover-three",
            &[("NumNodeGroups", "3"), ("AutomaticFailoverEnabled", "true")],
            3,
        ),
        (
            "one shard made cluster mode enabled by ClusterMode",
            "failover-cluster-mode",
            &[
                ("NumNodeGroups", "1"),
                ("ClusterMode", state::SETTING_ENABLED),
                ("AutomaticFailoverEnabled", "true"),
            ],
            1,
        ),
        (
            "failover asked for beside Multi-AZ, still no replica count",
            "failover-multi-az",
            &[
                ("NumNodeGroups", "2"),
                ("AutomaticFailoverEnabled", "true"),
                ("MultiAZEnabled", "true"),
            ],
            2,
        ),
    ];

    for (case, id, extra, shards) in rows {
        create_settled_group(&emulator, case, id, extra).await;
        let described = describe_group(&emulator, id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the describe must answer 200, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.all("NodeGroups/NodeGroup/NodeGroupId").len(),
            shards,
            "case {case:?}: expected {shards} node groups, got {}",
            described.body
        );
        assert_eq!(
            described.text("AutomaticFailover"),
            Some(state::SETTING_ENABLED),
            "case {case:?}: expected automatic failover enabled, got {:?}",
            described.text("AutomaticFailover")
        );
        // The default is a replica in every shard: two members, one of them promotable.
        let members = described.all("NodeGroupMembers/NodeGroupMember/CacheClusterId");
        assert_eq!(
            members.len(),
            shards * 2,
            "case {case:?}: expected a primary and one replica in each of {shards} shards, got \
             {members:?} in {}",
            described.body
        );
        // Every member is a cache cluster the account really runs.
        let held = account_cluster_ids(&emulator).await;
        for member in &members {
            assert!(
                held.contains(&(*member).to_string()),
                "case {case:?}: the member cluster {member} must be one DescribeCacheClusters \
                 reports, got {held:?}"
            );
        }
    }
}

/// A reshard writes the group a later read reports: the rows span adding shards, adding
/// them with a placement of their own, and taking them away from either side of the
/// selection.
#[tokio::test]
async fn a_reshard_writes_the_shards_a_later_read_reports() {
    // Each row: the case, the shard count the group is created with, the reshard's own
    // pairs, and the shard count it must end up with.
    let rows: Vec<ReshardCase> = vec![
        (
            "a scale-out from two shards to three",
            2,
            params(&[("NodeGroupCount", "3")]),
            3,
        ),
        (
            "a scale-out from two shards to five",
            2,
            params(&[("NodeGroupCount", "5")]),
            5,
        ),
        (
            "a scale-out that places the new shard itself",
            2,
            {
                let mut pairs = params(&[("NodeGroupCount", "3")]);
                pairs.extend(params(&[
                    (
                        "ReshardingConfiguration.ReshardingConfiguration.1.PreferredAvailabilityZones.AvailabilityZone.1",
                        "us-east-1b",
                    ),
                    (
                        "ReshardingConfiguration.ReshardingConfiguration.1.PreferredAvailabilityZones.AvailabilityZone.2",
                        "us-east-1c",
                    ),
                ]));
                pairs
            },
            3,
        ),
        (
            "a scale-in naming the shard to remove",
            3,
            params(&[
                ("NodeGroupCount", "2"),
                ("NodeGroupsToRemove.NodeGroupToRemove.1", "0003"),
            ]),
            2,
        ),
        (
            "a scale-in naming the shards to retain",
            3,
            params(&[
                ("NodeGroupCount", "1"),
                ("NodeGroupsToRetain.NodeGroupToRetain.1", "0002"),
            ]),
            1,
        ),
    ];

    for (case, created_shards, pairs, expected_shards) in rows {
        let emulator = Emulator::new();
        create_settled_group(
            &emulator,
            case,
            "reshard-group",
            &[
                ("NumNodeGroups", &created_shards.to_string()),
                ("ReplicasPerNodeGroup", "1"),
            ],
        )
        .await;
        let before = describe_group(&emulator, "reshard-group").await;
        let shards_before = reported_shards(&before);
        let clusters_before = account_cluster_ids(&emulator).await;

        let resharded = reshard(&emulator, "reshard-group", &pairs).await;
        assert_eq!(
            resharded.status, 200,
            "case {case:?}: the reshard must answer 200, got {} {}",
            resharded.status, resharded.body
        );
        // Resharding is work the service carries out after the call, so the call
        // itself answers with the group transient.
        assert_eq!(
            resharded.text("Status"),
            Some(state::STATUS_MODIFYING),
            "case {case:?}: expected the transient status in the answer, got {:?}",
            resharded.text("Status")
        );

        // The read that follows is answered once that work is done.
        let described = describe_group(&emulator, "reshard-group").await;
        assert_eq!(
            described.text("Status"),
            Some(state::STATUS_AVAILABLE),
            "case {case:?}: expected the group settled once the clock owes it, got {:?}",
            described.text("Status")
        );
        let shards_after = reported_shards(&described);
        assert_eq!(
            shards_after.len(),
            expected_shards,
            "case {case:?}: expected {expected_shards} node groups, got {shards_after:?}"
        );
        assert_partitions_keyspace(case, &described);
        // A shard that stayed kept its identity; nothing was renumbered under the caller.
        let retained_before: Vec<&String> = shards_before
            .iter()
            .map(|(id, _)| id)
            .filter(|id| shards_after.iter().any(|(after, _)| after == *id))
            .collect();
        assert!(
            !retained_before.is_empty(),
            "case {case:?}: a reshard must keep at least one shard, got {shards_after:?} from \
             {shards_before:?}"
        );

        // Every member cluster the group now reports is one the account runs, and every
        // member of a shard that went is gone.
        let members: Vec<String> = described
            .all("NodeGroupMembers/NodeGroupMember/CacheClusterId")
            .into_iter()
            .map(str::to_string)
            .collect();
        let clusters_after = account_cluster_ids(&emulator).await;
        for member in &members {
            assert!(
                clusters_after.contains(member),
                "case {case:?}: the member cluster {member} must be one DescribeCacheClusters \
                 reports, got {clusters_after:?}"
            );
        }
        for gone in clusters_before
            .iter()
            .filter(|id| !clusters_after.contains(id))
        {
            assert!(
                !members.contains(gone),
                "case {case:?}: the removed cluster {gone} must not still be a member, got \
                 {members:?}"
            );
        }
        assert_eq!(
            clusters_after.len(),
            // Every shard of this group carries a primary and one replica.
            expected_shards * 2,
            "case {case:?}: expected the account to run two clusters per shard, got \
             {clusters_after:?}"
        );
    }
}

/// Of two reshards of one group, exactly one wins: the first leaves the group transient,
/// and the second is refused with the modelled state fault until the clock settles it.
#[tokio::test]
async fn only_one_of_two_reshards_of_a_group_wins() {
    let emulator = Emulator::new();
    create_settled_group(
        &emulator,
        "concurrent reshards",
        "concurrent-group",
        &[("NumNodeGroups", "2"), ("ReplicasPerNodeGroup", "1")],
    )
    .await;

    let first = reshard(&emulator, "concurrent-group", &params(&[("NodeGroupCount", "3")])).await;
    let second = reshard(&emulator, "concurrent-group", &params(&[("NodeGroupCount", "4")])).await;
    assert_eq!(
        first.status, 200,
        "the first reshard must win, got {} {}",
        first.status, first.body
    );
    assert_eq!(
        second.status, 400,
        "the second reshard must be refused while the first is in flight, got {} {}",
        second.status, second.body
    );
    assert_eq!(
        second.error_code(),
        "InvalidReplicationGroupState",
        "expected the modelled state fault, got {}",
        second.body
    );
    assert_eq!(
        second.error_message(),
        state::replication_group_modify_state_refusal(),
        "expected the state refusal this service words for a change to a group, got {:?}",
        second.error_message()
    );

    // Only the winner's change is in the store.
    std::thread::sleep(PAST_TRANSITION);
    let described = describe_group(&emulator, "concurrent-group").await;
    assert_eq!(
        described.all("NodeGroups/NodeGroup/NodeGroupId").len(),
        3,
        "expected the winner's three shards, got {}",
        described.body
    );
    assert_partitions_keyspace("concurrent reshards", &described);

    // And once the group has settled, the next reshard is accepted again.
    let third = reshard(&emulator, "concurrent-group", &params(&[("NodeGroupCount", "4")])).await;
    assert_eq!(
        third.status, 200,
        "a reshard of a settled group must be accepted, got {} {}",
        third.status, third.body
    );
}

/// Every way a reshard can be wrong, and the refusal it gets — with the group left exactly
/// as it was.
#[tokio::test]
async fn a_reshard_is_refused_with_the_rule_it_broke() {
    let emulator = Emulator::new();
    create_settled_group(
        &emulator,
        "the sharded group",
        "refusal-group",
        &[("NumNodeGroups", "2"), ("ReplicasPerNodeGroup", "1")],
    )
    .await;
    // A cluster mode DISABLED group beside it: its keyspace is not partitioned, so it has
    // no shard configuration to change.
    create_settled_group(
        &emulator,
        "the unsharded group",
        "unsharded-group",
        &[("NumCacheClusters", "2")],
    )
    .await;

    // Each row: the case, the group the request names, its pairs, the status and the wire
    // code.
    let rows: Vec<ReshardRefusal> = vec![
        (
            "a group this account does not hold",
            "never-created-group",
            params(&[("NodeGroupCount", "3")]),
            404,
            "ReplicationGroupNotFoundFault",
        ),
        (
            "a cluster mode disabled group",
            "unsharded-group",
            params(&[("NodeGroupCount", "2")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a scale-in that selects no shards",
            "refusal-group",
            params(&[("NodeGroupCount", "1")]),
            400,
            "InvalidParameterCombination",
        ),
        (
            "a scale-in that selects from both sides",
            "refusal-group",
            params(&[
                ("NodeGroupCount", "1"),
                ("NodeGroupsToRemove.NodeGroupToRemove.1", "0002"),
                ("NodeGroupsToRetain.NodeGroupToRetain.1", "0001"),
            ]),
            400,
            "InvalidParameterCombination",
        ),
        (
            "a scale-out that names shards to remove",
            "refusal-group",
            params(&[
                ("NodeGroupCount", "3"),
                ("NodeGroupsToRemove.NodeGroupToRemove.1", "0002"),
            ]),
            400,
            "InvalidParameterCombination",
        ),
        (
            "a scale-in that places shards",
            "refusal-group",
            params(&[
                ("NodeGroupCount", "1"),
                ("NodeGroupsToRemove.NodeGroupToRemove.1", "0002"),
                (
                    "ReshardingConfiguration.ReshardingConfiguration.1.PreferredAvailabilityZones.AvailabilityZone.1",
                    "us-east-1a",
                ),
            ]),
            400,
            "InvalidParameterCombination",
        ),
        (
            "the count the group already has",
            "refusal-group",
            params(&[("NodeGroupCount", "2")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "no shard at all",
            "refusal-group",
            params(&[("NodeGroupCount", "0")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a negative count",
            "refusal-group",
            params(&[("NodeGroupCount", "-1")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "one shard past the published maximum",
            "refusal-group",
            params(&[(
                "NodeGroupCount",
                (state::MAX_NODE_GROUPS + 1).to_string().as_str(),
            )]),
            400,
            "NodeGroupsPerReplicationGroupQuotaExceeded",
        ),
        (
            "a shard to remove the group does not have",
            "refusal-group",
            params(&[
                ("NodeGroupCount", "1"),
                ("NodeGroupsToRemove.NodeGroupToRemove.1", "0009"),
            ]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a selection that leaves the wrong count",
            "refusal-group",
            params(&[
                ("NodeGroupCount", "1"),
                ("NodeGroupsToRetain.NodeGroupToRetain.1", "0001"),
                ("NodeGroupsToRetain.NodeGroupToRetain.2", "0002"),
            ]),
            400,
            "InvalidParameterValue",
        ),
        (
            "two new shards given the same id",
            "refusal-group",
            params(&[
                ("NodeGroupCount", "4"),
                (
                    "ReshardingConfiguration.ReshardingConfiguration.1.NodeGroupId",
                    "0007",
                ),
                (
                    "ReshardingConfiguration.ReshardingConfiguration.2.NodeGroupId",
                    "0007",
                ),
            ]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a new shard given an id the group already holds",
            "refusal-group",
            params(&[
                ("NodeGroupCount", "3"),
                (
                    "ReshardingConfiguration.ReshardingConfiguration.1.NodeGroupId",
                    "0001",
                ),
            ]),
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, id, pairs, status, code) in rows {
        let refused = reshard(&emulator, id, &pairs).await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected HTTP {status}, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case:?}: expected the wire code {code:?}, got {}",
            refused.body
        );
        // Nothing was written: the group still has the shards it had.
        let described = describe_group(&emulator, "refusal-group").await;
        assert_eq!(
            described.all("NodeGroups/NodeGroup/NodeGroupId").len(),
            2,
            "case {case:?}: a refused reshard must change nothing, got {}",
            described.body
        );
        assert_eq!(
            described.text("Status"),
            Some(state::STATUS_AVAILABLE),
            "case {case:?}: a refused reshard must leave the group settled, got {:?}",
            described.text("Status")
        );
    }

    // The sentence a scale-in that does not select its shards is refused with states the
    // rule, and both ways of breaking it get it.
    for (case, pairs) in [
        ("neither side named", params(&[("NodeGroupCount", "1")])),
        (
            "both sides named",
            params(&[
                ("NodeGroupCount", "1"),
                ("NodeGroupsToRemove.NodeGroupToRemove.1", "0002"),
                ("NodeGroupsToRetain.NodeGroupToRetain.1", "0001"),
            ]),
        ),
    ] {
        let refused = reshard(&emulator, "refusal-group", &pairs).await;
        assert_eq!(
            refused.error_message(),
            state::SCALE_IN_SELECTION_MESSAGE,
            "case {case:?}: expected the scale-in selection sentence, got {:?}",
            refused.error_message()
        );
    }

    // `ApplyImmediately` has one permitted value, and a request that asks for the change
    // to be deferred is refused rather than served as if it had.
    let deferred = emulator
        .call(
            "ModifyReplicationGroupShardConfiguration",
            &params(&[
                ("ReplicationGroupId", "refusal-group"),
                ("NodeGroupCount", "3"),
                ("ApplyImmediately", "false"),
            ]),
        )
        .await;
    assert_eq!(
        deferred.status, 400,
        "a deferred reshard must be refused, got {} {}",
        deferred.status, deferred.body
    );
    assert_eq!(
        deferred.error_code(),
        "InvalidParameterValue",
        "expected the parameter refusal for ApplyImmediately, got {}",
        deferred.body
    );
    assert!(
        deferred.error_message().contains("ApplyImmediately"),
        "expected the refusal to name the member, got {:?}",
        deferred.error_message()
    );
}

/// The reshard is recorded as an event of the group — the other place a state change shows
/// up.
#[tokio::test]
async fn a_reshard_is_recorded_as_an_event() {
    let emulator = Emulator::new();
    create_settled_group(
        &emulator,
        "the logged group",
        "logged-reshard",
        &[("NumNodeGroups", "2"), ("ReplicasPerNodeGroup", "1")],
    )
    .await;
    let resharded = reshard(&emulator, "logged-reshard", &params(&[("NodeGroupCount", "3")])).await;
    assert_eq!(
        resharded.status, 200,
        "the reshard must answer 200, got {} {}",
        resharded.status, resharded.body
    );

    let events = emulator
        .call(
            "DescribeEvents",
            &params(&[
                ("SourceType", "replication-group"),
                ("SourceIdentifier", "logged-reshard"),
            ]),
        )
        .await;
    assert_eq!(
        events.status, 200,
        "the event read must answer 200, got {} {}",
        events.status, events.body
    );
    let messages = events.all("Event/Message");
    assert!(
        messages
            .iter()
            .any(|message| message.to_ascii_lowercase().contains("shard configuration")),
        "expected an event recording the reshard, got {messages:?} from {}",
        events.body
    );
}
