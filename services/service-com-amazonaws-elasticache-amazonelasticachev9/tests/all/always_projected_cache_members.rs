// SPDX-License-Identifier: CC0-1.0

//! The members EVERY projection of a `CacheCluster` or a `ReplicationGroup` carries.
//!
//! RULE: a member of those shapes that ElastiCache populates for every cache — a
//! non-optional boolean, an enum the service RESOLVES when the cache is created, a list or
//! a structure the protocol serializes whether or not it holds anything — is carried by
//! every projection of that cache: the create that minted it, the read that reads it back
//! and the delete acknowledgement. A request that stated none of them is answered with the
//! value the service resolved for it, and a list or structure holding nothing is serialized
//! as an EMPTY element rather than withheld, so a client can tell "this cache has none"
//! from "this service did not say".
//!
//! Two members are derived rather than echoed, and are asserted the same way:
//! `CacheCluster$EngineVersion` is "the version of the cache engine that is used in this
//! cluster", so it reports the RELEASE the cluster runs — the release behind the version
//! LINE ElastiCache names a version by (`state::patch_release`) — and a cluster mode
//! disabled shard reports a `ReaderEndpoint` whatever its replica count, because that is
//! where a client sends its reads. An ENDPOINT is the third: it appears once the nodes
//! behind it are provisioned, and carries the address this emulator answers for the cache
//! at where it serves one.
//!
//! The rows span what the rule generalises over: the engine, whether the request STATES the
//! resolved members or leaves them to the service, whether it names an engine version and
//! whether that version is a line or a release, the node count and the port, and — for a
//! group — the cluster mode, the shard count and whether a shard holds a replica. The
//! values a row expects are either resolved defaults read from `state`, or the values that
//! row's own request stated; no recorded response is copied here.
//!
//! Every assertion goes over the wire through [`crate::harness`], through the same router
//! the production emulator mounts, so a member that is not computed from persisted state
//! cannot pass. An element that is present and EMPTY is read out of the raw body
//! ([`crate::harness::carries_element`]), because the harness' text reader cannot tell one
//! from a member the service never serialized.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{carries_element, is_own_host, params, poll_until, Emulator, Reply, REGION};

/// The `CacheCluster` members every projection of a cluster carries, as element paths of
/// the awsQuery envelope. Each is a member the shape declares and ElastiCache populates for
/// every cluster (`smithy_query(query="shape", shape="CacheCluster")`): the booleans it
/// resolves, the network enums, the cluster's own parameter-group status, and the lists and
/// structures it serializes empty.
const CLUSTER_ALWAYS_PROJECTED: &[&str] = &[
    "CacheClusterId",
    "ClientDownloadLandingPage",
    "Engine",
    "EngineVersion",
    "CacheClusterStatus",
    "NumCacheNodes",
    "PendingModifiedValues",
    "CacheSecurityGroups",
    "CacheParameterGroup",
    "CacheNodeIdsToReboot",
    "AutoMinorVersionUpgrade",
    "AuthTokenEnabled",
    "TransitEncryptionEnabled",
    "AtRestEncryptionEnabled",
    "ReplicationGroupLogDeliveryEnabled",
    "LogDeliveryConfigurations",
    "NetworkType",
    "IpDiscovery",
    "ARN",
];

/// The `ReplicationGroup` members every projection of a group carries
/// (`smithy_query(query="shape", shape="ReplicationGroup")`), by the same reading as
/// [`CLUSTER_ALWAYS_PROJECTED`].
///
/// `Status` is absent from the list and asserted by its own element PATH instead: a group's
/// body also carries a `Status` under each node group, which a bare element name could not
/// tell apart from the group's own.
const GROUP_ALWAYS_PROJECTED: &[&str] = &[
    "ReplicationGroupId",
    "Description",
    "GlobalReplicationGroupInfo",
    "PendingModifiedValues",
    "MemberClusters",
    "NodeGroups",
    "AutomaticFailover",
    "MultiAZ",
    "ClusterEnabled",
    "AutoMinorVersionUpgrade",
    "AuthTokenEnabled",
    "TransitEncryptionEnabled",
    "AtRestEncryptionEnabled",
    "LogDeliveryConfigurations",
    "NetworkType",
    "IpDiscovery",
    "ARN",
];

/// Assert `reply` carries every element in `required`, whatever each one holds.
fn assert_carries(case: &str, where_from: &str, reply: &Reply, required: &[&str]) {
    for member in required {
        assert!(
            carries_element(&reply.body, member),
            "case {case:?} ({where_from}): the projection must carry {member}, got {}",
            reply.body
        );
    }
}

