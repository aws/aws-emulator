// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SPELLING of the serverless lifecycle a client polls on.
//!
//! The rule under test: every status this service reports for a resource's lifecycle is
//! reported in the spelling the API puts on the wire — lower case — whichever resource and
//! whichever phase it is, so a client's waiter (poll until `available`) converges on a
//! serverless cache and a serverless cache snapshot exactly as it does on a cache cluster
//! and a replication group.
//!
//! The spellings are asserted as LITERALS here rather than through the service's own
//! constants: a suite that compares a response against the constant the handler wrote
//! cannot tell the two spellings apart, and the wire value is the whole point.
//!
//! The tests are tables: the rows span the engines a serverless cache runs, the phase the
//! resource is in (created, settled, modified, deleted, snapshotted), the resource KIND
//! (cache, snapshot) and where the value is read from (the write's own answer, a later
//! describe), so no single recorded request is what makes them pass.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, with the shared `harness` request builder and XML reader; nothing reaches into
//! the service's state struct.

use crate::harness::{params, Emulator, Reply, PAST_TRANSITION};

/// The transient status a resource whose creation is under way reports.
const CREATING: &str = "creating";
/// The status a resource a client may use reports.
const AVAILABLE: &str = "available";
/// The transient status a resource whose reconfiguration is under way reports.
const MODIFYING: &str = "modifying";
/// The status a resource on its way out reports.
const DELETING: &str = "deleting";

/// Create one serverless cache and insist the create itself succeeded.
async fn create_cache(emulator: &Emulator, case: &str, name: &str, engine: &str) -> Reply {
    let created = emulator
        .call(
            "CreateServerlessCache",
            &params(&[("ServerlessCacheName", name), ("Engine", engine)]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreateServerlessCache must answer 200, got {} {}",
        created.status, created.body
    );
    created
}

/// `DescribeServerlessCaches` for one cache by name.
async fn describe_cache(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "DescribeServerlessCaches",
            &params(&[("ServerlessCacheName", name)]),
        )
        .await
}

/// Assert `reply` reports `expected` as the resource's `Status`.
fn assert_status(case: &str, where_from: &str, reply: &Reply, expected: &str) {
    assert_eq!(
        reply.text("Status"),
        Some(expected),
        "case {case:?} ({where_from}): expected the wire status {expected:?}, got {:?} in {}",
        reply.text("Status"),
        reply.body
    );
}

/// A serverless cache reports every phase of its lifecycle in the wire spelling, from the
/// call that started the phase and from a read inside it.
///
/// The rows span the engines a serverless cache runs — the axis the rule generalises over,
/// since the status is the engine-independent lifecycle of the resource.
#[tokio::test]
async fn a_serverless_cache_reports_its_lifecycle_in_the_wire_spelling() {
    // Source of the engine list: `CreateServerlessCache$Engine` ("The name of the cache
    // engine to be used for creating the serverless cache").
    let rows: [(&str, &str, &str); 3] = [
        ("a Redis OSS cache", "status-redis", "redis"),
        ("a Valkey cache", "status-valkey", "valkey"),
        ("a Memcached cache", "status-memcached", "memcached"),
    ];

    for (case, name, engine) in rows {
        let emulator = Emulator::new();
        let created = create_cache(&emulator, case, name, engine).await;
        // The create's own answer is the transient status: the span is never zero.
        assert_status(case, "the create's answer", &created, CREATING);
        // A read is answered once the work the clock owed is done, which is the
        // value a waiter polls for.
        assert_status(
            case,
            "a read after the create",
            &describe_cache(&emulator, name).await,
            AVAILABLE,
        );

        // A capacity change is work the service carries out after the call, so the cache
        // is transient again — in the same spelling.
        let modified = emulator
            .call(
                "ModifyServerlessCache",
                &params(&[
                    ("ServerlessCacheName", name),
                    ("CacheUsageLimits.ECPUPerSecond.Maximum", "9000"),
                ]),
            )
            .await;
        assert_eq!(
            modified.status, 200,
            "case {case:?}: ModifyServerlessCache must answer 200, got {} {}",
            modified.status, modified.body
        );
        assert_status(case, "the modify's answer", &modified, MODIFYING);
        assert_status(
            case,
            "a read after the modify",
            &describe_cache(&emulator, name).await,
            AVAILABLE,
        );

        // And the way out.
        let deleted = emulator
            .call(
                "DeleteServerlessCache",
                &params(&[("ServerlessCacheName", name)]),
            )
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: DeleteServerlessCache must answer 200, got {} {}",
            deleted.status, deleted.body
        );
        assert_status(case, "the delete's answer", &deleted, DELETING);
        // The read after it is answered once the deletion the clock owed has
        // been carried through, so there is no cache left to report.
        let gone = describe_cache(&emulator, name).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: a read after the delete must not find the cache, got {} {}",
            gone.status, gone.body
        );
    }
}

