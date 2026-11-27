// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what makes a replication group CLUSTER MODE ENABLED.
//!
//! The rule under test: cluster mode is an engine parameter, and a replication group is
//! cluster mode enabled when the request asks for it by any of the ways the service offers —
//! a keyspace partitioned across more than one shard, `ClusterMode` enabled, or a cache
//! parameter group whose `cluster-enabled` parameter is on, which this operation's own
//! documentation names as how such a group is created. `ClusterEnabled` reports that, and so
//! does everything derived from it: the group's mode, its shards' keyspaces and the
//! configuration endpoint a client of a partitioned group connects to.
//!
//! The tables span what the rule generalises over: which of the three ways the request asks
//! for cluster mode, how many shards it asks for, which family's cluster-mode-on default
//! group it names, and a group of the account's OWN on which that parameter was SET — which
//! is why the flag is read off the stored parameter group rather than off its name.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, and each group is read back through `DescribeReplicationGroups`.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, Reply};

/// The node type every group here runs; this suite is not about node types.
const NODE_TYPE: &str = "cache.m7g.large";

/// The cluster-mode-on default parameter group of `family`, named as the service names it
/// (`default.<family>.cluster.on`).
fn cluster_on_group(family: &str) -> String {
    format!(
        "{}{family}{}",
        state::DEFAULT_PARAMETER_GROUP_PREFIX,
        state::CLUSTER_MODE_ON_SUFFIX
    )
}

/// The plain default parameter group of `family`.
fn plain_group(family: &str) -> String {
    format!("{}{family}", state::DEFAULT_PARAMETER_GROUP_PREFIX)
}

/// The published families a replication group can run which publish the cluster-mode
/// parameter at all — read off the service's own catalog rather than written down, so this
/// suite covers the families the service offers.
fn cluster_mode_families() -> Vec<&'static str> {
    state::PARAMETER_GROUP_FAMILIES
        .into_iter()
        .filter(|family| {
            state::family_publishes_cluster_mode(family)
                && matches!(state::family_engine(family), "redis" | "valkey")
        })
        .collect()
}

/// `CreateReplicationGroup` for `id`, running `engine`, with `extra` beside the shared
/// members.
async fn create_group(
    emulator: &Emulator,
    id: &str,
    engine: &str,
    extra: &[(&str, &str)],
) -> Reply {
    let mut pairs = params(&[
        ("ReplicationGroupId", id),
        ("ReplicationGroupDescription", "a cluster-mode row"),
        ("Engine", engine),
        ("CacheNodeType", NODE_TYPE),
    ]);
    pairs.extend(params(extra));
    emulator.call("CreateReplicationGroup", &pairs).await
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

/// Assert `reply` reports `ClusterEnabled` as `enabled` and the mode that goes with it.
fn assert_cluster_mode(case: &str, what: &str, reply: &Reply, enabled: bool) {
    assert_eq!(
        reply.text("ReplicationGroup/ClusterEnabled"),
        Some(if enabled { "true" } else { "false" }),
        "case {case:?}: {what} must report ClusterEnabled {enabled}, got {}",
        reply.body
    );
    let mode = if enabled {
        state::SETTING_ENABLED
    } else {
        state::SETTING_DISABLED
    };
    assert_eq!(
        reply.text("ReplicationGroup/ClusterMode"),
        Some(mode),
        "case {case:?}: {what} must report ClusterMode {mode:?}, got {}",
        reply.body
    );
}

#[tokio::test]
async fn a_cluster_mode_on_parameter_group_makes_the_group_cluster_enabled() {
    let emulator = Emulator::new();
    // Every family the service publishes a cluster-mode-on default group for, with ONE
    // shard: the topology alone would leave the group unpartitioned, so the parameter group
    // is what decides.
    let families = cluster_mode_families();
    assert!(
        families.len() >= 2,
        "the service must publish cluster-mode-on defaults for more than one family, got \
         {families:?}"
    );
    for (index, family) in families.iter().enumerate() {
        let engine = state::family_engine(family);
        let case = format!("{family} with one shard");
        let id = format!("cluster-on-{index}");
        let created = create_group(
            &emulator,
            &id,
            engine,
            &[
                ("NumNodeGroups", "1"),
                ("CacheParameterGroupName", &cluster_on_group(family)),
            ],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create must answer 200, got {} {}",
            created.status, created.body
        );
        assert_cluster_mode(&case, "the create", &created, true);
        let described = describe_group(&emulator, &id).await;
        assert_cluster_mode(&case, "the stored group", &described, true);
        // What cluster mode MEANS follows with it: the one shard owns the whole keyspace,
        // and the group publishes the configuration endpoint such a client connects to.
        let whole_keyspace = state::default_slots(1, 0);
        assert_eq!(
            described.all("NodeGroup/Slots"),
            vec![whole_keyspace.as_str()],
            "case {case:?}: the shard must own the whole keyspace, got {}",
            described.body
        );
        assert!(
            described.text("ConfigurationEndpoint/Address").is_some(),
            "case {case:?}: a cluster mode enabled group publishes a configuration endpoint, \
             got {}",
            described.body
        );

        // The control: the SAME request on the family's plain default group is not cluster
        // mode enabled.
        let plain_case = format!("{family} with its plain default group");
        let plain_id = format!("cluster-off-{index}");
        let plain = create_group(
            &emulator,
            &plain_id,
            engine,
            &[
                ("NumNodeGroups", "1"),
                ("CacheParameterGroupName", &plain_group(family)),
            ],
        )
        .await;
        assert_eq!(
            plain.status, 200,
            "case {plain_case:?}: the create must answer 200, got {} {}",
            plain.status, plain.body
        );
        assert_cluster_mode(&plain_case, "the create", &plain, false);
        assert_cluster_mode(
            &plain_case,
            "the stored group",
            &describe_group(&emulator, &plain_id).await,
            false,
        );
    }
}

#[tokio::test]
async fn a_group_of_the_accounts_own_that_sets_the_parameter_counts_too() {
    // The flag is the PARAMETER's, not the name's: a group of the account's own, created on
    // a family that publishes the parameter and then set to turn cluster mode on, makes a
    // replication group cluster mode enabled exactly as the published default does. The
    // control is the same group before the parameter was set.
    let emulator = Emulator::new();
    let family = cluster_mode_families()
        .first()
        .copied()
        .expect("the service publishes a family that can run cluster mode");
    let engine = state::family_engine(family);
    for (case, group, set_parameter, enabled) in [
        ("a group of our own, parameter unset", "own-plain", false, false),
        ("a group of our own, cluster mode set on", "own-cluster-on", true, true),
    ] {
        let created_group = emulator
            .call(
                "CreateCacheParameterGroup",
                &params(&[
                    ("CacheParameterGroupName", group),
                    ("CacheParameterGroupFamily", family),
                    ("Description", "a parameter group of this suite's own"),
                ]),
            )
            .await;
        assert_eq!(
            created_group.status, 200,
            "case {case:?}: the parameter group must be created, got {} {}",
            created_group.status, created_group.body
        );
        if set_parameter {
            let modified = emulator
                .call(
                    "ModifyCacheParameterGroup",
                    &params(&[
                        ("CacheParameterGroupName", group),
                        (
                            "ParameterNameValues.ParameterNameValue.1.ParameterName",
                            state::CLUSTER_ENABLED_PARAMETER,
                        ),
                        (
                            "ParameterNameValues.ParameterNameValue.1.ParameterValue",
                            state::CLUSTER_ENABLED_ON,
                        ),
                    ]),
                )
                .await;
            assert_eq!(
                modified.status, 200,
                "case {case:?}: the parameter must be settable, got {} {}",
                modified.status, modified.body
            );
        }
        let id = format!("own-group-{group}");
        let created = create_group(
            &emulator,
            &id,
            engine,
            &[("NumNodeGroups", "1"), ("CacheParameterGroupName", group)],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create must answer 200, got {} {}",
            created.status, created.body
        );
        assert_cluster_mode(case, "the create", &created, enabled);
        assert_cluster_mode(
            case,
            "the stored group",
            &describe_group(&emulator, &id).await,
            enabled,
        );
    }
}

/// One row of the other-ways table: the case name, the group it creates, the members the
/// create carries beside the shared ones, and whether cluster mode ends up on.
type ModeRow = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    bool,
);