/// Assert `reply` reports `expected` at each `(element path, value)` pair.
fn assert_reports(case: &str, where_from: &str, reply: &Reply, expected: &[(&str, String)]) {
    for (path, value) in expected {
        assert_eq!(
            reply.text(path),
            Some(value.as_str()),
            "case {case:?} ({where_from}): {path} must report {value:?}, got {:?}",
            reply.text(path)
        );
    }
}

/// The newest version LINE the published catalog offers for `engine`
/// (`state::CACHE_ENGINE_VERSIONS`, ordered by `state::version_sort_key`) — the version a
/// create that names none runs.
fn newest_line(engine: &str) -> String {
    state::CACHE_ENGINE_VERSIONS
        .iter()
        .filter(|row| row.engine == engine)
        .max_by_key(|row| state::version_sort_key(row.engine_version))
        .map(|row| row.engine_version.to_string())
        .unwrap_or_else(|| panic!("the catalog must publish a version for {engine}"))
}

/// One cluster row: what the request states, and what the resolved projection must report
/// beside the members every row shares.
struct ClusterRow {
    case: &'static str,
    id: &'static str,
    /// The form pairs sent beside `CacheClusterId`.
    extra: &'static [(&'static str, &'static str)],
    engine: &'static str,
    /// Whether this emulator serves a data plane for the cache — and so whether the cluster
    /// publishes an endpoint of its own at all.
    serves_data_plane: bool,
    /// The `EngineVersion` the request named, or `None` where it named none and the service
    /// picked the newest line the catalog publishes.
    named_version: Option<&'static str>,
    /// What the row's own request stated for the resolved members, as
    /// `(element, value)` — anything absent from here must come back as the service's
    /// default.
    stated: &'static [(&'static str, &'static str)],
}

/// The axis: the engine, whether the request leaves the resolved members to the service or
/// states them, whether it names a version (a release, or a LINE the catalog names), the
/// node count and the port.
const CLUSTER_ROWS: &[ClusterRow] = &[
    ClusterRow {
        case: "memcached, nothing but the engine stated",
        id: "projected-memcached",
        extra: &[("Engine", "memcached")],
        engine: "memcached",
        serves_data_plane: false,
        named_version: None,
        stated: &[],
    },
    ClusterRow {
        case: "memcached, two nodes on a port of its own",
        id: "projected-memcached-two",
        extra: &[
            ("Engine", "memcached"),
            ("NumCacheNodes", "2"),
            ("Port", "11222"),
            ("EngineVersion", "1.6.17"),
        ],
        engine: "memcached",
        serves_data_plane: false,
        named_version: Some("1.6.17"),
        stated: &[],
    },
    ClusterRow {
        case: "redis on a version LINE the catalog names",
        id: "projected-redis-line",
        extra: &[("Engine", "redis"), ("EngineVersion", "6.2")],
        engine: "redis",
        serves_data_plane: true,
        named_version: Some("6.2"),
        stated: &[],
    },
    ClusterRow {
        case: "valkey, every resolved member stated by the request",
        id: "projected-valkey-stated",
        extra: &[
            ("Engine", "valkey"),
            ("NetworkType", "dual_stack"),
            ("IpDiscovery", "ipv6"),
            ("AutoMinorVersionUpgrade", "false"),
            ("TransitEncryptionEnabled", "true"),
        ],
        engine: "valkey",
        serves_data_plane: true,
        named_version: None,
        stated: &[
            ("CacheCluster/NetworkType", "dual_stack"),
            ("CacheCluster/IpDiscovery", "ipv6"),
            ("CacheCluster/AutoMinorVersionUpgrade", "false"),
            ("CacheCluster/TransitEncryptionEnabled", "true"),
        ],
    },
];

/// The page `CacheCluster$ClientDownloadLandingPage` names: "the web page where you can
/// download the latest ElastiCache client library", which is the ElastiCache console's own
/// client-download page. Written out here rather than read from the service's own constant,
/// so the VALUE is what this suite pins and not merely that the response echoes whatever
/// the implementation holds.
const CLIENT_DOWNLOAD_LANDING_PAGE: &str =
    "https://console.aws.amazon.com/elasticache/home#client-download:";

/// What `NetworkType` and `IpDiscovery` resolve to for a cache created without either: the
/// IPv4 value of the modelled `NetworkType` / `IpDiscovery` enums
/// (`smithy_query(query="shape", shape="NetworkType,IpDiscovery")`: `ipv4 | ipv6 |
/// dual_stack` and `ipv4 | ipv6`). Written out for the reason
/// [`CLIENT_DOWNLOAD_LANDING_PAGE`] is.
const DEFAULT_NETWORK_FAMILY: &str = "ipv4";

