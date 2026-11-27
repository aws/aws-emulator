// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ElastiCache replication-group lifecycle.
//!
//! Every assertion goes over the wire through the same router the production
//! emulator mounts, using the harness in
//! [`crate::cache_cluster_lifecycle`]; nothing reaches into the service's state
//! struct, so a response that is not computed from persisted state cannot pass.
//!
//! The tests are tables: each row is one input along the axis the rule under
//! test generalises over (shard count, replica count, cluster mode, engine,
//! port, zone placement, keyspace partition, page size), and a single
//! `CreateReplicationGroup` call is only one row.

use crate::harness::{
    assert_own_endpoint, hold_pending_changes, is_own_host, params, poll_until,
    release_pending_changes, Emulator, RefusalCase, Reply, PAST_TRANSITION, REGION,
};

/// The ADDRESS and PORT one member cluster reports as its own node endpoint —
/// which is where this emulator serves that member's data plane, and so what the
/// group's primary or reader endpoint for it has to be.
async fn describe_cluster_endpoint(
    emulator: &Emulator,
    cache_cluster_id: &str,
) -> (Option<String>, Option<String>) {
    let reply = emulator
        .call(
            "DescribeCacheClusters",
            &params(&[
                ("CacheClusterId", cache_cluster_id),
                ("ShowCacheNodeInfo", "true"),
            ]),
        )
        .await;
    (
        reply
            .text("CacheNodes/CacheNode/Endpoint/Address")
            .map(str::to_string),
        reply
            .text("CacheNodes/CacheNode/Endpoint/Port")
            .map(str::to_string),
    )
}

/// Create one replication group with `ReplicationGroupId`, a description, and
/// whatever else `extra` names.
async fn create_group(emulator: &Emulator, id: &str, extra: &[(&str, &str)]) -> Reply {
    let mut pairs = params(&[
        ("ReplicationGroupId", id),
        ("ReplicationGroupDescription", "an emulated replication group"),
    ]);
    pairs.extend(params(extra));
    emulator.call("CreateReplicationGroup", &pairs).await
}

/// Describe one replication group by id.
async fn describe_group(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .call(
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", id)]),
        )
        .await
}

/// One member-cluster row: the group id, the form pairs that create the group,
/// and the member cluster ids it must be made of.
type MemberCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static [&'static str],
);

/// One keyspace-partition row: the case name, the form pairs that make the
/// request, and the `awsQuery` error code it must be refused with.
type PartitionCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static str,
);

/// One encryption row: the group id, the form pairs that create it, and what
/// the group and its member cluster must report for at-rest encryption,
/// in-transit encryption and the in-transit encryption mode.
type EncryptionCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static str,
    &'static str,
    Option<&'static str>,
);

/// One row of the create / round-trip table.
#[derive(Debug, Clone)]
struct GroupCase {
    id: &'static str,
    /// The form pairs sent beside the id and the description.
    extra: &'static [(&'static str, &'static str)],
    /// The identity the group is stored and reported under.
    stored_id: &'static str,
    engine: &'static str,
    /// Whether the keyspace is partitioned across shards.
    cluster_enabled: bool,
    cluster_mode: &'static str,
    /// The member cluster ids, in the order the group reports them.
    members: &'static [&'static str],
    /// One keyspace per shard, or empty for a group that is not partitioned.
    slots: &'static [&'static str],
    automatic_failover: &'static str,
    multi_az: &'static str,
    at_rest_encryption: &'static str,
    storage_encryption_type: &'static str,
}

