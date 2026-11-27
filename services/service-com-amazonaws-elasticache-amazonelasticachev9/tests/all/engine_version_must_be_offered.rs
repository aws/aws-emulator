// SPDX-License-Identifier: CC0-1.0

//! The engine versions a create or an upgrade may NAME: the ones the service offers.
//!
//! `DescribeCacheEngineVersions` publishes the catalogue of engines and versions this
//! account can run — AWS publishes it, no operation adds to it — and that catalogue is what
//! a client picks an `EngineVersion` from. So every operation of this service that takes an
//! `EngineVersion` (`CreateCacheCluster`, `CreateReplicationGroup`, `ModifyCacheCluster`,
//! `ModifyReplicationGroup`) accepts a version the catalogue offers for that engine — the
//! version itself, or the version LINE one belongs to, which is the same reading the
//! describe's own `EngineVersion` filter has — and refuses anything else with the modelled
//! `InvalidParameterCombinationException` (`awsQueryError` code
//! `InvalidParameterCombination`, HTTP 400) and the sentence
//! `Cannot find version <version> for <engine>`, writing nothing.
//!
//! The rows differ along what the rule generalises over: the engine, whether the value is a
//! published version, a version line, a version of ANOTHER engine, a prefix of a published
//! version that is not a line of it, or a version past everything published; and whether the
//! operation creates the resource or changes one that exists. Every "offered" row is checked
//! against what the describe ANSWERS first, so the table cannot drift from the published
//! catalogue, and every "absent" row is checked to be absent from it.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, Reply, PAST_TRANSITION};

/// The `awsQuery` error code the refusal carries, from the model:
/// `InvalidParameterCombinationException` has `awsQueryError` code
/// `InvalidParameterCombination` with HTTP 400
/// (`smithy_query(query="operation",operation="CreateCacheCluster")`).
const REFUSAL_CODE: &str = "InvalidParameterCombination";

/// Values that are NOT engine versions the catalogue offers, each for a different reason —
/// a version of another engine, a prefix that is not a version line, and a version past
/// everything published. Each is checked against `DescribeCacheEngineVersions` before it is
/// used, so a catalogue that later publishes one of them fails this suite rather than
/// silently weakening it.
const ABSENT: [(&str, &str); 5] = [
    // The recorded case: Valkey's published line does not reach back this far.
    ("valkey", "7.0"),
    // A Redis OSS version Valkey never had.
    ("valkey", "6.2"),
    // Valkey's version asked of Redis OSS.
    ("redis", "8.0"),
    // A PREFIX of a published Memcached version (`1.4.14`) that is not a version line of
    // it: a line is delimited by a dot.
    ("memcached", "1.4.1"),
    // Past everything any engine publishes.
    ("redis", "9.9"),
];

/// Values that ARE offered, spanning an exact published version per engine and a version
/// LINE that names several published patches.
const OFFERED: [(&str, &str); 5] = [
    ("redis", "7.1"),
    // The line `6.2`, which the catalogue publishes patches of.
    ("redis", "6.2"),
    ("memcached", "1.6.22"),
    ("valkey", "8.0"),
    ("valkey", "7.2"),
];