/// What `AutoMinorVersionUpgrade` resolves to for a cache created without one: a cache is
/// created opted IN to the next auto minor version upgrade campaign. Written out for the
/// reason [`CLIENT_DOWNLOAD_LANDING_PAGE`] is.
const DEFAULT_AUTO_MINOR_VERSION_UPGRADE: &str = "true";

/// What `row` must report for every resolved member: the value its request stated, or the
/// service's own default for a member it left out.
fn cluster_expectations(row: &ClusterRow) -> Vec<(&'static str, String)> {
    let line = row
        .named_version
        .map_or_else(|| newest_line(row.engine), str::to_string);
    let mut expected: Vec<(&'static str, String)> = vec![
        ("CacheCluster/Engine", row.engine.to_string()),
        (
            "CacheCluster/EngineVersion",
            state::patch_release(row.engine, &line).to_string(),
        ),
        (
            "CacheCluster/ClientDownloadLandingPage",
            CLIENT_DOWNLOAD_LANDING_PAGE.to_string(),
        ),
        (
            "CacheCluster/AutoMinorVersionUpgrade",
            DEFAULT_AUTO_MINOR_VERSION_UPGRADE.to_string(),
        ),
        (
            "CacheCluster/NetworkType",
            DEFAULT_NETWORK_FAMILY.to_string(),
        ),
        (
            "CacheCluster/IpDiscovery",
            DEFAULT_NETWORK_FAMILY.to_string(),
        ),
        // A standalone cluster is in no replication group, so it delivers none of a
        // group's logs and is encrypted neither in transit nor at rest unless the request
        // asked for it.
        (
            "CacheCluster/ReplicationGroupLogDeliveryEnabled",
            false.to_string(),
        ),
        ("CacheCluster/TransitEncryptionEnabled", false.to_string()),
        ("CacheCluster/AtRestEncryptionEnabled", false.to_string()),
    ];
    for (element, value) in row.stated {
        let stated = expected
            .iter_mut()
            .find(|(path, _)| path == element)
            .unwrap_or_else(|| panic!("case {:?}: {element} is not a resolved member", row.case));
        stated.1 = (*value).to_string();
    }
    expected
}

#[tokio::test]
async fn every_projection_of_a_cache_cluster_carries_the_members_the_service_resolves() {
    let emulator = Emulator::new();
    for row in CLUSTER_ROWS {
        let expected = cluster_expectations(row);
        let pairs = crate::harness::cluster_create_params(row.id, row.extra);
        let created = emulator.call("CreateCacheCluster", &pairs).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateCacheCluster must answer 200, got {} {}",
            row.case, created.status, created.body
        );
        assert_carries(row.case, "create", &created, CLUSTER_ALWAYS_PROJECTED);
        assert_reports(row.case, "create", &created, &expected);
        // A cluster whose nodes are still being provisioned publishes no endpoint: there is
        // nothing listening yet for a client to be pointed at.
        assert!(
            created.all("ConfigurationEndpoint/Address").is_empty(),
            "case {:?}: the create publishes no endpoint for a cluster still being \
             provisioned, got {:?}",
            row.case,
            created.all("ConfigurationEndpoint/Address")
        );

        // The same member set on the read-back, once the clock has carried the cluster to
        // the status a client polls for.
        let described = poll_until(
            &emulator,
            REGION,
            row.case,
            "the cluster reaching available",
            "DescribeCacheClusters",
            &params(&[("CacheClusterId", row.id), ("ShowCacheNodeInfo", "true")]),
            |reply| reply.text("CacheCluster/CacheClusterStatus") == Some("available"),
        )
        .await;
        assert_carries(row.case, "describe", &described, CLUSTER_ALWAYS_PROJECTED);
        assert_reports(row.case, "describe", &described, &expected);

        // WHERE the cluster is reached, and from WHEN: an endpoint appears once the nodes
        // behind it are provisioned, and it carries the address this emulator answers for
        // the cache at where it serves one — a Valkey / Redis OSS cache gets a socket of
        // its own, and a Memcached cache, whose clients speak a protocol this emulator has
        // nothing to answer with, keeps the name ElastiCache's own zone would carry.
        for member in [
            "ConfigurationEndpoint/Address",
            "CacheNodes/CacheNode/Endpoint/Address",
        ] {
            let published = described.all(member);
            assert!(
                !published.is_empty(),
                "case {:?}: a settled cluster publishes {member}, got {published:?}",
                row.case
            );
            // Only for a cache this emulator SERVES is there an address of its own to
            // publish; what a cache it serves none for carries is the subject of the
            // Memcached data plane this emulator does not have, so this row asserts the
            // member is there and leaves its spelling to that work.
            if row.serves_data_plane {
                assert!(
                    published.iter().all(|address| is_own_host(address)),
                    "case {:?}: {member} must be an address this emulator answers on, got \
                     {published:?}",
                    row.case
                );
            }
        }

        // And on the delete acknowledgement, which is a projection of the cluster being
        // torn down rather than an object with no configuration.
        let deleted = emulator
            .call(
                "DeleteCacheCluster",
                &params(&[("CacheClusterId", row.id)]),
            )
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: DeleteCacheCluster must answer 200, got {} {}",
            row.case, deleted.status, deleted.body
        );
        assert_eq!(
            deleted.text("CacheCluster/CacheClusterStatus"),
            Some("deleting"),
            "case {:?}: the delete acknowledgement reports the cluster leaving, got {:?}",
            row.case,
            deleted.text("CacheCluster/CacheClusterStatus")
        );
        assert_carries(row.case, "delete", &deleted, CLUSTER_ALWAYS_PROJECTED);
        assert_reports(row.case, "delete", &deleted, &expected);
    }
}