#[tokio::test]
async fn a_created_replication_group_is_read_back_from_persisted_state() {
    // The axis: how the group is partitioned (`NumNodeGroups`, `ClusterMode`),
    // how many clusters each shard has (`NumCacheClusters`,
    // `ReplicasPerNodeGroup`), the engine and its documented at-rest encryption
    // default ("true when using Valkey, false when using Redis OSS"), and the
    // port. The member cluster ids are the rule ElastiCache derives from the
    // group id: `<group>-00N`, and `<group>-000S-00N` once the keyspace is
    // partitioned.
    let cases = [
        GroupCase {
            id: "demo-rg",
            extra: &[("NumCacheClusters", "2"), ("CacheNodeType", "cache.t3.micro")],
            stored_id: "demo-rg",
            engine: "redis",
            cluster_enabled: false,
            cluster_mode: "disabled",
            members: &["demo-rg-001", "demo-rg-002"],
            slots: &[],
            // "Default: false", and a cluster mode disabled group keeps it.
            automatic_failover: "disabled",
            multi_az: "disabled",
            at_rest_encryption: "false",
            storage_encryption_type: "none",
        },
        GroupCase {
            id: "Mixed-Case-RG",
            // `ReplicationGroupId` "is stored as a lowercase string", and so is
            // the engine name.
            extra: &[("Engine", "VALKEY"), ("ReplicasPerNodeGroup", "0")],
            stored_id: "mixed-case-rg",
            engine: "valkey",
            cluster_enabled: false,
            cluster_mode: "disabled",
            members: &["mixed-case-rg-001"],
            slots: &[],
            automatic_failover: "disabled",
            multi_az: "disabled",
            // "Default: true when using Valkey, false when using Redis OSS."
            at_rest_encryption: "true",
            storage_encryption_type: "sse-elasticache",
        },
        GroupCase {
            id: "sharded-rg",
            extra: &[("NumNodeGroups", "3"), ("ReplicasPerNodeGroup", "1")],
            stored_id: "sharded-rg",
            engine: "redis",
            cluster_enabled: true,
            cluster_mode: "enabled",
            members: &[
                "sharded-rg-0001-001",
                "sharded-rg-0001-002",
                "sharded-rg-0002-001",
                "sharded-rg-0002-002",
                "sharded-rg-0003-001",
                "sharded-rg-0003-002",
            ],
            // The 16,384 slots split as evenly as they divide.
            slots: &["0-5461", "5462-10922", "10923-16383"],
            // "AutomaticFailoverEnabled must be enabled for Valkey or Redis OSS
            // (cluster mode enabled) replication groups."
            automatic_failover: "enabled",
            multi_az: "disabled",
            at_rest_encryption: "false",
            storage_encryption_type: "none",
        },
        GroupCase {
            id: "one-shard-enabled",
            // Cluster mode asked for explicitly, with the single shard a
            // request that omits `NumNodeGroups` gets.
            extra: &[("ClusterMode", "enabled")],
            stored_id: "one-shard-enabled",
            engine: "redis",
            cluster_enabled: true,
            cluster_mode: "enabled",
            members: &["one-shard-enabled-0001-001"],
            slots: &["0-16383"],
            // Automatic failover "must be enabled for Valkey or Redis OSS
            // (cluster mode enabled) replication groups", whatever each shard's
            // replica count is — so a cluster mode enabled group that names no
            // setting gets the only one that mode takes.
            automatic_failover: "enabled",
            multi_az: "disabled",
            at_rest_encryption: "false",
            storage_encryption_type: "none",
        },
        GroupCase {
            id: "max-replicas-rg",
            extra: &[
                // The published upper bound: "1 primary plus 5 replicas".
                ("ReplicasPerNodeGroup", "5"),
                ("AutomaticFailoverEnabled", "true"),
                ("MultiAZEnabled", "true"),
                ("Port", "7000"),
                ("AtRestEncryptionEnabled", "true"),
                ("KmsKeyId", "alias/emulated-key"),
            ],
            stored_id: "max-replicas-rg",
            engine: "redis",
            cluster_enabled: false,
            cluster_mode: "disabled",
            members: &[
                "max-replicas-rg-001",
                "max-replicas-rg-002",
                "max-replicas-rg-003",
                "max-replicas-rg-004",
                "max-replicas-rg-005",
                "max-replicas-rg-006",
            ],
            slots: &[],
            automatic_failover: "enabled",
            multi_az: "enabled",
            at_rest_encryption: "true",
            // "sse-kms if a customer-managed KMS key is used."
            storage_encryption_type: "sse-kms",
        },
    ];

    let emulator = Emulator::new();
    // Create every row first, then wait out the transition span ONCE.
    let mut creates = Vec::new();
    for case in &cases {
        let created = create_group(&emulator, case.id, case.extra).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: create must answer 200, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            created.text("ReplicationGroup/Status"),
            Some("creating"),
            "case {case:?}: a just-created group is not ready yet, got {:?}",
            created.text("ReplicationGroup/Status")
        );
        creates.push(created);
    }
    // Each row is read back once the clock owes it `available` — waited for by looking
    // again at the condition, not by waiting a fixed time.
    for case in &cases {
        poll_until(
            &emulator,
            REGION,
            case.id,
            "the group reaching available",
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", case.id)]),
            |reply| reply.text("ReplicationGroup/Status") == Some("available"),
        )
        .await;
    }

    for (case, created) in cases.iter().zip(&creates) {
        let described = describe_group(&emulator, case.id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: describe must answer 200, got {} {}",
            described.status, described.body
        );
        for (field, expected) in [
            ("ReplicationGroup/ReplicationGroupId", case.stored_id),
            ("ReplicationGroup/Engine", case.engine),
            ("ReplicationGroup/Status", "available"),
            (
                "ReplicationGroup/ClusterEnabled",
                if case.cluster_enabled { "true" } else { "false" },
            ),
            ("ReplicationGroup/ClusterMode", case.cluster_mode),
            ("ReplicationGroup/AutomaticFailover", case.automatic_failover),
            ("ReplicationGroup/MultiAZ", case.multi_az),
            (
                "ReplicationGroup/AtRestEncryptionEnabled",
                case.at_rest_encryption,
            ),
            (
                "ReplicationGroup/StorageEncryptionType",
                case.storage_encryption_type,
            ),
            // "Default: 0 (i.e., automatic backups are disabled)".
            ("ReplicationGroup/SnapshotRetentionLimit", "0"),
            ("ReplicationGroup/AuthTokenEnabled", "false"),
            ("ReplicationGroup/TransitEncryptionEnabled", "false"),
            // `Durability` reports what the request asked for, and
            // `EffectiveDurability` what it resolves to.
            ("ReplicationGroup/Durability", "default"),
            ("ReplicationGroup/EffectiveDurability", "async"),
            (
                "ReplicationGroup/Description",
                "an emulated replication group",
            ),
        ] {
            assert_eq!(
                described.text(field),
                Some(expected),
                "case {case:?}: {field} must report the stored value, got {:?}",
                described.text(field)
            );
        }
        assert_eq!(
            described.text("ReplicationGroup/ARN"),
            Some(
                format!(
                    "arn:aws:elasticache:{REGION}:{}:replicationgroup:{}",
                    engine::emulator_account_id(),
                    case.stored_id
                )
                .as_str()
            ),
            "case {case:?}: the ARN must name the replication group, got {:?}",
            described.text("ReplicationGroup/ARN")
        );

        // The member clusters: the ids ElastiCache derives from the group id.
        assert_eq!(
            described.all("MemberClusters/ClusterId"),
            case.members.to_vec(),
            "case {case:?}: the member clusters must be the stored ones"
        );

        // One node group per shard, with the 4-digit ids the model documents.
        let shards = described.all("NodeGroups/NodeGroup/NodeGroupId");
        let expected_shards = case.slots.len().max(1);
        assert_eq!(
            shards.len(),
            expected_shards,
            "case {case:?}: expected {expected_shards} node groups, got {shards:?}"
        );
        assert_eq!(
            shards.first(),
            Some(&"0001"),
            "case {case:?}: node group ids are 4-digit and start at 0001, got {shards:?}"
        );
        assert_eq!(
            described.all("NodeGroup/Slots"),
            case.slots.to_vec(),
            "case {case:?}: the keyspace partition must be the stored one"
        );
        assert_eq!(
            described.all("NodeGroupMember/CacheClusterId").len(),
            case.members.len(),
            "case {case:?}: every member cluster must appear in a node group, got {:?}",
            described.all("NodeGroupMember/CacheClusterId")
        );

        // `CurrentRole` and `ReadEndpoint` are "only applicable for Valkey or
        // Redis OSS (cluster mode disabled)" groups; a partitioned group is
        // reached through its configuration endpoint instead.
        let roles = described.all("NodeGroupMember/CurrentRole");
        let endpoint = described.text("ReplicationGroup/ConfigurationEndpoint/Address");
        let primary_endpoint = described.text("NodeGroup/PrimaryEndpoint/Address");
        if case.cluster_enabled {
            assert!(
                roles.is_empty(),
                "case {case:?}: a cluster mode enabled group reports no CurrentRole, got {roles:?}"
            );
            assert!(
                endpoint.is_some_and(is_own_host),
                "case {case:?}: the configuration endpoint of a group this emulator serves must \
                 be an address it answers on, got {endpoint:?}"
            );
            assert!(
                primary_endpoint.is_none(),
                "case {case:?}: a cluster mode enabled group reports no shard primary endpoint, \
                 got {primary_endpoint:?}"
            );
        } else {
            assert_eq!(
                roles.first(),
                Some(&"primary"),
                "case {case:?}: the first member of a shard is its primary, got {roles:?}"
            );
            assert_eq!(
                roles.iter().filter(|role| **role == "primary").count(),
                1,
                "case {case:?}: a shard has exactly one primary, got {roles:?}"
            );
            assert!(
                endpoint.is_none(),
                "case {case:?}: a cluster mode disabled group has no configuration endpoint, got \
                 {endpoint:?}"
            );
            // The group's primary endpoint is the address of the member holding
            // the primary role, which for a group this emulator serves is the
            // socket that member answers on; its reader endpoint is a replica's,
            // or the primary's where the shard holds no replica to read from.
            assert_own_endpoint(
                case.stored_id,
                "the shard's primary endpoint",
                primary_endpoint.unwrap_or_default(),
                described.text("NodeGroup/PrimaryEndpoint/Port"),
            );
            let reader = described.text("NodeGroup/ReaderEndpoint/Address");
            assert_own_endpoint(
                case.stored_id,
                "the shard's reader endpoint",
                reader.unwrap_or_default(),
                described.text("NodeGroup/ReaderEndpoint/Port"),
            );
            if case.members.len() > 1 {
                assert_ne!(
                    described.text("NodeGroup/ReaderEndpoint/Port"),
                    described.text("NodeGroup/PrimaryEndpoint/Port"),
                    "case {case:?}: a shard with a replica reads at another member's socket than \
                     it writes at, got {reader:?}"
                );
            } else {
                // A shard with no replica still publishes a reader endpoint —
                // where its reads are served from is the member holding the
                // primary role.
                assert_eq!(
                    described.text("NodeGroup/ReaderEndpoint/Port"),
                    described.text("NodeGroup/PrimaryEndpoint/Port"),
                    "case {case:?}: a shard with no replica reads where it writes, got {reader:?}"
                );
            }
        }

        // Every port reported anywhere in the group is a port the emulator's own
        // socket for that member answers on: one per member, and each reported
        // the same wherever it appears.
        let ports = described.all("Port");
        assert!(
            !ports.is_empty()
                && ports
                    .iter()
                    .all(|port| port.parse::<u16>().is_ok_and(|port| port > 0)),
            "case {case:?}: every endpoint must carry the port its member answers on, got \
             {ports:?}"
        );
        let distinct: std::collections::BTreeSet<&&str> = ports.iter().collect();
        // A cluster mode ENABLED group is reached at its configuration endpoint
        // alone, so one socket is reported; a cluster mode disabled one reports a
        // primary and a reader endpoint per member, which is one socket each.
        let expected_sockets = if case.cluster_enabled {
            1
        } else {
            case.members.len()
        };
        assert_eq!(
            distinct.len(),
            expected_sockets,
            "case {case:?}: the group is reached at {expected_sockets} socket(s), got {ports:?}"
        );

        // The create response and the describe response must agree on every
        // value the service minted, not just on the ones the request carried.
        for minted in [
            "ReplicationGroup/ARN",
            "ReplicationGroup/ReplicationGroupCreateTime",
        ] {
            assert_eq!(
                created.text(minted),
                described.text(minted),
                "case {case:?}: {minted} must be stored once, got {:?} then {:?}",
                created.text(minted),
                described.text(minted)
            );
            assert!(
                created.text(minted).is_some(),
                "case {case:?}: {minted} must be reported"
            );
        }
    }
}