/// The versions `DescribeCacheEngineVersions` reports for `engine`.
async fn published(emulator: &Emulator, engine: &str) -> Vec<String> {
    let reply = emulator
        .call(
            "DescribeCacheEngineVersions",
            &params(&[("Engine", engine), ("MaxRecords", "100")]),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "DescribeCacheEngineVersions must answer 200 for {engine}, got {} {}",
        reply.status, reply.body
    );
    reply
        .all("EngineVersion")
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Whether `version` names something the catalogue offers for `engine`: a published version,
/// or the leading components of one ("a value that names only the leading components of a
/// version names that whole line", as the describe's own `EngineVersion` filter reads it).
fn offers(catalogue: &[String], version: &str) -> bool {
    catalogue
        .iter()
        .any(|published| published == version || published.starts_with(&format!("{version}.")))
}

/// Create one cache cluster.
async fn create_cluster(emulator: &Emulator, id: &str, pairs: &[(&str, &str)]) -> Reply {
    let request = crate::harness::cluster_create_params(id, pairs);
    emulator.call("CreateCacheCluster", &request).await
}

/// Create one replication group.
async fn create_group(emulator: &Emulator, id: &str, pairs: &[(&str, &str)]) -> Reply {
    let mut request = params(&[
        ("ReplicationGroupId", id),
        ("ReplicationGroupDescription", "the engine-version catalogue"),
    ]);
    request.extend(params(pairs));
    emulator.call("CreateReplicationGroup", &request).await
}

/// Assert `reply` is the modelled refusal for a version the engine does not offer.
fn assert_not_offered(case: &str, reply: &Reply, engine: &str, version: &str) {
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
    let expected = format!("Cannot find version {version} for {engine}");
    assert_eq!(
        reply.error_message(),
        expected,
        "case {case}: expected {expected:?}, got {:?}",
        reply.error_message()
    );
}

#[tokio::test]
async fn a_version_the_catalogue_offers_is_accepted_and_one_it_does_not_is_refused() {
    let emulator = Emulator::new();

    // The table is held to the catalogue the service itself publishes, both ways.
    for (engine, version) in OFFERED {
        let catalogue = published(&emulator, engine).await;
        assert!(
            offers(&catalogue, version),
            "case {engine} {version}: the catalogue must offer it, got {catalogue:?}"
        );
    }
    for (engine, version) in ABSENT {
        let catalogue = published(&emulator, engine).await;
        assert!(
            !offers(&catalogue, version),
            "case {engine} {version}: the catalogue must NOT offer it, got {catalogue:?}"
        );
    }

    for (index, (engine, version)) in OFFERED.iter().enumerate() {
        let cluster = format!("offered-cluster-{index}");
        let created = create_cluster(
            &emulator,
            &cluster,
            &[("Engine", engine), ("EngineVersion", version)],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {engine} {version}: CreateCacheCluster must answer 200, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            created.text("CacheCluster/EngineVersion"),
            Some(state::patch_release(engine, version)),
            "case {engine} {version}: the cluster must report the release behind the version it \
             was created on, got {:?}",
            created.text("CacheCluster/EngineVersion")
        );
        // Memcached has no replication groups; the other two engines do.
        if *engine != "memcached" {
            let group = format!("offered-group-{index}");
            let created = create_group(
                &emulator,
                &group,
                &[("Engine", engine), ("EngineVersion", version)],
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {engine} {version}: CreateReplicationGroup must answer 200, got {} {}",
                created.status, created.body
            );
        }
    }

    for (index, (engine, version)) in ABSENT.iter().enumerate() {
        let case = format!("{engine} {version}");
        let cluster = format!("absent-cluster-{index}");
        let refused = create_cluster(
            &emulator,
            &cluster,
            &[("Engine", engine), ("EngineVersion", version)],
        )
        .await;
        assert_not_offered(
            &format!("CreateCacheCluster {case}"),
            &refused,
            engine,
            version,
        );
        // Nothing was written: the cluster the refused create named does not exist.
        let read = emulator
            .call(
                "DescribeCacheClusters",
                &params(&[("CacheClusterId", &cluster)]),
            )
            .await;
        assert_eq!(
            (read.status, read.error_code()),
            (404, "CacheClusterNotFound"),
            "case {case}: a refused create must create nothing, got {} {}",
            read.status,
            read.body
        );

        let group = format!("absent-group-{index}");
        if *engine == "memcached" {
            // A replication group runs Valkey or Redis OSS only, so a Memcached row has no
            // replication-group half: the engine is refused before any version is read.
            continue;
        }
        let refused = create_group(
            &emulator,
            &group,
            &[("Engine", engine), ("EngineVersion", version)],
        )
        .await;
        assert_not_offered(
            &format!("CreateReplicationGroup {case}"),
            &refused,
            engine,
            version,
        );
        let read = emulator
            .call(
                "DescribeReplicationGroups",
                &params(&[("ReplicationGroupId", &group)]),
            )
            .await;
        assert_eq!(
            (read.status, read.error_code()),
            (404, "ReplicationGroupNotFoundFault"),
            "case {case}: a refused create must create nothing, got {} {}",
            read.status,
            read.body
        );
    }
}

#[tokio::test]
async fn an_upgrade_to_a_version_the_catalogue_does_not_offer_is_refused() {
    // The same rule where the resource already exists: the version a change NAMES is held
    // to the catalogue, and the resource keeps the version it was running.
    let emulator = Emulator::new();
    // Each resource is created on its engine's own default version, so no row is refused
    // for naming a version the catalogue does offer.
    for (index, (engine, _)) in ABSENT.iter().enumerate() {
        let created = create_cluster(
            &emulator,
            &format!("upgrade-cluster-{index}"),
            &[("Engine", engine)],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "the row {index} needs its cluster, got {} {}",
            created.status, created.body
        );
        if *engine != "memcached" {
            let created = create_group(
                &emulator,
                &format!("upgrade-group-{index}"),
                &[("Engine", engine)],
            )
            .await;
            assert_eq!(
                created.status, 200,
                "the row {index} needs its replication group, got {} {}",
                created.status, created.body
            );
        }
    }
    // A resource takes no change until the clock owes it its usable status.
    std::thread::sleep(PAST_TRANSITION);

    for (index, (engine, version)) in ABSENT.iter().enumerate() {
        let case = format!("{engine} {version}");
        let cluster = format!("upgrade-cluster-{index}");
        let refused = emulator
            .call(
                "ModifyCacheCluster",
                &params(&[
                    ("CacheClusterId", &cluster),
                    ("EngineVersion", version),
                    ("ApplyImmediately", "true"),
                ]),
            )
            .await;
        assert_not_offered(
            &format!("ModifyCacheCluster {case}"),
            &refused,
            engine,
            version,
        );
        let read = emulator
            .call(
                "DescribeCacheClusters",
                &params(&[("CacheClusterId", &cluster)]),
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {case}: the cluster must still be there, got {} {}",
            read.status, read.body
        );
        assert_ne!(
            read.text("CacheCluster/EngineVersion"),
            Some(*version),
            "case {case}: a refused upgrade must leave the running version alone, got {}",
            read.body
        );

        if *engine != "memcached" {
            let group = format!("upgrade-group-{index}");
            let refused = emulator
                .call(
                    "ModifyReplicationGroup",
                    &params(&[
                        ("ReplicationGroupId", &group),
                        ("EngineVersion", version),
                        ("ApplyImmediately", "true"),
                    ]),
                )
                .await;
            assert_not_offered(
                &format!("ModifyReplicationGroup {case}"),
                &refused,
                engine,
                version,
            );
        }
    }
}