/// One group row: what the request states, how the group is shaped, and what its projection
/// must report beside the members every row shares.
struct GroupRow {
    case: &'static str,
    id: &'static str,
    /// The form pairs sent beside the id and the description.
    extra: &'static [(&'static str, &'static str)],
    engine: &'static str,
    /// Whether the keyspace is partitioned across shards, which is what decides whether the
    /// group is reached through a configuration endpoint or through its shard's own
    /// primary and reader endpoints.
    cluster_enabled: bool,
    /// Whether a shard of this group holds a replica to read from.
    has_replica: bool,
    /// See [`ClusterRow::stated`].
    stated: &'static [(&'static str, &'static str)],
}

/// The axis: the engine, the cluster mode and the shard count, whether a shard holds a
/// replica, and whether the request states the resolved members or leaves them to the
/// service.
const GROUP_ROWS: &[GroupRow] = &[
    GroupRow {
        case: "redis, one shard with no replica",
        id: "projected-group-single",
        extra: &[("Engine", "redis"), ("NumCacheClusters", "1")],
        engine: "redis",
        cluster_enabled: false,
        has_replica: false,
        stated: &[],
    },
    GroupRow {
        case: "redis, one shard with a replica",
        id: "projected-group-replica",
        extra: &[("Engine", "redis"), ("NumCacheClusters", "2")],
        engine: "redis",
        cluster_enabled: false,
        has_replica: true,
        stated: &[],
    },
    GroupRow {
        case: "valkey, two shards with the keyspace partitioned",
        id: "projected-group-shards",
        extra: &[
            ("Engine", "valkey"),
            ("ClusterMode", "enabled"),
            ("NumNodeGroups", "2"),
            ("ReplicasPerNodeGroup", "0"),
        ],
        engine: "valkey",
        cluster_enabled: true,
        has_replica: false,
        stated: &[],
    },
    GroupRow {
        case: "valkey, every resolved member stated by the request",
        id: "projected-group-stated",
        extra: &[
            ("Engine", "valkey"),
            ("NumCacheClusters", "1"),
            ("NetworkType", "dual_stack"),
            ("IpDiscovery", "ipv6"),
            ("AutoMinorVersionUpgrade", "false"),
        ],
        engine: "valkey",
        cluster_enabled: false,
        has_replica: false,
        stated: &[
            ("ReplicationGroup/NetworkType", "dual_stack"),
            ("ReplicationGroup/IpDiscovery", "ipv6"),
            ("ReplicationGroup/AutoMinorVersionUpgrade", "false"),
        ],
    },
];

/// What `row` must report for every resolved member of its group; see
/// [`cluster_expectations`].
fn group_expectations(row: &GroupRow) -> Vec<(&'static str, String)> {
    let mut expected: Vec<(&'static str, String)> = vec![
        ("ReplicationGroup/Engine", row.engine.to_string()),
        (
            "ReplicationGroup/AutoMinorVersionUpgrade",
            DEFAULT_AUTO_MINOR_VERSION_UPGRADE.to_string(),
        ),
        (
            "ReplicationGroup/NetworkType",
            DEFAULT_NETWORK_FAMILY.to_string(),
        ),
        (
            "ReplicationGroup/IpDiscovery",
            DEFAULT_NETWORK_FAMILY.to_string(),
        ),
        (
            "ReplicationGroup/ClusterEnabled",
            row.cluster_enabled.to_string(),
        ),
    ];
    for (element, value) in row.stated {
        let stated = expected
            .iter_mut()
            .find(|(path, _)| path == element)
            .unwrap_or_else(|| panic!("case {:?}: {element} is not a resolved member", row.case));
        stated.1 = (*value).to_string();
    }
    expected
}