#[tokio::test]
async fn the_member_clusters_of_a_group_are_the_clusters_the_cache_cluster_api_reports() {
    let emulator = Emulator::new();
    // Two rows: a partitioned group and an unpartitioned one, so the member
    // cluster rule is checked on both id forms.
    let cases: [MemberCase; 2] = [
        (
            "flat-rg",
            &[("NumCacheClusters", "2")],
            &["flat-rg-001", "flat-rg-002"],
        ),
        (
            "shard-rg",
            &[("NumNodeGroups", "2")],
            &["shard-rg-0001-001", "shard-rg-0002-001"],
        ),
    ];

    for (id, extra, _) in cases {
        let created = create_group(&emulator, id, extra).await;
        assert_eq!(
            created.status, 200,
            "case {id}: create must answer 200, got {}",
            created.body
        );
    }
    std::thread::sleep(PAST_TRANSITION);

    for (id, _, members) in cases {
        for member in members {
            let described = emulator
                .call(
                    "DescribeCacheClusters",
                    &params(&[("CacheClusterId", member), ("ShowCacheNodeInfo", "true")]),
                )
                .await;
            assert_eq!(
                described.status, 200,
                "case {id}/{member}: the member cluster must be described, got {} {}",
                described.status, described.body
            );
            assert_eq!(
                described.text("CacheCluster/ReplicationGroupId"),
                Some(id),
                "case {id}/{member}: the member cluster must name its group, got {:?}",
                described.text("CacheCluster/ReplicationGroupId")
            );
            assert_eq!(
                described.all("CacheNodes/CacheNode/CacheNodeId"),
                vec!["0001"],
                "case {id}/{member}: a Valkey or Redis OSS member cluster holds one node, got {:?}",
                described.all("CacheNodes/CacheNode/CacheNodeId")
            );
        }
    }

    // `ShowCacheClustersNotInReplicationGroups` keeps only the clusters that
    // belong to no group, and every cluster here belongs to one.
    let standalone = emulator
        .call(
            "DescribeCacheClusters",
            &params(&[("ShowCacheClustersNotInReplicationGroups", "true")]),
        )
        .await;
    assert_eq!(
        standalone.all("CacheCluster/CacheClusterId"),
        Vec::<&str>::new(),
        "every stored cluster is a replication group member, got {:?}",
        standalone.all("CacheCluster/CacheClusterId")
    );

    // A cluster can only join a replication group that exists.
    let unknown = emulator
        .call(
            "CreateCacheCluster",
            &params(&[("CacheClusterId", "joiner"), ("ReplicationGroupId", "nope")]),
        )
        .await;
    assert_eq!(
        (unknown.status, unknown.error_code()),
        (404, "ReplicationGroupNotFoundFault"),
        "a cluster joining a group that does not exist is refused, got {} {}",
        unknown.status,
        unknown.body
    );
    // Joining one that DOES exist is behaviour nobody has written yet, so it
    // answers the scaffold's 501 rather than a success it cannot persist.
    let existing = emulator
        .call(
            "CreateCacheCluster",
            &params(&[
                ("CacheClusterId", "joiner"),
                ("ReplicationGroupId", "flat-rg"),
            ]),
        )
        .await;
    assert_eq!(
        (existing.status, existing.error_code()),
        (501, "NotImplemented"),
        "joining an existing group is not implemented yet, got {} {}",
        existing.status,
        existing.body
    );
}

