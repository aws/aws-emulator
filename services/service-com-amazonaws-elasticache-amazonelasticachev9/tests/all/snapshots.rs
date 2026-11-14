// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ElastiCache snapshot slice: `CreateSnapshot` takes
//! a copy of a stored cluster and `DescribeSnapshots` reports the stored
//! copies.
//!
//! Every assertion goes over the wire through the shared [`crate::harness`]:
//! an `awsQuery` form-urlencoded request is dispatched through the production
//! router and only the HTTP status and the XML body are inspected. Nothing
//! reaches into the service's state struct, so a response that is not computed
//! from persisted state cannot pass.
//!
//! The tests are tables: each row is one input along the axis the rule under
//! test generalises over (engine, node count, port, retention settings, filter
//! member, page size, name form), never a single recorded case.

use crate::cache_cluster_lifecycle::{
    create_available_cluster, create_cluster, describe_cluster,
};
use crate::harness::{params, poll_until, Emulator, RefusalCase, Reply, PAST_TRANSITION, REGION};

/// One filter row: the case name, the filter's form pairs, and the snapshots it
/// selects in the order the describe reports them.
type FilterCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    Vec<&'static str>,
);

/// Take one snapshot named `name` of `cluster`, with whatever else `extra`
/// names.
pub(crate) async fn create_snapshot(
    emulator: &Emulator,
    name: &str,
    cluster: &str,
    extra: &[(&str, &str)],
) -> Reply {
    let mut pairs = params(&[("SnapshotName", name), ("CacheClusterId", cluster)]);
    pairs.extend(params(extra));
    emulator.call("CreateSnapshot", &pairs).await
}

/// Describe snapshots under `filters`.
pub(crate) async fn describe_snapshots(emulator: &Emulator, filters: &[(&str, &str)]) -> Reply {
    emulator.call("DescribeSnapshots", &params(filters)).await
}

/// The `SnapshotName`s a describe reported, in the order it reported them.
pub(crate) fn reported_names(reply: &Reply) -> Vec<&str> {
    reply.all("Snapshot/SnapshotName")
}

/// One row of the snapshot round-trip table: the cluster to snapshot, and what
/// the snapshot of it must report.
#[derive(Debug, Clone)]
struct SnapshotCase {
    /// The case name, and the `SnapshotName` the create is asked for.
    requested_name: &'static str,
    /// The identity the snapshot is stored and reported under: `SnapshotName`
    /// "is stored as a lowercase string".
    stored_name: &'static str,
    cluster: &'static str,
    /// The form pairs the source cluster is created with.
    cluster_extra: &'static [(&'static str, &'static str)],
    /// The form pairs sent beside `SnapshotName` / `CacheClusterId`.
    snapshot_extra: &'static [(&'static str, &'static str)],
    engine: &'static str,
    engine_version: &'static str,
    num_cache_nodes: &'static str,
    parameter_group: &'static str,
    /// `Snapshot$SnapshotRetentionLimit` "reflects the SnapshotRetentionLimit
    /// for the source cluster when the snapshot was created".
    retention_limit: &'static str,
    /// The source cluster's `SnapshotWindow`, when it has one.
    snapshot_window: Option<&'static str>,
    kms_key_id: Option<&'static str>,
}