/// A snapshot of a serverless cache reports its own lifecycle in the same spelling: the
/// rule is about the lifecycle of a resource, not about one resource kind.
#[tokio::test]
async fn a_serverless_cache_snapshot_reports_its_lifecycle_in_the_wire_spelling() {
    let emulator = Emulator::new();
    create_cache(&emulator, "a cache to snapshot", "snapshot-source", "valkey").await;
    std::thread::sleep(PAST_TRANSITION);

    // The rows span the snapshot taken of a cache by name and the snapshot a second take
    // of the same cache produces, which is what a client that snapshots on a schedule
    // does.
    for (case, snapshot_name) in [
        ("the first snapshot of the cache", "first-snapshot"),
        ("a second snapshot of the same cache", "second-snapshot"),
    ] {
        let created = emulator
            .call(
                "CreateServerlessCacheSnapshot",
                &params(&[
                    ("ServerlessCacheSnapshotName", snapshot_name),
                    ("ServerlessCacheName", "snapshot-source"),
                ]),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateServerlessCacheSnapshot must answer 200, got {} {}",
            created.status, created.body
        );
        assert_status(case, "the create's answer", &created, CREATING);

        std::thread::sleep(PAST_TRANSITION);
        let described = emulator
            .call(
                "DescribeServerlessCacheSnapshots",
                &params(&[("ServerlessCacheSnapshotName", snapshot_name)]),
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: DescribeServerlessCacheSnapshots must answer 200, got {} {}",
            described.status, described.body
        );
        assert_status(
            case,
            "a read once the clock has settled it",
            &described,
            AVAILABLE,
        );
    }
}

/// The same lifecycle spelling across resource KINDS: a cache cluster, a replication group
/// and a serverless cache all report the transient phase and then the usable one in the
/// same words, so one waiter serves all three.
#[tokio::test]
async fn every_resource_kind_reports_the_same_lifecycle_spelling() {
    let emulator = Emulator::new();

    // Each row: the case, the create, its form pairs, the read that follows it with the
    // identity to read back, and the element that resource reports its lifecycle in (a
    // cache cluster reports `CacheClusterStatus`, the other two `Status`).
    type Row = (
        &'static str,
        &'static str,
        &'static [(&'static str, &'static str)],
        &'static str,
        &'static [(&'static str, &'static str)],
        &'static str,
    );
    let rows: [Row; 3] = [
        (
            "a cache cluster",
            "CreateCacheCluster",
            &[
                ("CacheClusterId", "spelling-cluster"),
                ("Engine", "redis"),
                ("CacheNodeType", "cache.t3.micro"),
                ("NumCacheNodes", "1"),
            ],
            "DescribeCacheClusters",
            &[("CacheClusterId", "spelling-cluster")],
            "CacheClusterStatus",
        ),
        (
            "a replication group",
            "CreateReplicationGroup",
            &[
                ("ReplicationGroupId", "spelling-group"),
                ("ReplicationGroupDescription", "one spelling"),
                ("Engine", "redis"),
                ("CacheNodeType", "cache.t3.micro"),
            ],
            "DescribeReplicationGroups",
            &[("ReplicationGroupId", "spelling-group")],
            "Status",
        ),
        (
            "a serverless cache",
            "CreateServerlessCache",
            &[
                ("ServerlessCacheName", "spelling-serverless"),
                ("Engine", "redis"),
            ],
            "DescribeServerlessCaches",
            &[("ServerlessCacheName", "spelling-serverless")],
            "Status",
        ),
    ];

    for (case, create, create_params, describe, describe_params, element) in rows {
        let created = emulator.call(create, &params(create_params)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: {create} must answer 200, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            created.text(element),
            Some(CREATING),
            "case {case:?} (the create's answer): expected the wire status {CREATING:?} in \
             {element}, got {:?} in {}",
            created.text(element),
            created.body
        );
        std::thread::sleep(PAST_TRANSITION);
        let described = emulator.call(describe, &params(describe_params)).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: {describe} must answer 200, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.text(element),
            Some(AVAILABLE),
            "case {case:?} (a read once the clock has settled it): expected the wire status \
             {AVAILABLE:?} in {element}, got {:?} in {}",
            described.text(element),
            described.body
        );
    }
}
