// SPDX-License-Identifier: CC0-1.0

//! The ENGINE a cache parameter group a request names has to be the engine the resource
//! runs.
//!
//! A parameter group belongs to a family, and "a family's name is the engine followed by its
//! version line" (`CacheParameterGroupFamily`), so a group's parameters are one engine's. A
//! resource running another engine could not take them, and every operation of this service
//! that NAMES a group for a resource (`CreateCacheCluster`, `CreateReplicationGroup`,
//! `ModifyCacheCluster`, `ModifyReplicationGroup`) refuses the pair with the modelled
//! `InvalidParameterCombinationException` (`awsQueryError` code
//! `InvalidParameterCombination`, HTTP 400) and the sentence
//! `<group-engine> parameter group is not applicable to engine <engine>`, writing nothing.
//!
//! The rows differ along what the rule generalises over: which engine the group's family
//! belongs to, which engine the resource runs, and whether the request creates the resource
//! or changes one that exists. The group names are taken from
//! `DescribeCacheParameterGroups` — the catalogue this account actually holds — so the table
//! cannot name a group that does not exist, and the matching pair of each row is the control
//! that shows the refusal is about the ENGINE and nothing else.

use crate::harness::{params, poll_until, Emulator, Reply, REGION};

/// The `awsQuery` error code the refusal carries, from the model:
/// `InvalidParameterCombinationException` has `awsQueryError` code
/// `InvalidParameterCombination` with HTTP 400
/// (`smithy_query(query="operation",operation="ModifyCacheCluster")`).
const REFUSAL_CODE: &str = "InvalidParameterCombination";

/// One default group per engine, with the engine its family belongs to. Each name is checked
/// against `DescribeCacheParameterGroups` before it is used.
const GROUPS: [(&str, &str); 3] = [
    ("redis", "default.redis7"),
    ("valkey", "default.valkey8"),
    ("memcached", "default.memcached1.6"),
];

/// The engines a replication group may run: "This operation is valid for Valkey and Redis
/// OSS only" (`CreateReplicationGroup`).
const REPLICATION_GROUP_ENGINES: [&str; 2] = ["redis", "valkey"];

/// Assert `group` is a parameter group this account holds, as the describe reports it.
async fn assert_published(emulator: &Emulator, group: &str) {
    let described = emulator
        .call(
            "DescribeCacheParameterGroups",
            &params(&[("CacheParameterGroupName", group)]),
        )
        .await;
    assert_eq!(
        described.status, 200,
        "the catalogue must hold {group}, got {} {}",
        described.status, described.body
    );
}

/// Assert `reply` is the modelled refusal for a group of `group_engine` named for a resource
/// running `engine`.
fn assert_not_applicable(case: &str, reply: &Reply, group_engine: &str, engine: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case}: expected HTTP 400, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        REFUSAL_CODE,
        "case {case}: expected {REFUSAL_CODE}, got {:?} ({})",
        reply.error_code(),
        reply.body
    );
    let expected = format!("{group_engine} parameter group is not applicable to engine {engine}");
    assert_eq!(
        reply.error_message(),
        expected,
        "case {case}: expected {expected:?}, got {:?}",
        reply.error_message()
    );
}