#[tokio::test]
async fn a_created_snapshot_is_read_back_from_persisted_state() {
    // The axis: the source cluster's configuration (engine and its documented
    // defaults, the port, the backup settings) and the snapshot's own members
    // (name case, KMS key), because every value a snapshot reports is a copy of
    // one of those.
    let cases = [
        SnapshotCase {
            requested_name: "redis-default-snap",
            stored_name: "redis-default-snap",
            cluster: "snap-redis",
            cluster_extra: &[("CacheNodeType", "cache.t3.micro")],
            snapshot_extra: &[],
            engine: "redis",
            engine_version: "7.1",
            num_cache_nodes: "1",
            parameter_group: "default.redis7",
            // "Default: 0 (i.e., automatic backups are disabled for this cache
            // cluster)".
            retention_limit: "0",
            snapshot_window: None,
            kms_key_id: None,
        },
        SnapshotCase {
            requested_name: "Mixed-Case-Snap",
            stored_name: "mixed-case-snap",
            cluster: "snap-valkey",
            cluster_extra: &[
                ("Engine", "valkey"),
                ("Port", "7000"),
                ("SnapshotRetentionLimit", "5"),
                ("SnapshotWindow", "05:00-09:00"),
            ],
            snapshot_extra: &[("KmsKeyId", "alias/aws-elasticache")],
            engine: "valkey",
            engine_version: "9.0",
            num_cache_nodes: "1",
            parameter_group: "default.valkey9",
            retention_limit: "5",
            snapshot_window: Some("05:00-09:00"),
            kms_key_id: Some("alias/aws-elasticache"),
        },
        SnapshotCase {
            requested_name: "tagged-snap",
            stored_name: "tagged-snap",
            cluster: "snap-redis-six",
            cluster_extra: &[("EngineVersion", "6.2"), ("SnapshotRetentionLimit", "1")],
            snapshot_extra: &[("Tags.Tag.1.Key", "team"), ("Tags.Tag.1.Value", "cache")],
            engine: "redis",
            engine_version: "6.2",
            num_cache_nodes: "1",
            // Redis OSS 6 is the one published parameter group family whose
            // version line is `<major>.x` rather than `<major>`
            // (`state::PARAMETER_GROUP_FAMILIES`).
            parameter_group: "default.redis6.x",
            retention_limit: "1",
            snapshot_window: None,
            kms_key_id: None,
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        create_available_cluster(&emulator, case.cluster, case.cluster_extra).await;

        let created = create_snapshot(
            &emulator,
            case.requested_name,
            case.cluster,
            case.snapshot_extra,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {}: expected HTTP 200, got {} {}",
            case.requested_name, created.status, created.body
        );
        // A created snapshot is NOT ready: it answers `creating` first.
        assert_eq!(
            created.text("Snapshot/SnapshotStatus"),
            Some("creating"),
            "case {}: a just-created snapshot must report creating, got {}",
            case.requested_name,
            created.body
        );

        // Every value the create answered with, and then the same values read
        // back through the describe: the response has to come from the store.
        assert_eq!(
            created.text("Snapshot/SnapshotName"),
            Some(case.stored_name),
            "case {}: expected the stored lowercase name, got {}",
            case.requested_name,
            created.body
        );

        // Read back once the clock owes the snapshot `available` — waited for by looking
        // again at the condition rather than for a fixed time.
        let described = poll_until(
            &emulator,
            REGION,
            case.requested_name,
            "the snapshot reaching available",
            "DescribeSnapshots",
            &params(&[("SnapshotName", case.requested_name), ("MaxRecords", "20")]),
            |reply| reply.text("Snapshot/SnapshotStatus") == Some("available"),
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {}: describe expected HTTP 200, got {} {}",
            case.requested_name, described.status, described.body
        );
        assert_eq!(
            reported_names(&described),
            vec![case.stored_name],
            "case {}: the describe must report exactly the stored snapshot, got {}",
            case.requested_name,
            described.body
        );
        // The clock owed the snapshot `available`, and only the status moved.
        assert_eq!(
            described.text("Snapshot/SnapshotStatus"),
            Some("available"),
            "case {}: the snapshot must reach available on the clock, got {}",
            case.requested_name,
            described.body
        );

        let expected_arn = format!(":snapshot:{}", case.stored_name);
        // The port the SOURCE cluster publishes, which is what the snapshot of it reports:
        // read back from that cluster's own describe rather than assumed.
        let source_port = crate::cache_cluster_lifecycle::describe_cluster(&emulator, case.cluster)
            .await
            .text("ConfigurationEndpoint/Port")
            .map(str::to_string);
        for (phase, reply) in [("create", &created), ("describe", &described)] {
            for (element, expected) in [
                ("Snapshot/CacheClusterId", Some(case.cluster)),
                ("Snapshot/SnapshotSource", Some("manual")),
                ("Snapshot/Engine", Some(case.engine)),
                ("Snapshot/EngineVersion", Some(case.engine_version)),
                ("Snapshot/NumCacheNodes", Some(case.num_cache_nodes)),
                (
                    "Snapshot/CacheParameterGroupName",
                    Some(case.parameter_group),
                ),
                (
                    "Snapshot/SnapshotRetentionLimit",
                    Some(case.retention_limit),
                ),
                ("Snapshot/SnapshotWindow", case.snapshot_window),
                ("Snapshot/KmsKeyId", case.kms_key_id),
            ] {
                assert_eq!(
                    reply.text(element),
                    expected,
                    "case {} ({phase}): expected {element} = {expected:?}, got {}",
                    case.requested_name,
                    reply.body
                );
            }
            // `Snapshot$Port` is "the port number used by each cache nodes in the
            // source cluster", which for a cluster this emulator serves a RESP
            // data plane for is the port its own socket answers on — so it is
            // asserted against the port the SOURCE CLUSTER publishes, read back
            // from that cluster's own describe, rather than as a literal.
            assert_eq!(
                reply.text("Snapshot/Port"),
                source_port.as_deref(),
                "case {} ({phase}): expected the source cluster's own port {source_port:?}, got \
                 {:?}",
                case.requested_name,
                reply.text("Snapshot/Port")
            );
            assert!(
                reply
                    .text("Snapshot/ARN")
                    .is_some_and(|arn| arn.ends_with(&expected_arn)),
                "case {} ({phase}): expected an ARN ending in {expected_arn}, got {:?} in {}",
                case.requested_name,
                reply.text("Snapshot/ARN"),
                reply.body
            );
            // One `NodeSnapshot` per node of the source cluster, naming that
            // cluster and the node it was copied from.
            assert_eq!(
                reply.all("NodeSnapshot/CacheNodeId"),
                vec!["0001"],
                "case {} ({phase}): expected one node snapshot per cache node, got {}",
                case.requested_name,
                reply.body
            );
            assert_eq!(
                reply.all("NodeSnapshot/CacheClusterId"),
                vec![case.cluster],
                "case {} ({phase}): a node snapshot must name its source cluster, got {}",
                case.requested_name,
                reply.body
            );
            // The emulator serves no cache data plane, so there is no cached
            // data whose size it could report rather than invent.
            assert!(
                reply.text("NodeSnapshot/CacheSize").is_none(),
                "case {} ({phase}): no CacheSize may be claimed, got {}",
                case.requested_name,
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn describe_filters_select_the_stored_snapshots() {
    let emulator = Emulator::new();
    // Three clusters, three snapshots, one of the clusters snapshotted twice
    // and one not snapshotted at all, so every filter below has something it
    // must keep and something it must drop.
    for cluster in ["c-one", "c-two", "c-three"] {
        let created = create_cluster(&emulator, cluster, &[]).await;
        assert_eq!(
            created.status, 200,
            "case {cluster}: create must succeed, got {}",
            created.body
        );
    }
    std::thread::sleep(PAST_TRANSITION);
    for (name, cluster) in [("snap-a", "c-one"), ("snap-c", "c-two")] {
        let created = create_snapshot(&emulator, name, cluster, &[]).await;
        assert_eq!(
            created.status, 200,
            "case {name}: create must succeed, got {}",
            created.body
        );
    }
    // The source cluster is `snapshotting` until the clock owes it `available`
    // again, so a second snapshot of `c-one` waits out that span.
    std::thread::sleep(PAST_TRANSITION);
    let second = create_snapshot(&emulator, "snap-b", "c-one", &[]).await;
    assert_eq!(
        second.status, 200,
        "case snap-b: create must succeed, got {}",
        second.body
    );

    // Each row: the filter, and the snapshots it selects — in the stable total
    // order the describe reports, which is by stored name.
    let cases: [FilterCase; 13] = [
        ("no filter", &[], vec!["snap-a", "snap-b", "snap-c"]),
        (
            "CacheClusterId",
            &[("CacheClusterId", "c-one")],
            vec!["snap-a", "snap-b"],
        ),
        (
            "CacheClusterId is not case sensitive",
            &[("CacheClusterId", "C-ONE")],
            vec!["snap-a", "snap-b"],
        ),
        (
            "a cluster with no snapshots",
            &[("CacheClusterId", "c-three")],
            vec![],
        ),
        (
            "SnapshotName, mixed case",
            &[("SnapshotName", "SNAP-C")],
            vec!["snap-c"],
        ),
        (
            "SnapshotSource=user shows the manually created ones",
            &[("SnapshotSource", "user")],
            vec!["snap-a", "snap-b", "snap-c"],
        ),
        (
            "SnapshotSource=system shows the automatic ones",
            &[("SnapshotSource", "system")],
            vec![],
        ),
        (
            "SnapshotSource=manual is the spelling the response reports",
            &[("SnapshotSource", "manual")],
            vec!["snap-a", "snap-b", "snap-c"],
        ),
        (
            "SnapshotSource=automated is the spelling the response reports",
            &[("SnapshotSource", "automated")],
            vec![],
        ),
        (
            "SnapshotSource is not case sensitive",
            &[("SnapshotSource", "Manual")],
            vec!["snap-a", "snap-b", "snap-c"],
        ),
        (
            "a SnapshotName the store does not hold selects nothing",
            &[("SnapshotName", "no-such-snap")],
            vec![],
        ),
        (
            "ReplicationGroupId selects snapshots of a replication group",
            &[("ReplicationGroupId", "rg-1")],
            vec![],
        ),
        (
            "filters combine",
            &[("CacheClusterId", "c-one"), ("SnapshotSource", "system")],
            vec![],
        ),
    ];

    for (case, filters, expected) in cases {
        let reply = describe_snapshots(&emulator, filters).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reported_names(&reply),
            expected,
            "case {case}: expected {expected:?}, got {}",
            reply.body
        );
        assert!(
            reply.marker().is_none(),
            "case {case}: a page that returned everything must carry no Marker, got {}",
            reply.body
        );
    }

    // `ShowNodeGroupConfig`: the node group (shard) configuration is reported
    // only when it was asked for — and a snapshot of a cache cluster belongs to
    // no node group, so there is none to report either way.
    for show in ["true", "false"] {
        let reply = describe_snapshots(&emulator, &[("ShowNodeGroupConfig", show)]).await;
        assert_eq!(
            reply.status, 200,
            "case ShowNodeGroupConfig={show}: expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
        assert!(
            reply.text("NodeGroupConfiguration/NodeGroupId").is_none(),
            "case ShowNodeGroupConfig={show}: a cache cluster's snapshot has no shard \
             configuration, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_describe_naming_what_the_store_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "described", &[]).await;
    let created = create_snapshot(&emulator, "held-snap", "described", &[]).await;
    assert_eq!(
        created.status, 200,
        "the snapshot must be created: {}",
        created.body
    );

    // Each row: the filter, and the HTTP status plus `awsQuery` error code it
    // must be answered with. `MaxRecords` walks its published bounds
    // ("Default: 50. Constraints: minimum 20; maximum 50") and one past each.
    let cases: [RefusalCase; 9] = [
        (
            "unknown cluster id",
            &[("CacheClusterId", "no-such-cluster")],
            404,
            "CacheClusterNotFound",
        ),
        (
            "a SnapshotSource in neither vocabulary",
            &[("SnapshotSource", "scheduled")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a Marker this service did not mint",
            &[("Marker", "not-a-marker")],
            400,
            "InvalidParameterValue",
        ),
        (
            "MaxRecords one below the minimum",
            &[("MaxRecords", "19")],
            400,
            "InvalidParameterValue",
        ),
        (
            "MaxRecords at the minimum",
            &[("MaxRecords", "20")],
            200,
            "",
        ),
        (
            "MaxRecords at the maximum",
            &[("MaxRecords", "50")],
            200,
            "",
        ),
        (
            "MaxRecords one above the maximum",
            &[("MaxRecords", "51")],
            400,
            "InvalidParameterValue",
        ),
        (
            "MaxRecords the cluster describes accept but this one does not",
            &[("MaxRecords", "100")],
            400,
            "InvalidParameterValue",
        ),
        (
            "MaxRecords zero",
            &[("MaxRecords", "0")],
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, filters, status, code) in cases {
        let reply = describe_snapshots(&emulator, filters).await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case}: expected error code {code:?}, got {}",
            reply.body
        );
        if status == 200 {
            assert_eq!(
                reported_names(&reply),
                vec!["held-snap"],
                "case {case}: an accepted page must report the stored snapshot, got {}",
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn snapshot_pages_are_bounded_and_resumable() {
    let emulator = Emulator::new();
    // 21 snapshots: more than one page at the smallest page size the operation
    // serves. Each snapshot needs its own available cluster, because a cluster
    // is `snapshotting` for a span after each snapshot of it.
    let total = 21;
    for index in 0..total {
        let id = format!("paged-{index:02}");
        let created = create_cluster(&emulator, &id, &[]).await;
        assert_eq!(
            created.status, 200,
            "case {id}: create must succeed, got {}",
            created.body
        );
    }
    std::thread::sleep(PAST_TRANSITION);
    for index in 0..total {
        let id = format!("paged-{index:02}");
        let created = create_snapshot(&emulator, &format!("snap-{index:02}"), &id, &[]).await;
        assert_eq!(
            created.status, 200,
            "case {id}: snapshot must succeed, got {}",
            created.body
        );
    }

    let first = describe_snapshots(&emulator, &[("MaxRecords", "20")]).await;
    assert_eq!(
        first.status, 200,
        "case page 1: expected HTTP 200, got {} {}",
        first.status, first.body
    );
    let mut seen: Vec<String> = reported_names(&first)
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(
        seen.len(),
        20,
        "case page 1: expected 20 snapshots, got {seen:?}"
    );
    let marker = first
        .marker()
        .unwrap_or_else(|| panic!("case page 1: more records exist, so a Marker must be returned: {}", first.body))
        .to_string();

    let second = describe_snapshots(&emulator, &[("MaxRecords", "20"), ("Marker", &marker)]).await;
    assert_eq!(
        second.status, 200,
        "case page 2: expected HTTP 200, got {} {}",
        second.status, second.body
    );
    seen.extend(reported_names(&second).into_iter().map(str::to_string));
    assert!(
        second.marker().is_none(),
        "case page 2: the last page must carry no Marker, got {}",
        second.body
    );

    // Complete and without repeats: the two pages are exactly the stored set.
    let expected: Vec<String> = (0..total).map(|index| format!("snap-{index:02}")).collect();
    assert_eq!(
        seen, expected,
        "case pagination: the pages together must be the stored snapshots, got {seen:?}"
    );
}

#[tokio::test]
async fn a_snapshot_of_what_cannot_be_snapshotted_is_refused() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "snapshottable", &[]).await;
    create_available_cluster(&emulator, "memcached-cluster", &[("Engine", "memcached")]).await;
    create_available_cluster(
        &emulator,
        "tiny-cluster",
        &[("CacheNodeType", "cache.t1.micro")],
    )
    .await;
    let taken = create_snapshot(&emulator, "taken-snap", "snapshottable", &[]).await;
    assert_eq!(
        taken.status, 200,
        "the first snapshot must be created: {}",
        taken.body
    );
    // A cluster that is still `creating` has nothing settled to copy.
    let creating = create_cluster(&emulator, "still-creating", &[]).await;
    assert_eq!(
        creating.status, 200,
        "the cluster must be created: {}",
        creating.body
    );

    // Each row: the form pairs sent beside `SnapshotName`, and the modelled
    // refusal. The axis is the reason the snapshot cannot be taken.
    let cases: [RefusalCase; 8] = [
        (
            "unknown cluster",
            &[("CacheClusterId", "no-such-cluster")],
            404,
            "CacheClusterNotFound",
        ),
        (
            "a Memcached cluster",
            &[("CacheClusterId", "memcached-cluster")],
            400,
            "SnapshotFeatureNotSupportedFault",
        ),
        (
            "a node type that does not support snapshots",
            &[("CacheClusterId", "tiny-cluster")],
            400,
            "SnapshotFeatureNotSupportedFault",
        ),
        (
            "a cluster that is not available yet",
            &[("CacheClusterId", "still-creating")],
            400,
            "InvalidCacheClusterState",
        ),
        (
            "a cluster that is still snapshotting",
            &[("CacheClusterId", "snapshottable")],
            400,
            "InvalidCacheClusterState",
        ),
        (
            "both sources named",
            &[
                ("CacheClusterId", "snapshottable"),
                ("ReplicationGroupId", "rg-1"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        ("no source named", &[], 400, "InvalidParameterCombination"),
        (
            "an unknown replication group",
            &[("ReplicationGroupId", "rg-1")],
            404,
            "ReplicationGroupNotFoundFault",
        ),
    ];

    for (case, extra, status, code) in cases {
        let mut pairs = params(&[("SnapshotName", "refused-snap")]);
        pairs.extend(params(extra));
        let reply = emulator.call("CreateSnapshot", &pairs).await;
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
    }

    // A name already taken is refused rather than overwriting the snapshot
    // under it.
    let duplicate = create_snapshot(&emulator, "Taken-Snap", "snapshottable", &[]).await;
    assert_eq!(
        duplicate.status, 400,
        "case duplicate name: expected HTTP 400, got {} {}",
        duplicate.status, duplicate.body
    );
    assert_eq!(
        duplicate.error_code(),
        "SnapshotAlreadyExistsFault",
        "case duplicate name: expected SnapshotAlreadyExistsFault, got {}",
        duplicate.body
    );

    // Not one refusal added a record: the store still holds only the snapshot
    // that was taken.
    let described = describe_snapshots(&emulator, &[]).await;
    assert_eq!(
        reported_names(&described),
        vec!["taken-snap"],
        "a refused create must persist nothing, got {}",
        described.body
    );
}

#[tokio::test]
async fn the_documented_snapshot_name_form_is_enforced() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "named", &[]).await;

    // The axis: the name form. The documented rule is the character set and the
    // 255-character bound, so the row at the bound is accepted and the row one
    // past it is refused — and a hyphen's placement, which no ElastiCache
    // documentation constrains for a snapshot name, is accepted.
    let at_bound = "n".repeat(255);
    let past_bound = "n".repeat(256);
    let cases: [(&str, &str, u16, &str); 6] = [
        ("a plain name", "plain-snap", 200, ""),
        ("digits and hyphens", "20260101-snap-2", 200, ""),
        ("two consecutive hyphens", "ok--snap", 200, ""),
        ("a trailing hyphen", "ok-snap-", 200, ""),
        (
            "a character that is neither alphanumeric nor a hyphen",
            "bad.snap",
            400,
            "InvalidParameterValue",
        ),
        (
            "one character past the 255-character bound",
            past_bound.as_str(),
            400,
            "InvalidParameterValue",
        ),
    ];

    let mut accepted: Vec<String> = Vec::new();
    for (case, name, status, code) in cases
        .into_iter()
        .chain([("at the 255-character bound", at_bound.as_str(), 200u16, "")])
    {
        // Each accepted row needs the cluster settled again, because the one
        // before it left the cluster `snapshotting`.
        std::thread::sleep(PAST_TRANSITION);
        let reply = create_snapshot(&emulator, name, "named", &[]).await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case}: expected error code {code:?}, got {}",
            reply.body
        );
        if status == 200 {
            accepted.push(name.to_ascii_lowercase());
        }
    }

    accepted.sort();
    let described = describe_snapshots(&emulator, &[]).await;
    assert_eq!(
        reported_names(&described),
        accepted,
        "only the accepted names may be stored, got {}",
        described.body
    );
}

#[tokio::test]
async fn a_snapshot_is_the_cluster_as_it_stood_when_it_was_taken() {
    let emulator = Emulator::new();
    create_available_cluster(
        &emulator,
        "copied",
        &[("SnapshotRetentionLimit", "1"), ("SnapshotWindow", "05:00-09:00")],
    )
    .await;

    let created = create_snapshot(&emulator, "before-change", "copied", &[]).await;
    assert_eq!(
        created.status, 200,
        "case before-change: expected HTTP 200, got {} {}",
        created.status, created.body
    );
    // Taking a snapshot leaves the source cluster `snapshotting` for a span:
    // `CreateSnapshot` answers with the snapshot rather than the cluster, so
    // what shows the cluster is transient is the state gate — a second snapshot
    // of it, straight after, is refused with the status it has. The read that
    // follows is answered once the cluster is available again.
    let while_snapshotting = create_snapshot(&emulator, "during", "copied", &[]).await;
    assert_eq!(
        while_snapshotting.error_code(),
        "InvalidCacheClusterState",
        "case before-change: a snapshotting cluster refuses a second snapshot, got {} {}",
        while_snapshotting.status, while_snapshotting.body
    );
    assert!(
        while_snapshotting.error_message().contains("snapshotting"),
        "case before-change: the refusal must name the status the cluster has, got {:?}",
        while_snapshotting.error_message()
    );
    let settled = describe_cluster(&emulator, "copied").await;
    assert_eq!(
        settled.text("CacheClusterStatus"),
        Some("available"),
        "case before-change: the source cluster must return to available, got {}",
        settled.body
    );

    // The source cluster's backup settings change...
    let modified = emulator
        .call(
            "ModifyCacheCluster",
            &params(&[
                ("CacheClusterId", "copied"),
                ("SnapshotRetentionLimit", "7"),
                ("SnapshotWindow", "10:00-11:00"),
            ]),
        )
        .await;
    assert_eq!(
        modified.status, 200,
        "the modify must succeed, got {} {}",
        modified.status, modified.body
    );

    // ... and the snapshot already taken still reports what the cluster was.
    let before = describe_snapshots(&emulator, &[("SnapshotName", "before-change")]).await;
    for (element, expected) in [
        ("Snapshot/SnapshotRetentionLimit", "1"),
        ("Snapshot/SnapshotWindow", "05:00-09:00"),
    ] {
        assert_eq!(
            before.text(element),
            Some(expected),
            "case before-change: expected {element} = {expected}, got {}",
            before.body
        );
    }

    // A snapshot taken after the change reports the changed cluster.
    let after_create = create_snapshot(&emulator, "after-change", "copied", &[]).await;
    assert_eq!(
        after_create.status, 200,
        "case after-change: expected HTTP 200, got {} {}",
        after_create.status, after_create.body
    );
    for (element, expected) in [
        ("Snapshot/SnapshotRetentionLimit", "7"),
        ("Snapshot/SnapshotWindow", "10:00-11:00"),
    ] {
        assert_eq!(
            after_create.text(element),
            Some(expected),
            "case after-change: expected {element} = {expected}, got {}",
            after_create.body
        );
    }

    // Deleting the source cluster leaves its snapshots: a snapshot outlives the
    // cluster it was taken from, and the describe still resolves that id. The
    // read first, because a delete needs the cluster settled and the read is
    // answered once the snapshot it was taking is over.
    let settled_again = describe_cluster(&emulator, "copied").await;
    assert_eq!(
        settled_again.text("CacheClusterStatus"),
        Some("available"),
        "the source cluster must be available again before the delete, got {}",
        settled_again.body
    );
    let deleted = emulator
        .call("DeleteCacheCluster", &params(&[("CacheClusterId", "copied")]))
        .await;
    assert_eq!(
        deleted.status, 200,
        "the delete must succeed, got {} {}",
        deleted.status, deleted.body
    );
    let survivors = describe_snapshots(&emulator, &[("CacheClusterId", "copied")]).await;
    assert_eq!(
        survivors.status, 200,
        "case survivors: expected HTTP 200, got {} {}",
        survivors.status, survivors.body
    );
    assert_eq!(
        reported_names(&survivors),
        vec!["after-change", "before-change"],
        "a snapshot must outlive its source cluster, got {}",
        survivors.body
    );
}

#[tokio::test]
async fn a_snapshot_name_the_store_does_not_hold_selects_an_empty_page() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "probed", &[]).await;
    for name in ["held-snap", "other-snap"] {
        let created = create_snapshot(&emulator, name, "probed", &[]).await;
        assert_eq!(
            created.status, 200,
            "case {name}: create must succeed, got {}",
            created.body
        );
        // Taking a snapshot leaves the source cluster `snapshotting`.
        std::thread::sleep(PAST_TRANSITION);
    }

    // `SnapshotName` NARROWS the page, so the axis is how the name relates to
    // the stored set: one that matches, one that matches but for its case, a
    // name no snapshot carries, a name that is a prefix or an extension of a
    // stored one, a name of a different form, the empty name, and the longest
    // name the service accepts.
    let long_name = "n".repeat(255);
    let rows: [(&str, &str, Vec<&str>); 8] = [
        ("a stored name", "held-snap", vec!["held-snap"]),
        ("a stored name in another case", "HELD-SNAP", vec!["held-snap"]),
        ("the other stored name", "other-snap", vec!["other-snap"]),
        ("a name nothing was created under", "no-such-snap", vec![]),
        ("a prefix of a stored name", "held", vec![]),
        ("an extension of a stored name", "held-snap-2", vec![]),
        ("the empty name", "", vec![]),
        ("the longest name the service accepts", long_name.as_str(), vec![]),
    ];
    for (case, name, expected) in rows {
        let reply = describe_snapshots(&emulator, &[("SnapshotName", name)]).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: a name filter is answered with a page, not a fault; expected HTTP \
             200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "",
            "case {case}: no error may be reported, got {}",
            reply.body
        );
        assert_eq!(
            reported_names(&reply),
            expected,
            "case {case}: expected {expected:?}, got {}",
            reply.body
        );
        assert!(
            reply.marker().is_none(),
            "case {case}: a single-name page paginates nothing, got {}",
            reply.body
        );
    }

    // The probe changed nothing: the store still holds both snapshots, so a
    // caller that polled for one that does not exist yet sees the same set.
    let all = describe_snapshots(&emulator, &[]).await;
    assert_eq!(
        reported_names(&all),
        vec!["held-snap", "other-snap"],
        "case after probing: a describe must not change what a later caller sees, got {}",
        all.body
    );
}

#[tokio::test]
async fn the_snapshot_source_filter_takes_the_spelling_the_response_reports() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "sourced", &[]).await;
    let created = create_snapshot(&emulator, "sourced-snap", "sourced", &[]).await;
    assert_eq!(
        created.status, 200,
        "the snapshot must be created, got {}",
        created.body
    );

    // The round trip the rule is about: read a snapshot's own
    // `SnapshotSource`, then list only the snapshots of that source.
    let reported_source = created
        .text("Snapshot/SnapshotSource")
        .unwrap_or_else(|| panic!("a snapshot reports its SnapshotSource: {}", created.body))
        .to_string();
    let round_trip = describe_snapshots(&emulator, &[("SnapshotSource", &reported_source)]).await;
    assert_eq!(
        round_trip.status, 200,
        "case round trip {reported_source}: the spelling the API gave back must be accepted, \
         got {} {}",
        round_trip.status, round_trip.body
    );
    assert_eq!(
        reported_names(&round_trip),
        vec!["sourced-snap"],
        "case round trip {reported_source}: the filter must select the snapshot that reported \
         that source, got {}",
        round_trip.body
    );

    // The accepted vocabulary, from the member's documentation ("If set to
    // system … If set to user …") and from what `Snapshot$SnapshotSource`
    // reports (`automated` / `manual`). The axis is which spelling of which
    // source, and its case; the stored snapshot is manually created, so the
    // manual spellings select it and the automatic ones select nothing.
    let accepted: [(&str, Vec<&str>); 8] = [
        ("user", vec!["sourced-snap"]),
        ("manual", vec!["sourced-snap"]),
        ("User", vec!["sourced-snap"]),
        ("MANUAL", vec!["sourced-snap"]),
        ("system", vec![]),
        ("automated", vec![]),
        ("System", vec![]),
        ("AUTOMATED", vec![]),
    ];
    for (source, expected) in accepted {
        let reply = describe_snapshots(&emulator, &[("SnapshotSource", source)]).await;
        assert_eq!(
            reply.status, 200,
            "case SnapshotSource={source}: expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reported_names(&reply),
            expected,
            "case SnapshotSource={source}: expected {expected:?}, got {}",
            reply.body
        );
    }

    // A value in NEITHER vocabulary is still the modelled refusal. The axis is
    // how it misses: another `UpdateActionStatus`-shaped word, a status of a
    // snapshot rather than its source, a near miss, and the empty value.
    for source in ["scheduled", "creating", "manually", "users", ""] {
        let reply = describe_snapshots(&emulator, &[("SnapshotSource", source)]).await;
        assert_eq!(
            reply.status, 400,
            "case SnapshotSource={source:?}: expected HTTP 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case SnapshotSource={source:?}: expected InvalidParameterValue, got {}",
            reply.body
        );
    }
}

/// One settings-change row: the case name, the cluster it runs on, the form
/// pairs the immediate modify sends, and the element plus value that must
/// report the change.
type SettingsModifyCase = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static str,
    &'static str,
);

/// One node-change row: the case name, the cluster it runs on, and the form
/// pairs the immediate modify sends.
type NodeModifyCase = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

#[tokio::test]
async fn an_immediate_settings_modify_leaves_the_cluster_snapshotable() {
    let emulator = Emulator::new();

    // The axis: which member outside `PendingModifiedValues` the immediate
    // modify changes. None of them is node work, so each is applied
    // synchronously and the cluster stays settled — the applied value and a
    // usable cluster state become visible together.
    let rows: [SettingsModifyCase; 6] = [
        (
            "SnapshotRetentionLimit",
            "settled-retention",
            &[("SnapshotRetentionLimit", "3")],
            "CacheCluster/SnapshotRetentionLimit",
            "3",
        ),
        (
            "SnapshotWindow",
            "settled-window",
            &[("SnapshotWindow", "04:00-05:00")],
            "CacheCluster/SnapshotWindow",
            "04:00-05:00",
        ),
        (
            "PreferredMaintenanceWindow",
            "settled-maintenance",
            &[("PreferredMaintenanceWindow", "thu:07:00-thu:08:00")],
            "CacheCluster/PreferredMaintenanceWindow",
            "thu:07:00-thu:08:00",
        ),
        (
            "AutoMinorVersionUpgrade",
            "settled-upgrade",
            &[("AutoMinorVersionUpgrade", "false")],
            "CacheCluster/AutoMinorVersionUpgrade",
            "false",
        ),
        (
            "IpDiscovery",
            "settled-discovery",
            &[("IpDiscovery", "ipv6")],
            "CacheCluster/IpDiscovery",
            "ipv6",
        ),
        (
            "NotificationTopicArn",
            "settled-topic",
            &[("NotificationTopicArn", "arn:aws:sns:us-east-1:000000000000:ec")],
            "NotificationConfiguration/TopicArn",
            "arn:aws:sns:us-east-1:000000000000:ec",
        ),
    ];

    for (case, cluster, extra, element, expected) in rows {
        create_available_cluster(&emulator, cluster, &[("Engine", "redis")]).await;
        let mut pairs = params(&[("CacheClusterId", cluster), ("ApplyImmediately", "true")]);
        pairs.extend(params(extra));
        let modified = emulator.call("ModifyCacheCluster", &pairs).await;
        assert_eq!(
            modified.status, 200,
            "case {case}: the modify must answer 200, got {}",
            modified.body
        );

        // The documented way to confirm a modify landed: read the field back.
        let described = describe_cluster(&emulator, cluster).await;
        assert_eq!(
            described.text(element),
            Some(expected),
            "case {case}: {element} must report the applied value, got {:?}",
            described.text(element)
        );
        assert_eq!(
            described.text("CacheCluster/CacheClusterStatus"),
            Some("available"),
            "case {case}: a settings change is no node work, so the cluster stays available, \
             got {:?}",
            described.text("CacheCluster/CacheClusterStatus")
        );

        // And the state-gated operation on that cluster is accepted straight
        // away, with no wait between reading the value back and calling it.
        let snapshot = create_snapshot(&emulator, &format!("{cluster}-snap"), cluster, &[]).await;
        assert_eq!(
            snapshot.status, 200,
            "case {case}: CreateSnapshot must be accepted once the modified value is \
             observable, got {} {}",
            snapshot.status, snapshot.body
        );
        assert_eq!(
            snapshot.error_code(),
            "",
            "case {case}: CreateSnapshot must report no error, got {}",
            snapshot.body
        );
    }

    // The contrast, on the same axis: a member `PendingModifiedValues` DOES
    // model is node work, so an immediate modify of one leaves the cluster in
    // the transient status and the state-gated call is refused until the clock
    // settles it.
    let node_rows: [NodeModifyCase; 2] = [
        (
            "EngineVersion",
            "moving-version",
            &[("EngineVersion", "7.0")],
        ),
        (
            "CacheNodeType",
            "moving-type",
            &[("CacheNodeType", "cache.m7g.large")],
        ),
    ];
    for (case, cluster, extra) in node_rows {
        create_available_cluster(&emulator, cluster, &[("Engine", "redis")]).await;
        let mut pairs = params(&[("CacheClusterId", cluster), ("ApplyImmediately", "true")]);
        pairs.extend(params(extra));
        let modified = emulator.call("ModifyCacheCluster", &pairs).await;
        assert_eq!(
            modified.status, 200,
            "case {case}: the modify must answer 200, got {}",
            modified.body
        );
        assert_eq!(
            modified.text("CacheCluster/CacheClusterStatus"),
            Some("modifying"),
            "case {case}: node work answers with the cluster transient, got {:?}",
            modified.text("CacheCluster/CacheClusterStatus")
        );
        // Straight after that call, inside the span: the state-gated call is
        // refused with the status the cluster has.
        let refused = create_snapshot(&emulator, &format!("{cluster}-snap"), cluster, &[]).await;
        assert_eq!(
            refused.error_code(),
            "InvalidCacheClusterState",
            "case {case}: a cluster doing node work refuses the state-gated call, got {} {}",
            refused.status, refused.body
        );
        // A read is answered once the node work the clock owed is done, and the
        // same call is accepted from then on.
        let described = describe_cluster(&emulator, cluster).await;
        assert_eq!(
            described.text("CacheCluster/CacheClusterStatus"),
            Some("available"),
            "case {case}: the read is answered once the node work is done, got {:?}",
            described.text("CacheCluster/CacheClusterStatus")
        );
        let accepted = create_snapshot(&emulator, &format!("{cluster}-snap"), cluster, &[]).await;
        assert_eq!(
            accepted.status, 200,
            "case {case}: once settled the state-gated call is accepted, got {} {}",
            accepted.status, accepted.body
        );
    }
}