#[tokio::test]
async fn a_group_built_on_an_existing_primary_cluster_adopts_it() {
    let emulator = Emulator::new();
    let cluster = emulator
        .call(
            "CreateCacheCluster",
            &crate::harness::cluster_create_params(
                "legacy-primary",
                &[("Engine", "valkey"), ("Port", "7111")],
            ),
        )
        .await;
    assert_eq!(
        cluster.status, 200,
        "the primary cluster must be created, got {}",
        cluster.body
    );

    // "This cluster must already exist and have a status of available": while
    // the clock still owes it `available`, the create is refused. The clock owes a
    // just-created cluster that status from the call that created it, so the phase
    // is held out of reach here rather than raced for.
    hold_pending_changes(&emulator);
    let too_early = create_group(
        &emulator,
        "adopting-rg",
        &[("PrimaryClusterId", "legacy-primary")],
    )
    .await;
    assert_eq!(
        (too_early.status, too_early.error_code()),
        (400, "InvalidCacheClusterState"),
        "a primary that is still creating is refused, got {} {}",
        too_early.status,
        too_early.body
    );

    // Waited for by looking again at the CONDITION the create needs — the primary
    // reaching `available` — rather than by waiting a fixed time.
    release_pending_changes(&emulator);
    poll_until(
        &emulator,
        REGION,
        "legacy-primary",
        "the primary cluster reaching available",
        "DescribeCacheClusters",
        &params(&[("CacheClusterId", "legacy-primary")]),
        |reply| reply.text("CacheCluster/CacheClusterStatus") == Some("available"),
    )
    .await;
    let created = create_group(
        &emulator,
        "adopting-rg",
        &[("PrimaryClusterId", "legacy-primary")],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the group must be created on the available primary, got {} {}",
        created.status,
        created.body
    );
    let described = poll_until(
        &emulator,
        REGION,
        "adopting-rg",
        "the group reaching available",
        "DescribeReplicationGroups",
        &params(&[("ReplicationGroupId", "adopting-rg")]),
        |reply| reply.text("ReplicationGroup/Status") == Some("available"),
    )
    .await;
    assert_eq!(
        described.all("MemberClusters/ClusterId"),
        vec!["legacy-primary"],
        "the named primary keeps its own id as the group's member, got {:?}",
        described.all("MemberClusters/ClusterId")
    );
    // The group takes the primary's configuration wherever the request named
    // none.
    assert_eq!(
        described.text("ReplicationGroup/Engine"),
        Some("valkey"),
        "the engine must be inherited from the primary cluster, got {:?}",
        described.text("ReplicationGroup/Engine")
    );
    // The group is reached at the ADDRESS the adopted primary answers on — the
    // socket this emulator serves that member's data plane at, which is also what
    // the member cluster reports as its own node endpoint.
    let member_endpoint = describe_cluster_endpoint(&emulator, "legacy-primary").await;
    assert_eq!(
        (
            described.text("NodeGroup/PrimaryEndpoint/Address"),
            described.text("NodeGroup/PrimaryEndpoint/Port")
        ),
        (member_endpoint.0.as_deref(), member_endpoint.1.as_deref()),
        "the group's primary endpoint must be the adopted member's own address, got {:?}",
        described.text("NodeGroup/PrimaryEndpoint/Address")
    );
    // The adopted cluster now names its group.
    let member = emulator
        .call(
            "DescribeCacheClusters",
            &params(&[("CacheClusterId", "legacy-primary")]),
        )
        .await;
    assert_eq!(
        member.text("CacheCluster/ReplicationGroupId"),
        Some("adopting-rg"),
        "the adopted cluster must name its group, got {:?}",
        member.text("CacheCluster/ReplicationGroupId")
    );
    // A cluster that is already a member cannot be another group's primary.
    let again = create_group(
        &emulator,
        "second-rg",
        &[("PrimaryClusterId", "legacy-primary")],
    )
    .await;
    assert_eq!(
        (again.status, again.error_code()),
        (400, "InvalidParameterValue"),
        "a cluster that already belongs to a group is refused as a primary, got {} {}",
        again.status,
        again.body
    );
}