#[tokio::test]
async fn a_create_naming_a_parameter_group_of_another_engine_is_refused() {
    let emulator = Emulator::new();
    for (_, group) in GROUPS {
        assert_published(&emulator, group).await;
    }

    for (row, (group_engine, group)) in GROUPS.iter().enumerate() {
        for (column, engine) in GROUPS.iter().map(|(engine, _)| *engine).enumerate() {
            let case = format!("{group} on {engine}");
            let cluster = format!("engine-pair-{row}-{column}");
            let created = emulator
                .call(
                    "CreateCacheCluster",
                    &crate::harness::cluster_create_params(
                        &cluster,
                        &[("Engine", engine), ("CacheParameterGroupName", group)],
                    ),
                )
                .await;
            if engine == *group_engine {
                assert_eq!(
                    created.status, 200,
                    "case {case}: the group of this engine must be accepted, got {} {}",
                    created.status, created.body
                );
                let described = emulator
                    .call(
                        "DescribeCacheClusters",
                        &params(&[("CacheClusterId", &cluster)]),
                    )
                    .await;
                assert_eq!(
                    described.text("CacheParameterGroupName"),
                    Some(*group),
                    "case {case}: the cluster must carry {group}, got {}",
                    described.body
                );
            } else {
                assert_not_applicable(&case, &created, group_engine, engine);
                let described = emulator
                    .call(
                        "DescribeCacheClusters",
                        &params(&[("CacheClusterId", &cluster)]),
                    )
                    .await;
                assert_eq!(
                    (described.status, described.error_code()),
                    (404, "CacheClusterNotFound"),
                    "case {case}: a refused create must create nothing, got {} {}",
                    described.status,
                    described.body
                );
            }

            if !REPLICATION_GROUP_ENGINES.contains(&engine) {
                continue;
            }
            let replication_group = format!("engine-pair-rg-{row}-{column}");
            let created = emulator
                .call(
                    "CreateReplicationGroup",
                    &params(&[
                        ("ReplicationGroupId", &replication_group),
                        ("ReplicationGroupDescription", "the parameter-group engine"),
                        ("Engine", engine),
                        ("CacheParameterGroupName", group),
                    ]),
                )
                .await;
            if engine == *group_engine {
                assert_eq!(
                    created.status, 200,
                    "case {case}: the group of this engine must be accepted, got {} {}",
                    created.status, created.body
                );
            } else {
                assert_not_applicable(&case, &created, group_engine, engine);
                let described = emulator
                    .call(
                        "DescribeReplicationGroups",
                        &params(&[("ReplicationGroupId", &replication_group)]),
                    )
                    .await;
                assert_eq!(
                    (described.status, described.error_code()),
                    (404, "ReplicationGroupNotFoundFault"),
                    "case {case}: a refused create must create nothing, got {} {}",
                    described.status,
                    described.body
                );
            }
        }
    }
}

#[tokio::test]
async fn a_change_naming_a_parameter_group_of_another_engine_is_refused() {
    // The same rule where the resource already exists: it keeps the group it was running.
    let emulator = Emulator::new();
    let cluster = "engine-pair-cluster";
    let replication_group = "engine-pair-group";
    for (action, request) in [
        (
            "CreateCacheCluster",
            crate::harness::cluster_create_params(cluster, &[("Engine", "redis")]),
        ),
        (
            "CreateReplicationGroup",
            params(&[
                ("ReplicationGroupId", replication_group),
                ("ReplicationGroupDescription", "the parameter-group engine"),
                ("Engine", "redis"),
            ]),
        ),
    ] {
        let created = emulator.call(action, &request).await;
        assert_eq!(
            created.status, 200,
            "{action} must answer 200, got {} {}",
            created.status, created.body
        );
    }
    // A resource takes no change until the clock owes it its usable status, which is
    // reached by looking again at each resource until it reports that status rather than
    // by waiting a fixed time.
    poll_until(
        &emulator,
        REGION,
        "the engine pair",
        &format!("the cluster {cluster} settling at available"),
        "DescribeCacheClusters",
        &params(&[("CacheClusterId", cluster)]),
        |reply| reply.text("CacheCluster/CacheClusterStatus") == Some("available"),
    )
    .await;
    poll_until(
        &emulator,
        REGION,
        "the engine pair",
        &format!("the replication group {replication_group} settling at available"),
        "DescribeReplicationGroups",
        &params(&[("ReplicationGroupId", replication_group)]),
        |reply| reply.text("ReplicationGroup/Status") == Some("available"),
    )
    .await;

    // Both of the other engines' groups, against a resource running Redis OSS.
    for (group_engine, group) in GROUPS.iter().filter(|(engine, _)| *engine != "redis") {
        let case = format!("{group} on redis");
        let refused = emulator
            .call(
                "ModifyCacheCluster",
                &params(&[
                    ("CacheClusterId", cluster),
                    ("CacheParameterGroupName", group),
                    ("ApplyImmediately", "true"),
                ]),
            )
            .await;
        assert_not_applicable(&case, &refused, group_engine, "redis");
        let described = emulator
            .call("DescribeCacheClusters", &params(&[("CacheClusterId", cluster)]))
            .await;
        assert_ne!(
            described.text("CacheParameterGroupName"),
            Some(*group),
            "case {case}: a refused change must leave the group alone, got {}",
            described.body
        );

        let refused = emulator
            .call(
                "ModifyReplicationGroup",
                &params(&[
                    ("ReplicationGroupId", replication_group),
                    ("CacheParameterGroupName", group),
                    ("ApplyImmediately", "true"),
                ]),
            )
            .await;
        assert_not_applicable(&case, &refused, group_engine, "redis");
        let described = emulator
            .call(
                "DescribeCacheClusters",
                &params(&[("CacheClusterId", &format!("{replication_group}-001"))]),
            )
            .await;
        assert_ne!(
            described.text("CacheParameterGroupName"),
            Some(*group),
            "case {case}: a refused change must leave the group alone, got {}",
            described.body
        );
    }
}