#[tokio::test]
async fn every_other_way_of_asking_for_cluster_mode_still_holds() {
    // The parameter group is one way among the ways the service offers, and the others are
    // unchanged: more than one shard, and `ClusterMode` named enabled.
    let emulator = Emulator::new();
    let rows: &[ModeRow] = &[
        (
            "two shards",
            "mode-two-shards",
            &[("NumNodeGroups", "2"), ("ReplicasPerNodeGroup", "1")],
            true,
        ),
        (
            "ClusterMode enabled with one shard",
            "mode-named-on",
            &[("NumNodeGroups", "1"), ("ClusterMode", state::SETTING_ENABLED)],
            true,
        ),
        (
            "nothing that asks for it",
            "mode-plain",
            &[("NumCacheClusters", "1")],
            false,
        ),
        (
            "ClusterMode named disabled",
            "mode-named-off",
            &[
                ("NumCacheClusters", "1"),
                ("ClusterMode", state::SETTING_DISABLED),
            ],
            false,
        ),
    ];
    for (case, id, extra, enabled) in rows {
        let created = create_group(&emulator, id, state::DEFAULT_ENGINE, extra).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create must answer 200, got {} {}",
            created.status, created.body
        );
        assert_cluster_mode(case, "the create", &created, *enabled);
        assert_cluster_mode(
            case,
            "the stored group",
            &describe_group(&emulator, id).await,
            *enabled,
        );
    }
}

#[tokio::test]
async fn asking_for_cluster_mode_off_and_a_group_that_turns_it_on_is_refused() {
    // The two cannot hold together, so the request is refused rather than answered 200 with
    // one of them dropped — the same refusal the other contradiction of cluster mode gets.
    let emulator = Emulator::new();
    let family = cluster_mode_families()
        .first()
        .copied()
        .expect("the service publishes a family that can run cluster mode");
    let engine = state::family_engine(family);
    let refused = create_group(
        &emulator,
        "mode-contradiction",
        engine,
        &[
            ("NumNodeGroups", "1"),
            ("CacheParameterGroupName", &cluster_on_group(family)),
            ("ClusterMode", state::SETTING_DISABLED),
        ],
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, "InvalidParameterCombination"),
        "a request that asks for cluster mode off and a group that turns it on is refused, got \
         {} {}",
        refused.status,
        refused.body
    );
    let described = describe_group(&emulator, "mode-contradiction").await;
    assert_eq!(
        described.status, 404,
        "a refused create must store nothing, got {} {}",
        described.status, described.body
    );
    // And the partitioning contradiction is refused the same way.
    let partitioned = create_group(
        &emulator,
        "mode-contradiction-shards",
        state::DEFAULT_ENGINE,
        &[
            ("NumNodeGroups", "2"),
            ("ClusterMode", state::SETTING_DISABLED),
        ],
    )
    .await;
    assert_eq!(
        (partitioned.status, partitioned.error_code()),
        (400, "InvalidParameterCombination"),
        "more than one shard cannot be combined with cluster mode off, got {} {}",
        partitioned.status,
        partitioned.body
    );
}