#[tokio::test]
async fn the_keyspace_is_partitioned_across_every_shard_exactly_once() {
    let emulator = Emulator::new();
    // The axis is the shard count: the keyspace 0-16383 is split over it, so
    // whatever the count, the partition starts at 0, ends at 16383 and leaves
    // no gap. The bound is the published maximum of 90 shards.
    for shards in [1usize, 2, 3, 7, 16] {
        let id = format!("partition-{shards}");
        let created = create_group(
            &emulator,
            &id,
            &[
                ("NumNodeGroups", shards.to_string().as_str()),
                // A single shard is partitioned only when the request asks for
                // cluster mode; more than one shard is partitioned by itself.
                ("ClusterMode", "enabled"),
            ],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {shards} shards: create must answer 200, got {} {}",
            created.status, created.body
        );
        let slots = created.all("NodeGroup/Slots");
        assert_eq!(
            slots.len(),
            shards,
            "case {shards} shards: one keyspace per shard, got {slots:?}"
        );
        let mut next = 0i32;
        for (index, range) in slots.iter().enumerate() {
            let (start, end) = range
                .split_once('-')
                .unwrap_or_else(|| panic!("case {shards} shards: {range} is not a slot range"));
            let start: i32 = start
                .parse()
                .unwrap_or_else(|_| panic!("case {shards} shards: {range} has no start slot"));
            let end: i32 = end
                .parse()
                .unwrap_or_else(|_| panic!("case {shards} shards: {range} has no end slot"));
            assert_eq!(
                start, next,
                "case {shards} shards: shard {index} must continue the keyspace at {next}, got \
                 {slots:?}"
            );
            assert!(
                end >= start,
                "case {shards} shards: shard {index} must not be empty, got {slots:?}"
            );
            next = end + 1;
        }
        assert_eq!(
            next, 16_384,
            "case {shards} shards: the partition must cover all 16384 slots, got {slots:?}"
        );
    }

    // A request may partition the keyspace itself, and that partition is what
    // is stored.
    let configured = create_group(
        &emulator,
        "configured-slots",
        &[
            ("NumNodeGroups", "2"),
            ("NodeGroupConfiguration.NodeGroupConfiguration.1.NodeGroupId", "7"),
            ("NodeGroupConfiguration.NodeGroupConfiguration.1.Slots", "0-999"),
            (
                "NodeGroupConfiguration.NodeGroupConfiguration.1.PrimaryAvailabilityZone",
                "us-east-1f",
            ),
            ("NodeGroupConfiguration.NodeGroupConfiguration.2.Slots", "1000-16383"),
        ],
    )
    .await;
    assert_eq!(
        configured.status, 200,
        "a request that names the keyspaces must be accepted, got {} {}",
        configured.status,
        configured.body
    );
    assert_eq!(
        configured.all("NodeGroup/Slots"),
        vec!["0-999", "1000-16383"],
        "the named keyspaces must be the stored ones, got {:?}",
        configured.all("NodeGroup/Slots")
    );
    assert_eq!(
        configured.all("NodeGroups/NodeGroup/NodeGroupId"),
        vec!["0007", "0002"],
        "a user supplied node group id is stored in its 4-digit form, got {:?}",
        configured.all("NodeGroups/NodeGroup/NodeGroupId")
    );
    assert_eq!(
        configured.all("MemberClusters/ClusterId"),
        vec!["configured-slots-0002-001", "configured-slots-0007-001"],
        "a member cluster id is derived from the group id and its shard's own id, got {:?}",
        configured.all("MemberClusters/ClusterId")
    );
    assert_eq!(
        configured.text("NodeGroupMember/PreferredAvailabilityZone"),
        Some("us-east-1f"),
        "the shard's primary must sit in the zone the request named, got {:?}",
        configured.text("NodeGroupMember/PreferredAvailabilityZone")
    );
}