#[tokio::test]
async fn every_projection_of_a_replication_group_carries_the_members_the_service_resolves() {
    let emulator = Emulator::new();
    for row in GROUP_ROWS {
        let expected = group_expectations(row);
        let mut pairs = params(&[
            ("ReplicationGroupId", row.id),
            ("ReplicationGroupDescription", "a group of this suite"),
        ]);
        pairs.extend(params(row.extra));
        let created = emulator.call("CreateReplicationGroup", &pairs).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateReplicationGroup must answer 200, got {} {}",
            row.case, created.status, created.body
        );
        assert_carries(row.case, "create", &created, GROUP_ALWAYS_PROJECTED);
        assert_reports(row.case, "create", &created, &expected);

        let described = poll_until(
            &emulator,
            REGION,
            row.case,
            "the group reaching available",
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", row.id)]),
            |reply| reply.text("ReplicationGroup/Status") == Some("available"),
        )
        .await;
        assert_carries(row.case, "describe", &described, GROUP_ALWAYS_PROJECTED);
        assert_reports(row.case, "describe", &described, &expected);
        assert_eq!(
            described.text("ReplicationGroup/Status"),
            Some("available"),
            "case {:?}: the settled group reports its own status, got {:?}",
            row.case,
            described.text("ReplicationGroup/Status")
        );

        // WHERE a client of the settled group reads. A group whose keyspace is not
        // partitioned publishes its shard's primary and reader endpoints — the reader one
        // whatever the shard's replica count, because a shard with no replica serves its
        // reads from the member holding the primary role.
        let reader = described.text("NodeGroup/ReaderEndpoint/Address");
        let primary = described.text("NodeGroup/PrimaryEndpoint/Port");
        if row.cluster_enabled {
            assert!(
                reader.is_none(),
                "case {:?}: a partitioned group is reached through its configuration \
                 endpoint, not a shard's reader endpoint, got {reader:?}",
                row.case
            );
        } else {
            assert!(
                reader.is_some_and(is_own_host),
                "case {:?}: the shard's reader endpoint must be an address this emulator \
                 answers on, got {reader:?}",
                row.case
            );
            let reads_elsewhere = described.text("NodeGroup/ReaderEndpoint/Port") != primary;
            assert_eq!(
                reads_elsewhere, row.has_replica,
                "case {:?}: a shard reads at a replica's socket only where it has one, got \
                 reader {:?} against primary {primary:?}",
                row.case,
                described.text("NodeGroup/ReaderEndpoint/Port")
            );
        }

        // The group's member clusters are clusters of the account, and each projects the
        // cluster member set too — including the log-delivery flag a member carries for
        // the group it belongs to.
        for member in described.all("NodeGroupMember/CacheClusterId") {
            let read = emulator
                .call(
                    "DescribeCacheClusters",
                    &params(&[("CacheClusterId", member), ("ShowCacheNodeInfo", "true")]),
                )
                .await;
            assert_eq!(
                read.status, 200,
                "case {:?}: the member {member} must be a cluster of the account, got {} {}",
                row.case, read.status, read.body
            );
            assert_carries(row.case, "the member cluster", &read, CLUSTER_ALWAYS_PROJECTED);
            assert_eq!(
                read.text("CacheCluster/ReplicationGroupLogDeliveryEnabled"),
                Some("false"),
                "case {:?}: the member {member} delivers none of the group's logs, got {:?}",
                row.case,
                read.text("CacheCluster/ReplicationGroupLogDeliveryEnabled")
            );
        }

        // And the delete acknowledgement projects the group being torn down.
        let deleted = emulator
            .call(
                "DeleteReplicationGroup",
                &params(&[("ReplicationGroupId", row.id)]),
            )
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: DeleteReplicationGroup must answer 200, got {} {}",
            row.case, deleted.status, deleted.body
        );
        assert_eq!(
            deleted.text("ReplicationGroup/Status"),
            Some("deleting"),
            "case {:?}: the delete acknowledgement reports the group leaving, got {:?}",
            row.case,
            deleted.text("ReplicationGroup/Status")
        );
        assert_carries(row.case, "delete", &deleted, GROUP_ALWAYS_PROJECTED);
        assert_reports(row.case, "delete", &deleted, &expected);
    }
}