#[tokio::test]
async fn a_create_this_service_cannot_satisfy_is_refused_with_its_modelled_error() {
    let emulator = Emulator::new();
    create_group(&emulator, "taken-rg", &[]).await;

    // Each row: the members that make the request wrong, and the modelled
    // refusal. The axis is the rule each one breaks — a name, a bound, a
    // resource that does not exist, or two members that cannot hold together.
    let cases: [RefusalCase; 24] = [
        (
            "duplicate group",
            &[("ReplicationGroupId", "taken-rg")],
            400,
            "ReplicationGroupAlreadyExists",
        ),
        (
            "an id that does not start with a letter",
            &[("ReplicationGroupId", "9-rg")],
            400,
            "InvalidParameterValue",
        ),
        (
            "an id with two consecutive hyphens",
            &[("ReplicationGroupId", "rg--two")],
            400,
            "InvalidParameterValue",
        ),
        (
            "an id longer than 40 characters",
            &[(
                "ReplicationGroupId",
                "rg-that-is-far-too-long-to-be-a-replication-group",
            )],
            400,
            "InvalidParameterValue",
        ),
        (
            "an engine a replication group cannot run",
            &[("Engine", "memcached")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a Global datastore that does not exist",
            &[("GlobalReplicationGroupId", "ldgnf-global")],
            404,
            "GlobalReplicationGroupNotFoundFault",
        ),
        (
            "a user group that does not exist",
            &[("UserGroupIds.member.1", "ug-1")],
            404,
            "UserGroupNotFound",
        ),
        (
            // The whole cache-security-group surface is closed for this
            // account, so naming one is a bad parameter value rather than a
            // group that could be created and retried with.
            "a cache security group",
            &[("CacheSecurityGroupNames.CacheSecurityGroupName.1", "sg-a")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a parameter group that does not exist",
            &[("CacheParameterGroupName", "my-own-group")],
            404,
            "CacheParameterGroupNotFound",
        ),
        (
            "a subnet group that does not exist",
            &[("CacheSubnetGroupName", "nope")],
            400,
            "CacheSubnetGroupNotFoundFault",
        ),
        (
            "a snapshot that does not exist",
            &[("SnapshotName", "nightly")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a port outside the TCP range",
            &[("Port", "70000")],
            400,
            "InvalidParameterValue",
        ),
        (
            "more clusters than a group may have",
            &[("NumCacheClusters", "7")],
            400,
            "InvalidParameterValue",
        ),
        (
            "more replicas than a shard may have",
            &[("ReplicasPerNodeGroup", "6")],
            400,
            "InvalidParameterValue",
        ),
        (
            "more shards than a group may have",
            &[("NumNodeGroups", "91")],
            400,
            "NodeGroupsPerReplicationGroupQuotaExceeded",
        ),
        (
            "no shard at all",
            &[("NumNodeGroups", "0")],
            400,
            "InvalidParameterValue",
        ),
        (
            "automatic failover with no replica to promote",
            &[("AutomaticFailoverEnabled", "true")],
            400,
            "InvalidParameterCombination",
        ),
        (
            "automatic failover turned off on a sharded group",
            &[
                ("NumNodeGroups", "2"),
                ("ReplicasPerNodeGroup", "1"),
                ("AutomaticFailoverEnabled", "false"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "Multi-AZ without automatic failover",
            &[("MultiAZEnabled", "true")],
            400,
            "InvalidParameterCombination",
        ),
        (
            "an auth token without in-transit encryption",
            &[("AuthToken", "aVeryLongPassword123")],
            400,
            "InvalidParameterValue",
        ),
        (
            "an auth token shorter than 16 characters",
            &[("TransitEncryptionEnabled", "true"), ("AuthToken", "short")],
            400,
            "InvalidParameterValue",
        ),
        (
            "an auth token with a character the password rule reserves",
            &[
                ("TransitEncryptionEnabled", "true"),
                ("AuthToken", "has@an@at@sign12"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "NumCacheClusters on a sharded group",
            &[("NumNodeGroups", "2"), ("NumCacheClusters", "2")],
            400,
            "InvalidParameterCombination",
        ),
        (
            "cluster mode disabled with more than one shard",
            &[("NumNodeGroups", "2"), ("ClusterMode", "disabled")],
            400,
            "InvalidParameterCombination",
        ),
    ];

    for (case, extra, status, code) in cases {
        let mut pairs = params(&[
            ("ReplicationGroupId", "refused-rg"),
            ("ReplicationGroupDescription", "refused"),
        ]);
        pairs.extend(params(extra));
        // A row that carries its own id must win over the default.
        for (key, value) in params(extra) {
            if key == "ReplicationGroupId" {
                pairs.retain(|(existing, _)| existing != "ReplicationGroupId");
                pairs.push((key, value));
            }
        }
        let reply = emulator.call("CreateReplicationGroup", &pairs).await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case}: expected error code {code}, got {}",
            reply.body
        );
        // A refused create persists nothing.
        let described = describe_group(&emulator, "refused-rg").await;
        assert_eq!(
            (described.status, described.error_code()),
            (404, "ReplicationGroupNotFoundFault"),
            "case {case}: a refused create must persist no group, got {} {}",
            described.status,
            described.body
        );
    }
}

#[tokio::test]
async fn a_keyspace_partition_that_does_not_cover_the_keyspace_once_is_refused() {
    let emulator = Emulator::new();
    // Each row is one way a named partition can be wrong: a gap, an overlap, a
    // malformed range, a slot past the end of the keyspace, a partition given
    // for only some shards, and a partition on a group that is not partitioned
    // at all.
    let cases: [PartitionCase; 7] = [
        (
            "two shards named by the same node group id",
            &[
                ("NumNodeGroups", "2"),
                (
                    "NodeGroupConfiguration.NodeGroupConfiguration.1.NodeGroupId",
                    "3",
                ),
                ("NodeGroupConfiguration.NodeGroupConfiguration.1.Slots", "0-999"),
                (
                    "NodeGroupConfiguration.NodeGroupConfiguration.2.NodeGroupId",
                    "3",
                ),
                (
                    "NodeGroupConfiguration.NodeGroupConfiguration.2.Slots",
                    "1000-16383",
                ),
            ],
            "InvalidParameterCombination",
        ),
        (
            "a gap in the keyspace",
            &[
                ("NumNodeGroups", "2"),
                ("NodeGroupConfiguration.NodeGroupConfiguration.1.Slots", "0-999"),
                (
                    "NodeGroupConfiguration.NodeGroupConfiguration.2.Slots",
                    "2000-16383",
                ),
            ],
            "InvalidParameterCombination",
        ),
        (
            "overlapping shards",
            &[
                ("NumNodeGroups", "2"),
                ("NodeGroupConfiguration.NodeGroupConfiguration.1.Slots", "0-9000"),
                (
                    "NodeGroupConfiguration.NodeGroupConfiguration.2.Slots",
                    "8000-16383",
                ),
            ],
            "InvalidParameterCombination",
        ),
        (
            "a range that is not startkey-endkey",
            &[
                ("ClusterMode", "enabled"),
                ("NodeGroupConfiguration.NodeGroupConfiguration.1.Slots", "whole"),
            ],
            "InvalidParameterValue",
        ),
        (
            "a slot past the end of the keyspace",
            &[
                ("ClusterMode", "enabled"),
                ("NodeGroupConfiguration.NodeGroupConfiguration.1.Slots", "0-16384"),
            ],
            "InvalidParameterValue",
        ),
        (
            "a partition for only one of two shards",
            &[
                ("NumNodeGroups", "2"),
                ("NodeGroupConfiguration.NodeGroupConfiguration.1.Slots", "0-16383"),
                ("NodeGroupConfiguration.NodeGroupConfiguration.2.ReplicaCount", "0"),
            ],
            "InvalidParameterCombination",
        ),
        (
            "a partition on a group that is not partitioned",
            &[(
                "NodeGroupConfiguration.NodeGroupConfiguration.1.Slots",
                "0-16383",
            )],
            "InvalidParameterCombination",
        ),
    ];

    for (case, extra, code) in cases {
        let reply = create_group(&emulator, "partition-refused", extra).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected HTTP 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case}: expected error code {code}, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_member_cluster_carries_the_groups_encryption_settings() {
    let emulator = Emulator::new();
    // The axis: which encryption the request asked for. At-rest encryption
    // defaults to the engine's setting ("true when using Valkey, false when
    // using Redis OSS"), in-transit encryption to "false", and the in-transit
    // encryption MODE is reported only where the request named one.
    let cases: [EncryptionCase; 4] = [
        ("enc-default-rg", &[], "false", "false", None),
        ("enc-valkey-rg", &[("Engine", "valkey")], "true", "false", None),
        (
            "enc-transit-rg",
            &[
                ("TransitEncryptionEnabled", "true"),
                ("TransitEncryptionMode", "preferred"),
                // "AuthToken can be specified only on replication groups where
                // TransitEncryptionEnabled is true", and it must be at least 16
                // printable ASCII characters.
                ("AuthToken", "aVeryLongPassword1"),
            ],
            "false",
            "true",
            Some("preferred"),
        ),
        (
            "enc-kms-rg",
            &[
                ("AtRestEncryptionEnabled", "true"),
                ("KmsKeyId", "alias/emulated-key"),
            ],
            "true",
            "false",
            None,
        ),
    ];

    for (id, extra, _, _, _) in cases {
        let created = create_group(&emulator, id, extra).await;
        assert_eq!(
            created.status, 200,
            "case {id}: create must answer 200, got {} {}",
            created.status, created.body
        );
    }
    std::thread::sleep(PAST_TRANSITION);

    for (id, extra, at_rest, transit, mode) in cases {
        let group = describe_group(&emulator, id).await;
        let member = emulator
            .call(
                "DescribeCacheClusters",
                &params(&[("CacheClusterId", &format!("{id}-001"))]),
            )
            .await;
        assert_eq!(
            member.status, 200,
            "case {id}: the member cluster must be described, got {} {}",
            member.status, member.body
        );
        let auth_token_enabled = extra.iter().any(|(key, _)| *key == "AuthToken");
        for (reply, prefix) in [(&group, "ReplicationGroup"), (&member, "CacheCluster")] {
            for (member_name, expected) in [
                ("AtRestEncryptionEnabled", Some(at_rest)),
                ("TransitEncryptionEnabled", Some(transit)),
                ("TransitEncryptionMode", mode),
                (
                    "AuthTokenEnabled",
                    Some(if auth_token_enabled { "true" } else { "false" }),
                ),
            ] {
                let field = format!("{prefix}/{member_name}");
                assert_eq!(
                    reply.text(&field),
                    expected,
                    "case {id}: {field} must report the stored setting, got {:?}",
                    reply.text(&field)
                );
            }
        }
    }
}

#[tokio::test]
async fn seeding_a_group_from_a_snapshot_is_refused_by_whether_that_snapshot_exists() {
    let emulator = Emulator::new();
    // A stored snapshot to seed from: `CreateSnapshot` takes one of an available
    // cluster.
    let cluster = emulator
        .call(
            "CreateCacheCluster",
            &crate::harness::cluster_create_params("seed-source", &[("Engine", "redis")]),
        )
        .await;
    assert_eq!(
        cluster.status, 200,
        "the source cluster must be created, got {}",
        cluster.body
    );
    // `CreateSnapshot` takes a snapshot of an AVAILABLE cluster, so this waits for
    // that CONDITION rather than for a span: the handlers never sleep, and only the
    // clock carries the cluster there.
    poll_until(
        &emulator,
        REGION,
        "seed-source",
        "the source cluster reaching available",
        "DescribeCacheClusters",
        &params(&[("CacheClusterId", "seed-source")]),
        |reply| reply.text("CacheCluster/CacheClusterStatus") == Some("available"),
    )
    .await;
    let snapshot = emulator
        .call(
            "CreateSnapshot",
            &params(&[
                ("SnapshotName", "seed-snap"),
                ("CacheClusterId", "seed-source"),
            ]),
        )
        .await;
    assert_eq!(
        snapshot.status, 200,
        "the snapshot must be taken, got {}",
        snapshot.body
    );
    // And a restore source is a snapshot the clock has made `available`, which is
    // the condition a client's own wait is written against.
    poll_until(
        &emulator,
        REGION,
        "seed-snap",
        "the snapshot reaching available",
        "DescribeSnapshots",
        &params(&[("SnapshotName", "seed-snap")]),
        |reply| reply.text("Snapshot/SnapshotStatus") == Some("available"),
    )
    .await;

    // A name this account never took is the modelled bad value; a snapshot the
    // store DOES hold is a restore source, whichever case it is named in ("this
    // parameter is stored as a lowercase string"). The axis is exactly that:
    // whether the named snapshot exists. What the restored group then SERVES is
    // the data the snapshot captured, which the data-consequence suite asserts
    // over the group's own endpoint.
    for (case, id, name, status, code) in [
        (
            "a snapshot this account never took",
            "seeded-rg-missing",
            "no-such-snap",
            400,
            "InvalidParameterValue",
        ),
        ("a snapshot the store holds", "seeded-rg", "seed-snap", 200, ""),
        (
            "the same snapshot in mixed case",
            "seeded-rg-mixed",
            "Seed-Snap",
            200,
            "",
        ),
    ] {
        let reply = create_group(&emulator, id, &[("SnapshotName", name)]).await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case:?}: expected error code {code:?}, got {}",
            reply.body
        );
        let described = describe_group(&emulator, id).await;
        if status == 200 {
            assert_eq!(
                described.status, 200,
                "case {case:?}: an accepted create must persist the group, got {} {}",
                described.status, described.body
            );
        } else {
            assert_eq!(
                (described.status, described.error_code()),
                (404, "ReplicationGroupNotFoundFault"),
                "case {case:?}: a refused create must persist no group, got {} {}",
                described.status,
                described.body
            );
        }
    }
}

#[tokio::test]
async fn describe_replication_groups_pages_the_stored_groups() {
    let emulator = Emulator::new();
    // One more group than the smallest page size the operation serves, so the
    // first page is full and the second carries the remainder.
    let total = 21usize;
    for index in 0..total {
        let id = format!("page-rg-{index:03}");
        let created = create_group(&emulator, &id, &[("ReplicasPerNodeGroup", "0")]).await;
        assert_eq!(
            created.status, 200,
            "case {id}: create must answer 200, got {}",
            created.body
        );
    }

    // "Default: 100. Constraints: minimum 20; maximum 100."
    let refused = emulator
        .call("DescribeReplicationGroups", &params(&[("MaxRecords", "19")]))
        .await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, "InvalidParameterValue"),
        "a page size below the published minimum is refused, got {} {}",
        refused.status,
        refused.body
    );
    let bad_marker = emulator
        .call(
            "DescribeReplicationGroups",
            &params(&[("Marker", "not-a-marker")]),
        )
        .await;
    assert_eq!(
        (bad_marker.status, bad_marker.error_code()),
        (400, "InvalidParameterValue"),
        "a marker this service did not mint is refused, got {} {}",
        bad_marker.status,
        bad_marker.body
    );

    let first = emulator
        .call("DescribeReplicationGroups", &params(&[("MaxRecords", "20")]))
        .await;
    assert_eq!(
        first.status, 200,
        "the first page must answer 200, got {}",
        first.body
    );
    let first_ids = first.all("ReplicationGroups/ReplicationGroup/ReplicationGroupId");
    assert_eq!(
        first_ids.len(),
        20,
        "the first page holds MaxRecords groups, got {first_ids:?}"
    );
    let marker = first
        .marker()
        .expect("more groups exist than one page holds, so a Marker must be returned")
        .to_string();

    let second = emulator
        .call(
            "DescribeReplicationGroups",
            &params(&[("MaxRecords", "20"), ("Marker", &marker)]),
        )
        .await;
    let second_ids = second.all("ReplicationGroups/ReplicationGroup/ReplicationGroupId");
    assert_eq!(
        second_ids.len(),
        total - 20,
        "the second page holds the remaining groups, got {second_ids:?}"
    );
    assert!(
        second.marker().is_none(),
        "the last page carries no Marker, got {:?}",
        second.marker()
    );
    assert!(
        first_ids.iter().all(|id| !second_ids.contains(id)),
        "the two pages must not overlap: {first_ids:?} then {second_ids:?}"
    );

    // Every group is reported when the caller names none.
    let all = emulator
        .call("DescribeReplicationGroups", &params(&[]))
        .await;
    assert_eq!(
        all.all("ReplicationGroups/ReplicationGroup/ReplicationGroupId")
            .len(),
        total,
        "every stored group must be reported, got {:?}",
        all.all("ReplicationGroups/ReplicationGroup/ReplicationGroupId")
    );
    // A group that does not exist is the modelled not-found, not an empty page.
    let missing = describe_group(&emulator, "page-rg-999").await;
    assert_eq!(
        (missing.status, missing.error_code()),
        (404, "ReplicationGroupNotFoundFault"),
        "a group that does not exist is refused, got {} {}",
        missing.status,
        missing.body
    );
}
