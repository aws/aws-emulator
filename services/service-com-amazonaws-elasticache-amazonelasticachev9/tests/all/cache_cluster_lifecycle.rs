// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ElastiCache cache-cluster and cache-subnet-group
//! lifecycle.
//!
//! Every assertion goes over the wire through [`crate::harness`]: an
//! `awsQuery` form-urlencoded request is built, dispatched through the same
//! `MultiServiceRouter` the production emulator mounts, and only the HTTP
//! status and the XML body are inspected. Nothing reaches into the service's
//! state struct, so a response that is not computed from persisted state cannot
//! pass.
//!
//! The tests are tables: each row is one input along the axis the rule under
//! test generalises over (engine, node count, port, region, page size, subnet
//! count, apply-immediately), and the `demo-cluster` case from the bootstrap
//! issue is only one row.
//!
//! The wire harness this suite drives the emulator through lives in
//! [`crate::harness`], so every suite of this test target drives the same
//! router through the same helpers instead of carrying a second copy of them.

use std::collections::BTreeMap;
use std::time::Duration;

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{
    assert_own_endpoint, is_own_host, params, poll_until, Emulator, RefusalCase, Reply,
    PAST_TRANSITION, REGION,
};


/// One refusal row whose request has to be BUILT rather than listed, because a
/// value in it is derived (a subnet id the account has): the case name, the form
/// pairs, the HTTP status and the awsQuery error code.
pub(crate) type BuiltRefusalCase = (&'static str, Vec<(String, String)>, u16, &'static str);

/// One node-placement row: the case name, the form pairs that make the
/// request, how many distinct zones its nodes must end up in, and what
/// `PreferredAvailabilityZone` must report.
type PlacementCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    usize,
    &'static str,
);

/// The subnets the account HAS in [`REGION`] — the only subnet ids a request may
/// name, as Amazon EC2 (the service that OWNS subnets, and the one every
/// ElastiCache placement is resolved against) reports them, ordered by
/// Availability Zone.
///
/// `pub(crate)` for the same reason as the rest of this harness: the sibling
/// suites name the same subnets rather than inventing ids the service refuses.
pub(crate) async fn account_subnet_ids(emulator: &Emulator) -> Vec<String> {
    emulator
        .account_subnets()
        .await
        .into_iter()
        .map(|subnet| subnet.id)
        .collect()
}

/// `ids` as the indexed `SubnetIds` form pairs of a request, under the list's
/// member name (`SubnetIdentifier` for a cache subnet group, `SubnetId` for a
/// serverless cache).
pub(crate) fn subnet_pairs(member: &str, ids: &[String]) -> Vec<(String, String)> {
    ids.iter()
        .enumerate()
        .map(|(index, id)| (format!("SubnetIds.{member}.{}", index + 1), id.clone()))
        .collect()
}

/// One `CreateCacheSubnetGroup` request: the name, the description and the
/// account subnets `ids` names.
pub(crate) fn subnet_group_params(
    name: &str,
    description: &str,
    ids: &[String],
) -> Vec<(String, String)> {
    let mut pairs = params(&[
        ("CacheSubnetGroupName", name),
        ("CacheSubnetGroupDescription", description),
    ]);
    pairs.extend(subnet_pairs("SubnetIdentifier", ids));
    pairs
}

/// Create one cluster with `CacheClusterId` and whatever else `extra` names.
pub(crate) async fn create_cluster(emulator: &Emulator, id: &str, extra: &[(&str, &str)]) -> Reply {
    let mut pairs = params(&[("CacheClusterId", id)]);
    pairs.extend(params(extra));
    emulator.call("CreateCacheCluster", &pairs).await
}

/// Describe one cluster by id, with its node list.
pub(crate) async fn describe_cluster(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .call(
            "DescribeCacheClusters",
            &params(&[("CacheClusterId", id), ("ShowCacheNodeInfo", "true")]),
        )
        .await
}

/// Create a cluster and wait out the span after which the clock owes it
/// `available`, so a test about modify / delete starts from a settled cluster.
pub(crate) async fn create_available_cluster(emulator: &Emulator, id: &str, extra: &[(&str, &str)]) {
    let created = create_cluster(emulator, id, extra).await;
    assert_eq!(
        created.status, 200,
        "case {id}: create must succeed, got {}",
        created.body
    );
    std::thread::sleep(PAST_TRANSITION);
}

/// One row of the create/round-trip table.
#[derive(Debug, Clone)]
struct ClusterCase {
    id: &'static str,
    /// The form pairs sent beside `CacheClusterId`.
    extra: &'static [(&'static str, &'static str)],
    /// The identity the cluster is stored and reported under.
    stored_id: &'static str,
    engine: &'static str,
    engine_version: &'static str,
    port: &'static str,
    num_cache_nodes: usize,
    parameter_group: &'static str,
}

#[tokio::test]
async fn a_created_cluster_is_read_back_from_persisted_state() {
    // The axis: the engine and its documented defaults (`Engine` "memcached |
    // redis", the operation's own doc adding Valkey), the node count the engine
    // permits, and whether the request names a port and a version at all. The
    // parameter group is the family rule: the default group of the published
    // family the engine version belongs to (`state::PARAMETER_GROUP_FAMILIES`).
    let cases = [
        ClusterCase {
            id: "demo-cluster",
            extra: &[("CacheNodeType", "cache.t3.micro")],
            stored_id: "demo-cluster",
            engine: "redis",
            engine_version: "7.1",
            port: "6379",
            num_cache_nodes: 1,
            parameter_group: "default.redis7",
        },
        ClusterCase {
            id: "Mixed-Case-Id",
            extra: &[("Engine", "REDIS"), ("EngineVersion", "6.2")],
            // `CacheClusterId` "is stored as a lowercase string", and so is the
            // engine name.
            stored_id: "mixed-case-id",
            engine: "redis",
            engine_version: "6.2",
            port: "6379",
            num_cache_nodes: 1,
            // Redis OSS 6 is the one published family whose version line is
            // `<major>.x` rather than `<major>`
            // (`state::PARAMETER_GROUP_FAMILIES`).
            parameter_group: "default.redis6.x",
        },
        ClusterCase {
            id: "memcached-three",
            extra: &[
                ("Engine", "memcached"),
                ("NumCacheNodes", "3"),
                ("CacheNodeType", "cache.m7g.large"),
            ],
            stored_id: "memcached-three",
            engine: "memcached",
            engine_version: "1.6.22",
            port: "11211",
            num_cache_nodes: 3,
            parameter_group: "default.memcached1.6",
        },
        ClusterCase {
            id: "memcached-max",
            extra: &[
                ("Engine", "memcached"),
                // The published upper bound on a Memcached cluster.
                ("NumCacheNodes", "40"),
                ("Port", "11222"),
            ],
            stored_id: "memcached-max",
            engine: "memcached",
            engine_version: "1.6.22",
            port: "11222",
            num_cache_nodes: 40,
            parameter_group: "default.memcached1.6",
        },
        ClusterCase {
            id: "valkey-cluster",
            extra: &[("Engine", "valkey"), ("Port", "7000")],
            stored_id: "valkey-cluster",
            engine: "valkey",
            engine_version: "9.0",
            port: "7000",
            num_cache_nodes: 1,
            parameter_group: "default.valkey9",
        },
    ];

    let emulator = Emulator::new();
    // Create every row first, then wait out the transition span ONCE: a cluster
    // in the `creating` state reports cluster-level information only, so the
    // node-level assertions below need a settled cluster.
    let mut creates = Vec::new();
    for case in &cases {
        let created = create_cluster(&emulator, case.id, case.extra).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: create must answer 200, got {}",
            created.body
        );
        assert_eq!(
            created.text("CacheCluster/CacheClusterStatus"),
            Some("creating"),
            "case {case:?}: a just-created cluster is not ready yet, got {:?}",
            created.text("CacheCluster/CacheClusterStatus")
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
            "the cluster reaching available",
            "DescribeCacheClusters",
            &params(&[("CacheClusterId", case.id), ("ShowCacheNodeInfo", "true")]),
            |reply| reply.text("CacheCluster/CacheClusterStatus") == Some("available"),
        )
        .await;
    }

    for (case, created) in cases.iter().zip(&creates) {
        // The round trip: everything the response claims must come back out of
        // the store through the Describe reader.
        let described = describe_cluster(&emulator, case.id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: describe must answer 200, got {}",
            described.body
        );
        for (field, expected) in [
            ("CacheCluster/CacheClusterId", case.stored_id),
            ("CacheCluster/Engine", case.engine),
            ("CacheCluster/EngineVersion", case.engine_version),
            (
                "CacheParameterGroup/CacheParameterGroupName",
                case.parameter_group,
            ),
        ] {
            assert_eq!(
                described.text(field),
                Some(expected),
                "case {case:?}: {field} must report the stored value, got {:?}",
                described.text(field)
            );
        }
        // WHERE the cluster is reached. A Valkey / Redis OSS cluster is served by
        // this emulator's own RESP socket, so its endpoint is the address that
        // socket answers on and the port is the one the core assigned it; a
        // Memcached cluster is served by nothing here, so it keeps the name
        // ElastiCache's own zone would carry and the port the request asked for.
        let serves_resp = case.engine != "memcached";
        if serves_resp {
            assert_own_endpoint(
                case.stored_id,
                "the configuration endpoint",
                described
                    .text("ConfigurationEndpoint/Address")
                    .unwrap_or_default(),
                described.text("ConfigurationEndpoint/Port"),
            );
        } else {
            assert_eq!(
                described.text("ConfigurationEndpoint/Port"),
                Some(case.port),
                "case {case:?}: ConfigurationEndpoint/Port must report the stored value, got {:?}",
                described.text("ConfigurationEndpoint/Port")
            );
        }
        assert_eq!(
            described.text("CacheCluster/NumCacheNodes"),
            Some(case.num_cache_nodes.to_string().as_str()),
            "case {case:?}: NumCacheNodes must report the stored count, got {:?}",
            described.text("CacheCluster/NumCacheNodes")
        );

        // The create response and the describe response must agree on every
        // value the service minted, not just on the ones the request carried.
        for minted in [
            "CacheCluster/ARN",
            "ConfigurationEndpoint/Address",
            "CacheCluster/CacheClusterCreateTime",
            "CacheCluster/PreferredAvailabilityZone",
            "CacheCluster/PreferredMaintenanceWindow",
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
        assert_eq!(
            described.text("CacheCluster/ARN"),
            Some(
                format!(
                    "arn:aws:elasticache:{REGION}:{}:cluster:{}",
                    engine::emulator_account_id(),
                    case.stored_id
                )
                .as_str()
            ),
            "case {case:?}: the ARN must name the cluster, got {:?}",
            described.text("CacheCluster/ARN")
        );
        if serves_resp {
            assert!(
                described
                    .text("ConfigurationEndpoint/Address")
                    .is_some_and(is_own_host),
                "case {case:?}: the configuration endpoint of a cache this emulator serves must \
                 be an address it answers on, got {:?}",
                described.text("ConfigurationEndpoint/Address")
            );
        } else {
            assert!(
                described
                    .text("ConfigurationEndpoint/Address")
                    .is_some_and(|address| address.starts_with(&format!("{}.", case.stored_id))
                        && address.contains(".cfg.")
                        && address.ends_with(&format!("{REGION}.cache.amazonaws.com"))),
                "case {case:?}: the configuration endpoint must name the cluster and carry .cfg, \
                 got {:?}",
                described.text("ConfigurationEndpoint/Address")
            );
        }

        // `ShowCacheNodeInfo=true`, so the node list is reported, one node per
        // stored node with the 4-digit ids the model documents.
        let node_ids = described.all("CacheNodes/CacheNode/CacheNodeId");
        assert_eq!(
            node_ids.len(),
            case.num_cache_nodes,
            "case {case:?}: expected {} cache nodes, got {node_ids:?}",
            case.num_cache_nodes
        );
        assert_eq!(
            node_ids.first(),
            Some(&"0001"),
            "case {case:?}: node ids are 4-digit and start at 0001, got {node_ids:?}"
        );
        let node_ports = described.all("CacheNodes/CacheNode/Endpoint/Port");
        let cluster_port = described
            .text("ConfigurationEndpoint/Port")
            .unwrap_or_default();
        assert!(
            node_ports.iter().all(|port| *port == cluster_port),
            "case {case:?}: every node listens on the cluster's port {cluster_port}, got \
             {node_ports:?}"
        );
    }
}

#[tokio::test]
async fn cache_nodes_are_reported_only_when_the_caller_asks_for_them() {
    let emulator = Emulator::new();
    create_cluster(&emulator, "node-info", &[("Engine", "memcached"), ("NumCacheNodes", "2")])
        .await;
    // A cluster in the `creating` state reports cluster-level information only,
    // so this test starts from a settled one.
    std::thread::sleep(PAST_TRANSITION);

    for (show, expected_nodes) in [("true", 2), ("false", 0)] {
        let reply = emulator
            .call(
                "DescribeCacheClusters",
                &params(&[("CacheClusterId", "node-info"), ("ShowCacheNodeInfo", show)]),
            )
            .await;
        let node_ids = reply.all("CacheNodes/CacheNode/CacheNodeId");
        assert_eq!(
            node_ids.len(),
            expected_nodes,
            "case ShowCacheNodeInfo={show}: expected {expected_nodes} nodes, got {node_ids:?}"
        );
    }

    // Absent is the same as false: the flag is what asks for the node list.
    let reply = emulator
        .call(
            "DescribeCacheClusters",
            &params(&[("CacheClusterId", "node-info")]),
        )
        .await;
    assert!(
        reply.all("CacheNodes/CacheNode/CacheNodeId").is_empty(),
        "case ShowCacheNodeInfo absent: no node list is reported, got {}",
        reply.body
    );
}

#[tokio::test]
async fn a_cluster_reaches_available_on_the_clock_and_not_before() {
    let emulator = Emulator::new();

    // Every engine's cluster starts transient and settles the same way, so the
    // row is the engine.
    for engine in ["redis", "valkey", "memcached"] {
        let id = format!("settle-{engine}");
        let created = create_cluster(&emulator, &id, &[("Engine", engine)]).await;
        assert_eq!(
            created.text("CacheCluster/CacheClusterStatus"),
            Some("creating"),
            "case {engine}: the create answer is transient, got {:?}",
            created.text("CacheCluster/CacheClusterStatus")
        );
        // "If the cluster is in the creating state, only cluster-level
        // information is displayed until all of the nodes are successfully
        // provisioned".
        assert!(
            created.all("CacheNodes/CacheNode/CacheNodeId").is_empty(),
            "case {engine}: a creating cluster reports cluster-level information only, got {}",
            created.body
        );

        // The next read is answered once the provisioning the clock owed the
        // cluster is done, so it reports the settled cluster and the nodes that
        // came up with it — the first read a client issues after its create
        // converges rather than costing it a poll interval.
        let settled = describe_cluster(&emulator, &id).await;
        assert_eq!(
            settled.text("CacheCluster/CacheClusterStatus"),
            Some("available"),
            "case {engine}: the read is answered once the cluster is available, got {:?}",
            settled.text("CacheCluster/CacheClusterStatus")
        );
        let node_statuses = settled.all("CacheNodes/CacheNode/CacheNodeStatus");
        assert!(
            !node_statuses.is_empty() && node_statuses.iter().all(|status| *status == "available"),
            "case {engine}: the cluster's nodes settle with it, got {node_statuses:?}"
        );
    }
}

#[tokio::test]
async fn the_node_count_a_create_accepts_is_the_one_the_engine_permits() {
    // `NumCacheNodes`: "For clusters running Valkey or Redis OSS, this value
    // must be 1. For clusters running Memcached, this value must be between 1
    // and 40." Each published bound is a row, and so is one past it.
    let cases: [(&str, &str, bool); 13] = [
        ("redis", "1", true),
        ("redis", "2", false),
        ("redis", "0", false),
        ("valkey", "1", true),
        ("valkey", "3", false),
        ("memcached", "1", true),
        ("memcached", "20", true),
        ("memcached", "40", true),
        ("memcached", "41", false),
        ("memcached", "0", false),
        // `NumCacheNodes` carries no modelled `@range`, so the extremes of the
        // wire type reach the handler and must be refused rather than
        // overflowing any arithmetic on the way.
        ("memcached", "-2147483648", false),
        ("memcached", "2147483647", false),
        ("redis", "-2147483647", false),
    ];

    let emulator = Emulator::new();
    for (index, (engine, count, accepted)) in cases.iter().enumerate() {
        let id = format!("nodes-{index}");
        let reply = create_cluster(
            &emulator,
            &id,
            &[("Engine", engine), ("NumCacheNodes", count)],
        )
        .await;
        if *accepted {
            assert_eq!(
                reply.status, 200,
                "case {engine}/{count}: must be accepted, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.text("CacheCluster/NumCacheNodes"),
                Some(*count),
                "case {engine}/{count}: the stored count is what was asked for, got {:?}",
                reply.text("CacheCluster/NumCacheNodes")
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {engine}/{count}: must be refused 400, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "InvalidParameterValue",
                "case {engine}/{count}: expected InvalidParameterValue, got {}",
                reply.body
            );
            // A refused create stores nothing.
            let described = describe_cluster(&emulator, &id).await;
            assert_eq!(
                described.status, 404,
                "case {engine}/{count}: a refused create must not have stored a cluster, got {}",
                described.body
            );
        }
    }
}

#[tokio::test]
async fn a_create_naming_a_resource_this_emulator_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    // A subnet group that DOES exist, so the refusals below are about the
    // named resource and not about the group.
    let account_subnets = account_subnet_ids(&emulator).await;
    let group = emulator
        .call(
            "CreateCacheSubnetGroup",
            &subnet_group_params("demo-subnets", "demo", &account_subnets[..1]),
        )
        .await;
    assert_eq!(group.status, 200, "the subnet group must be created: {}", group.body);
    create_cluster(&emulator, "taken", &[]).await;

    // Each row: the member that names a resource, and the modelled refusal.
    let cases: [RefusalCase; 9] = [
        (
            "duplicate id",
            &[("CacheClusterId", "taken")],
            400,
            "CacheClusterAlreadyExists",
        ),
        (
            "unknown subnet group",
            &[("CacheSubnetGroupName", "nope")],
            400,
            "CacheSubnetGroupNotFoundFault",
        ),
        (
            "unknown replication group",
            &[("ReplicationGroupId", "rg-1")],
            404,
            "ReplicationGroupNotFoundFault",
        ),
        (
            "cache security group named at all",
            &[("CacheSecurityGroupNames.CacheSecurityGroupName.1", "sg-a")],
            400,
            "InvalidParameterValue",
        ),
        (
            "unknown parameter group",
            &[("CacheParameterGroupName", "my-own-group")],
            404,
            "CacheParameterGroupNotFound",
        ),
        (
            "a default name that is no engine family",
            &[("CacheParameterGroupName", "default.nonsense")],
            404,
            "CacheParameterGroupNotFound",
        ),
        (
            "unknown engine",
            &[("Engine", "dynamo")],
            400,
            "InvalidParameterValue",
        ),
        (
            "SnapshotRetentionLimit on memcached",
            &[("Engine", "memcached"), ("SnapshotRetentionLimit", "5")],
            400,
            "InvalidParameterCombination",
        ),
        (
            "SnapshotWindow on memcached",
            &[("Engine", "memcached"), ("SnapshotWindow", "05:00-09:00")],
            400,
            "InvalidParameterCombination",
        ),
    ];

    for (case, extra, status, code) in cases {
        let mut pairs = params(&[("CacheClusterId", "refused")]);
        pairs.extend(params(extra));
        // The duplicate row carries its own id, which must win.
        for (key, value) in params(extra) {
            if key == "CacheClusterId" {
                pairs.retain(|(k, _)| k != "CacheClusterId");
                pairs.push((key, value));
            }
        }
        let reply = emulator.call("CreateCacheCluster", &pairs).await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case}: expected {code}, got {}",
            reply.body
        );
    }

    // The subnet group that DOES exist is accepted, and the cluster reports it.
    let accepted = create_cluster(
        &emulator,
        "in-a-subnet-group",
        &[
            ("CacheSubnetGroupName", "Demo-Subnets"),
            // An engine family's default group DOES exist, so naming it
            // explicitly is accepted and stored.
            ("CacheParameterGroupName", "default.redis7"),
        ],
    )
    .await;
    assert_eq!(
        accepted.text("CacheCluster/CacheSubnetGroupName"),
        Some("demo-subnets"),
        "a cluster reports the stored (lowercased) subnet group name, got {}",
        accepted.body
    );
    assert_eq!(
        accepted.text("CacheParameterGroup/CacheParameterGroupName"),
        Some("default.redis7"),
        "a named default parameter group is accepted and stored, got {}",
        accepted.body
    );
}

#[tokio::test]
async fn describe_pagination_is_bounded_resumable_and_complete() {
    let emulator = Emulator::new();
    // 25 clusters: more than one page at the smallest page size the service
    // serves, so the second page is non-empty and the third does not exist.
    let total = 25;
    for index in 0..total {
        let reply = create_cluster(&emulator, &format!("page-{index:03}"), &[]).await;
        assert_eq!(reply.status, 200, "cluster {index} must be created: {}", reply.body);
    }

    // `MaxRecords`: "Default: 100. Constraints: minimum 20; maximum 100." Each
    // published bound is a row, and so is one past it.
    let cases: [(&str, bool); 5] = [
        ("20", true),
        ("100", true),
        ("19", false),
        ("101", false),
        ("1", false),
    ];
    for (max_records, accepted) in cases {
        let reply = emulator
            .call(
                "DescribeCacheClusters",
                &params(&[("MaxRecords", max_records)]),
            )
            .await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case MaxRecords={max_records}: must be accepted, got {} {}",
                reply.status, reply.body
            );
            let page = reply.all("CacheClusters/CacheCluster/CacheClusterId");
            let expected = max_records.parse::<usize>().expect("numeric row").min(total);
            assert_eq!(
                page.len(),
                expected,
                "case MaxRecords={max_records}: expected {expected} clusters, got {page:?}"
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case MaxRecords={max_records}: must be refused 400, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "InvalidParameterValue",
                "case MaxRecords={max_records}: expected InvalidParameterValue, got {}",
                reply.body
            );
        }
    }

    // A marker this service did not mint is a bad parameter value, not an
    // empty page.
    let bogus = emulator
        .call(
            "DescribeCacheClusters",
            &params(&[("Marker", "not-a-marker")]),
        )
        .await;
    assert_eq!(
        bogus.status, 400,
        "case bogus marker: must be refused 400, got {} {}",
        bogus.status, bogus.body
    );

    // Walking the pages visits every cluster exactly once.
    let mut seen: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut pairs = params(&[("MaxRecords", "20")]);
        if let Some(token) = marker.as_deref() {
            pairs.extend(params(&[("Marker", token)]));
        }
        let reply = emulator.call("DescribeCacheClusters", &pairs).await;
        assert_eq!(
            reply.status, 200,
            "page {pages}: must be served, got {} {}",
            reply.status, reply.body
        );
        let page: Vec<String> = reply
            .all("CacheClusters/CacheCluster/CacheClusterId")
            .into_iter()
            .map(str::to_string)
            .collect();
        assert!(
            !page.is_empty(),
            "page {pages}: a marker must not have been issued for an empty page"
        );
        seen.extend(page);
        pages += 1;
        marker = reply.marker().map(str::to_string);
        if marker.is_none() {
            break;
        }
        assert!(pages < 10, "pagination must terminate; saw {pages} pages");
    }
    assert_eq!(pages, 2, "25 clusters at 20 per page is 2 pages, got {pages}");
    let unique: BTreeMap<&String, ()> = seen.iter().map(|id| (id, ())).collect();
    assert_eq!(
        unique.len(),
        total,
        "every cluster must appear exactly once across the pages, got {} of {total} unique in {}",
        unique.len(),
        seen.len()
    );
    assert_eq!(
        seen.len(),
        total,
        "no cluster may repeat across pages, got {seen:?}"
    );
}

#[tokio::test]
async fn modify_applies_immediately_or_records_pending_modified_values() {
    let emulator = Emulator::new();

    // The axis: whether `ApplyImmediately` is set, over the members
    // `PendingModifiedValues` models (node count, node type, engine version) —
    // these are the ones that axis governs.
    let cases: [(&str, &str); 3] = [
        ("true", "immediate"),
        ("false", "deferred"),
        // Absent is the same as false.
        ("", "absent"),
    ];

    for (apply, label) in cases {
        let id = format!("modify-{label}");
        create_available_cluster(&emulator, &id, &[("Engine", "memcached"), ("NumCacheNodes", "1")])
            .await;

        let mut pairs = params(&[
            ("CacheClusterId", &id),
            ("NumCacheNodes", "3"),
            ("CacheNodeType", "cache.m7g.large"),
            ("EngineVersion", "1.6.17"),
            ("PreferredMaintenanceWindow", "tue:04:00-tue:05:00"),
        ]);
        if !apply.is_empty() {
            pairs.extend(params(&[("ApplyImmediately", apply)]));
        }
        let modified = emulator.call("ModifyCacheCluster", &pairs).await;
        assert_eq!(
            modified.status, 200,
            "case {label}: modify must answer 200, got {}",
            modified.body
        );

        let described = describe_cluster(&emulator, &id).await;
        // A member `PendingModifiedValues` does NOT model has nowhere to be
        // pending, so it is applied whatever `ApplyImmediately` says.
        assert_eq!(
            described.text("CacheCluster/PreferredMaintenanceWindow"),
            Some("tue:04:00-tue:05:00"),
            "case {label}: the maintenance window applies at once, got {:?}",
            described.text("CacheCluster/PreferredMaintenanceWindow")
        );

        if apply == "true" {
            assert_eq!(
                described.text("CacheCluster/NumCacheNodes"),
                Some("3"),
                "case {label}: an applied change is visible to the next describe, got {:?}",
                described.text("CacheCluster/NumCacheNodes")
            );
            assert_eq!(
                described.all("CacheNodes/CacheNode/CacheNodeId").len(),
                3,
                "case {label}: the node list grows with the count, got {:?}",
                described.all("CacheNodes/CacheNode/CacheNodeId")
            );
            assert_eq!(
                described.text("CacheCluster/CacheNodeType"),
                Some("cache.m7g.large"),
                "case {label}: the node type applies, got {:?}",
                described.text("CacheCluster/CacheNodeType")
            );
            assert_eq!(
                described.text("CacheCluster/EngineVersion"),
                Some("1.6.17"),
                "case {label}: the engine version applies, got {:?}",
                described.text("CacheCluster/EngineVersion")
            );
            assert!(
                described.all("PendingModifiedValues/NumCacheNodes").is_empty(),
                "case {label}: nothing stays pending once applied, got {}",
                described.body
            );
            assert_eq!(
                modified.text("CacheCluster/CacheClusterStatus"),
                Some("modifying"),
                "case {label}: an immediate modify answers with the cluster transient, got {:?}",
                modified.text("CacheCluster/CacheClusterStatus")
            );
            assert_eq!(
                described.text("CacheCluster/CacheClusterStatus"),
                Some("available"),
                "case {label}: a later read is answered once the work is done, got {:?}",
                described.text("CacheCluster/CacheClusterStatus")
            );
        } else {
            assert_eq!(
                described.text("CacheCluster/NumCacheNodes"),
                Some("1"),
                "case {label}: a deferred change leaves the running count alone, got {:?}",
                described.text("CacheCluster/NumCacheNodes")
            );
            assert_eq!(
                described.all("CacheNodes/CacheNode/CacheNodeId").len(),
                1,
                "case {label}: a deferred change adds no nodes, got {:?}",
                described.all("CacheNodes/CacheNode/CacheNodeId")
            );
            for (field, expected) in [
                ("PendingModifiedValues/NumCacheNodes", "3"),
                ("PendingModifiedValues/CacheNodeType", "cache.m7g.large"),
                ("PendingModifiedValues/EngineVersion", "1.6.17"),
            ] {
                assert_eq!(
                    described.text(field),
                    Some(expected),
                    "case {label}: {field} must report the deferred value, got {:?}",
                    described.text(field)
                );
            }
            assert_eq!(
                described.text("CacheCluster/CacheClusterStatus"),
                Some("available"),
                "case {label}: a deferred modify does not move the status, got {:?}",
                described.text("CacheCluster/CacheClusterStatus")
            );
        }
    }
}

#[tokio::test]
async fn removing_nodes_needs_the_ids_to_remove() {
    let emulator = Emulator::new();
    create_available_cluster(
        &emulator,
        "shrink",
        &[("Engine", "memcached"), ("NumCacheNodes", "3")],
    )
    .await;

    // `NumCacheNodes`: "If you are removing cache nodes, you must use the
    // CacheNodeIdsToRemove parameter to provide the IDs of the specific cache
    // nodes to remove."
    let cases: [RefusalCase; 3] = [
        (
            "no ids",
            &[("NumCacheNodes", "1"), ("ApplyImmediately", "true")],
            400,
            "InvalidParameterCombination",
        ),
        (
            "too few ids",
            &[
                ("NumCacheNodes", "1"),
                ("ApplyImmediately", "true"),
                ("CacheNodeIdsToRemove.CacheNodeId.1", "0002"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "unknown id",
            &[
                ("NumCacheNodes", "2"),
                ("ApplyImmediately", "true"),
                ("CacheNodeIdsToRemove.CacheNodeId.1", "0009"),
            ],
            400,
            "InvalidParameterValue",
        ),
    ];
    for (case, extra, status, code) in cases {
        let mut pairs = params(&[("CacheClusterId", "shrink")]);
        pairs.extend(params(extra));
        let reply = emulator.call("ModifyCacheCluster", &pairs).await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case}: expected {code}, got {}",
            reply.body
        );
    }

    // With the ids, the named nodes are the ones that go.
    let shrunk = emulator
        .call(
            "ModifyCacheCluster",
            &params(&[
                ("CacheClusterId", "shrink"),
                ("NumCacheNodes", "1"),
                ("ApplyImmediately", "true"),
                ("CacheNodeIdsToRemove.CacheNodeId.1", "0001"),
                ("CacheNodeIdsToRemove.CacheNodeId.2", "0002"),
            ]),
        )
        .await;
    assert_eq!(shrunk.status, 200, "the shrink must succeed: {}", shrunk.body);
    let remaining = describe_cluster(&emulator, "shrink").await;
    assert_eq!(
        remaining.all("CacheNodes/CacheNode/CacheNodeId"),
        vec!["0003"],
        "the named nodes are removed and the rest stay, got {:?}",
        remaining.all("CacheNodes/CacheNode/CacheNodeId")
    );
}

#[tokio::test]
async fn delete_reports_the_cluster_deleting_and_removes_it() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "doomed", &[]).await;
    create_available_cluster(&emulator, "survivor", &[]).await;

    let deleted = emulator
        .call(
            "DeleteCacheCluster",
            &params(&[("CacheClusterId", "Doomed")]),
        )
        .await;
    assert_eq!(deleted.status, 200, "delete must answer 200, got {}", deleted.body);
    assert_eq!(
        deleted.text("CacheCluster/CacheClusterStatus"),
        Some("deleting"),
        "case delete: the final snapshot reports the cluster deleting, got {:?}",
        deleted.text("CacheCluster/CacheClusterStatus")
    );
    assert_eq!(
        deleted.text("CacheCluster/CacheClusterId"),
        Some("doomed"),
        "case delete: the answer names the stored cluster, got {}",
        deleted.body
    );

    let gone = describe_cluster(&emulator, "doomed").await;
    assert_eq!(
        gone.status, 404,
        "case delete: the cluster is gone, got {} {}",
        gone.status, gone.body
    );
    assert_eq!(
        gone.error_code(),
        "CacheClusterNotFound",
        "case delete: expected CacheClusterNotFound, got {}",
        gone.body
    );

    let listed = emulator.call("DescribeCacheClusters", &[]).await;
    assert_eq!(
        listed.all("CacheClusters/CacheCluster/CacheClusterId"),
        vec!["survivor"],
        "case delete: the unfiltered describe no longer lists it, got {:?}",
        listed.all("CacheClusters/CacheCluster/CacheClusterId")
    );

    // `ShowCacheClustersNotInReplicationGroups` keeps the clusters that belong
    // to no replication group; `CreateCacheCluster` refuses a
    // `ReplicationGroupId`, so every stored cluster qualifies and the flag
    // excludes nothing.
    let standalone = emulator
        .call(
            "DescribeCacheClusters",
            &params(&[("ShowCacheClustersNotInReplicationGroups", "true")]),
        )
        .await;
    assert_eq!(
        standalone.all("CacheClusters/CacheCluster/CacheClusterId"),
        vec!["survivor"],
        "case standalone filter: every stored cluster belongs to no replication group, got {:?}",
        standalone.all("CacheClusters/CacheCluster/CacheClusterId")
    );
}

#[tokio::test]
async fn a_final_snapshot_is_refused_rather_than_claimed() {
    let emulator = Emulator::new();
    // The axis: the engine. Neither can take a final snapshot here — this slice
    // stores no snapshots at all — so the member is refused for both rather
    // than accepted and dropped.
    for engine in ["memcached", "redis", "valkey"] {
        let id = format!("final-{engine}");
        create_available_cluster(&emulator, &id, &[("Engine", engine)]).await;

        let refused = emulator
            .call(
                "DeleteCacheCluster",
                &params(&[
                    ("CacheClusterId", &id),
                    ("FinalSnapshotIdentifier", "last-one"),
                ]),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {engine}: expected HTTP 400, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "SnapshotFeatureNotSupportedFault",
            "case {engine}: expected SnapshotFeatureNotSupportedFault, got {}",
            refused.body
        );
        // The refused delete left the cluster in place.
        let still_there = describe_cluster(&emulator, &id).await;
        assert_eq!(
            still_there.status, 200,
            "case {engine}: a refused delete must not remove the cluster, got {}",
            still_there.body
        );

        // Without the member the delete goes through.
        let accepted = emulator
            .call("DeleteCacheCluster", &params(&[("CacheClusterId", &id)]))
            .await;
        assert_eq!(
            accepted.status, 200,
            "case {engine}: expected HTTP 200 without a final snapshot, got {} {}",
            accepted.status, accepted.body
        );
    }
}

#[tokio::test]
async fn every_stored_cluster_setting_is_reported_back() {
    // The axis: the member. Each row sets one configuration member and names
    // the element that must report it, so a member accepted and dropped fails
    // here.
    let emulator = Emulator::new();
    // The VPC security group this cluster is attached to is an Amazon EC2
    // resource, so its id is minted by the owner rather than written down here.
    let security_group = emulator
        .security_groups("cluster-settings", 1)
        .await
        .remove(0);
    let created = create_cluster(
        &emulator,
        "configured",
        &[
            ("NotificationTopicArn", "arn:aws:sns:us-east-1:000000000000:events"),
            ("AuthToken", "a-very-long-and-secret-token"),
            ("TransitEncryptionEnabled", "true"),
            ("NetworkType", "dual_stack"),
            ("IpDiscovery", "ipv6"),
            ("SnapshotRetentionLimit", "5"),
            ("SnapshotWindow", "05:00-09:00"),
            ("AutoMinorVersionUpgrade", "false"),
            ("SecurityGroupIds.SecurityGroupId.1", security_group.as_str()),
            ("LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1.LogType", "slow-log"),
            (
                "LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1.DestinationType",
                "cloudwatch-logs",
            ),
            (
                "LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1.DestinationDetails.CloudWatchLogsDetails.LogGroup",
                "/aws/elasticache/slow",
            ),
            ("LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1.LogFormat", "json"),
            ("LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1.Enabled", "true"),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the configured create must answer 200, got {}",
        created.body
    );

    let described = describe_cluster(&emulator, "configured").await;
    for (member, element, expected) in [
        (
            "NotificationTopicArn",
            "NotificationConfiguration/TopicArn",
            "arn:aws:sns:us-east-1:000000000000:events",
        ),
        (
            "NotificationTopicArn",
            "NotificationConfiguration/TopicStatus",
            "active",
        ),
        ("AuthToken", "CacheCluster/AuthTokenEnabled", "true"),
        (
            "TransitEncryptionEnabled",
            "CacheCluster/TransitEncryptionEnabled",
            "true",
        ),
        ("NetworkType", "CacheCluster/NetworkType", "dual_stack"),
        ("IpDiscovery", "CacheCluster/IpDiscovery", "ipv6"),
        (
            "SnapshotRetentionLimit",
            "CacheCluster/SnapshotRetentionLimit",
            "5",
        ),
        ("SnapshotWindow", "CacheCluster/SnapshotWindow", "05:00-09:00"),
        (
            "AutoMinorVersionUpgrade",
            "CacheCluster/AutoMinorVersionUpgrade",
            "false",
        ),
        (
            "SecurityGroupIds",
            // `SecurityGroupMembershipList`'s member carries no `@xmlName`, so
            // the element is the Smithy default `member`.
            "SecurityGroups/member/SecurityGroupId",
            security_group.as_str(),
        ),
        ("SecurityGroupIds", "SecurityGroups/member/Status", "active"),
        (
            "LogDeliveryConfigurations",
            "LogDeliveryConfiguration/LogType",
            "slow-log",
        ),
        (
            "LogDeliveryConfigurations",
            "CloudWatchLogsDetails/LogGroup",
            "/aws/elasticache/slow",
        ),
        (
            "LogDeliveryConfigurations",
            "LogDeliveryConfiguration/Status",
            "active",
        ),
    ] {
        assert_eq!(
            described.text(element),
            Some(expected),
            "case {member}: {element} must report the stored value, got {:?}",
            described.text(element)
        );
    }
    // The auth token is a secret: only the fact that one was set is reported,
    // and the token itself appears nowhere in the answer.
    assert!(
        !described.body.contains("a-very-long-and-secret-token")
            && !created.body.contains("a-very-long-and-secret-token"),
        "the auth token must never be echoed, got {}",
        described.body
    );
    assert!(
        described
            .text("CacheCluster/AuthTokenLastModifiedDate")
            .is_some(),
        "setting an auth token stamps AuthTokenLastModifiedDate, got {}",
        described.body
    );

    // A cluster that set none of these reports the flag as false rather than
    // claiming a token.
    create_cluster(&emulator, "plain", &[]).await;
    let plain = describe_cluster(&emulator, "plain").await;
    assert_eq!(
        plain.text("CacheCluster/AuthTokenEnabled"),
        Some("false"),
        "case no auth token: AuthTokenEnabled is false, got {:?}",
        plain.text("CacheCluster/AuthTokenEnabled")
    );
    assert!(
        plain.all("NotificationConfiguration/TopicArn").is_empty(),
        "case no topic: no NotificationConfiguration is reported, got {}",
        plain.body
    );
}

#[tokio::test]
async fn node_placement_follows_the_zones_the_request_asked_for() {
    let emulator = Emulator::new();

    // `single-az` (and no AZ member at all) puts every node in one zone;
    // `cross-az` spreads the NEW nodes, and a cluster whose nodes are not all
    // in one zone reports "Multiple".
    let cases: [PlacementCase; 4] = [
        ("default", &[("Engine", "memcached"), ("NumCacheNodes", "3")], 1, "us-east-1a"),
        (
            "single-az",
            &[
                ("Engine", "memcached"),
                ("NumCacheNodes", "3"),
                ("AZMode", "single-az"),
            ],
            1,
            "us-east-1a",
        ),
        (
            "cross-az",
            &[
                ("Engine", "memcached"),
                ("NumCacheNodes", "3"),
                ("AZMode", "cross-az"),
            ],
            3,
            "Multiple",
        ),
        (
            "explicit zones",
            &[
                ("Engine", "memcached"),
                ("NumCacheNodes", "2"),
                ("PreferredAvailabilityZones.PreferredAvailabilityZone.1", "us-east-1b"),
                ("PreferredAvailabilityZones.PreferredAvailabilityZone.2", "us-east-1c"),
            ],
            2,
            "Multiple",
        ),
    ];

    for (case, extra, _, _) in cases {
        let id = format!("zones-{case}").replace(' ', "-");
        let created = create_cluster(&emulator, &id, extra).await;
        assert_eq!(
            created.status, 200,
            "case {case}: create must answer 200, got {}",
            created.body
        );
    }
    // A creating cluster reports cluster-level information only, so the
    // per-node zones are read once every cluster has settled.
    std::thread::sleep(PAST_TRANSITION);

    for (case, _, distinct_zones, preferred) in cases {
        let id = format!("zones-{case}").replace(' ', "-");
        let described = describe_cluster(&emulator, &id).await;
        let mut zones = described.all("CacheNodes/CacheNode/CustomerAvailabilityZone");
        zones.sort_unstable();
        zones.dedup();
        assert_eq!(
            zones.len(),
            distinct_zones,
            "case {case}: expected {distinct_zones} distinct node zone(s), got {zones:?}"
        );
        assert_eq!(
            described.text("CacheCluster/PreferredAvailabilityZone"),
            Some(preferred),
            "case {case}: PreferredAvailabilityZone must report {preferred}, got {:?}",
            described.text("CacheCluster/PreferredAvailabilityZone")
        );
    }

    // The documented combinations that cannot hold.
    let refusals: [RefusalCase; 4] = [
        (
            "AZMode on redis",
            &[("CacheClusterId", "az-redis"), ("AZMode", "cross-az")],
            400,
            "InvalidParameterCombination",
        ),
        (
            "PreferredAvailabilityZones on redis",
            &[
                ("CacheClusterId", "azs-redis"),
                ("PreferredAvailabilityZones.PreferredAvailabilityZone.1", "us-east-1b"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "both AZ members",
            &[
                ("CacheClusterId", "az-both"),
                ("Engine", "memcached"),
                ("PreferredAvailabilityZone", "us-east-1a"),
                ("PreferredAvailabilityZones.PreferredAvailabilityZone.1", "us-east-1b"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "zone count does not match NumCacheNodes",
            &[
                ("CacheClusterId", "az-count"),
                ("Engine", "memcached"),
                ("NumCacheNodes", "3"),
                ("PreferredAvailabilityZones.PreferredAvailabilityZone.1", "us-east-1b"),
            ],
            400,
            "InvalidParameterCombination",
        ),
    ];
    for (case, pairs, status, code) in refusals {
        let reply = emulator.call("CreateCacheCluster", &params(pairs)).await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case}: expected {code}, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_member_this_emulator_cannot_honour_is_refused_not_dropped() {
    let emulator = Emulator::new();
    // Each row names a member whose effect nothing in the store can carry, so
    // the request is refused instead of answered 200 with the member ignored.
    let cases: [RefusalCase; 5] = [
        (
            "SnapshotName",
            &[("CacheClusterId", "from-snapshot"), ("SnapshotName", "nightly")],
            400,
            "InvalidParameterValue",
        ),
        (
            "SnapshotArns",
            &[
                ("CacheClusterId", "from-s3"),
                ("SnapshotArns.SnapshotArn.1", "arn:aws:s3:::bucket/snap.rdb"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "OutpostMode",
            &[("CacheClusterId", "outpost-mode"), ("OutpostMode", "single-outpost")],
            400,
            "InvalidParameterValue",
        ),
        (
            "PreferredOutpostArn",
            &[
                ("CacheClusterId", "outpost-arn"),
                (
                    "PreferredOutpostArn",
                    "arn:aws:outposts:us-east-1:000000000000:outpost/op-1",
                ),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "PreferredOutpostArns",
            &[
                ("CacheClusterId", "outpost-arns"),
                (
                    "PreferredOutpostArns.PreferredOutpostArn.1",
                    "arn:aws:outposts:us-east-1:000000000000:outpost/op-1",
                ),
            ],
            400,
            "InvalidParameterValue",
        ),
    ];
    for (case, pairs, status, code) in cases {
        let reply = emulator.call("CreateCacheCluster", &params(pairs)).await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case}: expected {code}, got {}",
            reply.body
        );
        // Nothing was stored for the refused request.
        let id = pairs
            .iter()
            .find(|(key, _)| *key == "CacheClusterId")
            .map(|(_, value)| *value)
            .expect("every row names a cluster");
        let described = describe_cluster(&emulator, id).await;
        assert_eq!(
            described.status, 404,
            "case {case}: a refused create must store nothing, got {}",
            described.body
        );
    }
}

#[tokio::test]
async fn modify_refuses_a_member_that_names_no_change_it_can_make() {
    let emulator = Emulator::new();
    create_available_cluster(
        &emulator,
        "modify-refusals",
        &[("Engine", "memcached"), ("NumCacheNodes", "2")],
    )
    .await;
    create_available_cluster(&emulator, "modify-redis", &[("Engine", "redis")]).await;

    let cases: [RefusalCase; 10] = [
        (
            "NumCacheNodes at the wire type's floor",
            &[
                ("CacheClusterId", "modify-refusals"),
                ("NumCacheNodes", "-2147483648"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "NumCacheNodes at the wire type's ceiling",
            &[
                ("CacheClusterId", "modify-refusals"),
                ("NumCacheNodes", "2147483647"),
                ("CacheNodeIdsToRemove.CacheNodeId.1", "0001"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "ids to remove without NumCacheNodes",
            &[
                ("CacheClusterId", "modify-refusals"),
                ("CacheNodeIdsToRemove.CacheNodeId.1", "0001"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "ids to remove while growing",
            &[
                ("CacheClusterId", "modify-refusals"),
                ("NumCacheNodes", "3"),
                ("CacheNodeIdsToRemove.CacheNodeId.1", "0001"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "new zones that do not match the nodes being added",
            &[
                ("CacheClusterId", "modify-refusals"),
                ("NumCacheNodes", "3"),
                ("NewAvailabilityZones.PreferredAvailabilityZone.1", "us-east-1b"),
                ("NewAvailabilityZones.PreferredAvailabilityZone.2", "us-east-1c"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "new zones on redis",
            &[
                ("CacheClusterId", "modify-redis"),
                ("NewAvailabilityZones.PreferredAvailabilityZone.1", "us-east-1b"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "new zones without ApplyImmediately",
            &[
                ("CacheClusterId", "modify-refusals"),
                ("NumCacheNodes", "3"),
                ("NewAvailabilityZones.PreferredAvailabilityZone.1", "us-east-1b"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            // The 2-node Memcached cluster cannot become a redis one: "for
            // clusters running Valkey or Redis OSS, this value must be 1".
            "engine change that leaves an illegal node count",
            &[("CacheClusterId", "modify-refusals"), ("Engine", "redis")],
            400,
            "InvalidParameterValue",
        ),
        (
            "SnapshotWindow on a memcached cluster",
            &[
                ("CacheClusterId", "modify-refusals"),
                ("SnapshotWindow", "05:00-09:00"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "SnapshotRetentionLimit on a memcached cluster",
            &[
                ("CacheClusterId", "modify-refusals"),
                ("SnapshotRetentionLimit", "5"),
            ],
            400,
            "InvalidParameterCombination",
        ),
    ];
    for (case, pairs, status, code) in cases {
        let reply = emulator.call("ModifyCacheCluster", &params(pairs)).await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case}: expected {code}, got {}",
            reply.body
        );
    }

    // With one zone for the one node being added, the new node lands there and
    // the existing nodes stay where they were.
    let grown = emulator
        .call(
            "ModifyCacheCluster",
            &params(&[
                ("CacheClusterId", "modify-refusals"),
                ("NumCacheNodes", "3"),
                ("ApplyImmediately", "true"),
                ("NewAvailabilityZones.PreferredAvailabilityZone.1", "us-east-1f"),
            ]),
        )
        .await;
    assert_eq!(grown.status, 200, "the grow must succeed: {}", grown.body);
    // The added node is not provisioned yet when the grow answers, so that
    // answer reports it `creating` with no endpoint while the two it joined
    // still have theirs.
    let growing = grown.all("CacheNodes/CacheNode/CacheNodeStatus");
    assert_eq!(
        growing,
        vec!["available", "available", "creating"],
        "the added node is still being provisioned, got {growing:?}"
    );
    assert_eq!(
        grown.all("CacheNodes/CacheNode/Endpoint/Address").len(),
        2,
        "a node being added reports no endpoint until it is provisioned, got {}",
        grown.body
    );

    // A later read is answered once the work the clock owed is done: the new
    // node landed in the named zone, the existing nodes stayed where they were,
    // and every node reports its endpoint.
    let described = describe_cluster(&emulator, "modify-refusals").await;
    let zones = described.all("CacheNodes/CacheNode/CustomerAvailabilityZone");
    assert_eq!(
        zones,
        vec!["us-east-1a", "us-east-1a", "us-east-1f"],
        "only the new node is placed in the named zone, got {zones:?}"
    );
    let statuses = described.all("CacheNodes/CacheNode/CacheNodeStatus");
    assert_eq!(
        statuses,
        vec!["available", "available", "available"],
        "every node is provisioned once the read is answered, got {statuses:?}"
    );
    assert_eq!(
        described.all("CacheNodes/CacheNode/Endpoint/Address").len(),
        3,
        "every node reports an endpoint once the cluster settles, got {}",
        described.body
    );
}

#[tokio::test]
async fn modify_applies_the_members_pending_modified_values_cannot_hold() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "applied-at-once", &[("Engine", "redis")]).await;

    // `ApplyImmediately` is absent, so only a member `PendingModifiedValues`
    // models may be deferred. `Engine`, the notification topic, `IpDiscovery`
    // and the auth token are not among them, so they apply now.
    let modified = emulator
        .call(
            "ModifyCacheCluster",
            &params(&[
                ("CacheClusterId", "applied-at-once"),
                ("Engine", "valkey"),
                ("IpDiscovery", "ipv6"),
                ("NotificationTopicArn", "arn:aws:sns:us-east-1:000000000000:later"),
                ("AuthToken", "another-very-long-secret-token"),
                ("ScaleConfig.ScalePercentage", "20"),
                ("ScaleConfig.ScaleIntervalMinutes", "15"),
            ]),
        )
        .await;
    assert_eq!(
        modified.status, 200,
        "the modify must answer 200, got {}",
        modified.body
    );

    let described = describe_cluster(&emulator, "applied-at-once").await;
    for (member, element, expected) in [
        ("Engine", "CacheCluster/Engine", "valkey"),
        ("IpDiscovery", "CacheCluster/IpDiscovery", "ipv6"),
        (
            "NotificationTopicArn",
            "NotificationConfiguration/TopicArn",
            "arn:aws:sns:us-east-1:000000000000:later",
        ),
        ("AuthToken", "CacheCluster/AuthTokenEnabled", "true"),
        // `PendingModifiedValues` is the only place the model reports a
        // `ScaleConfig`, so it is pending whatever `ApplyImmediately` says.
        ("ScaleConfig", "ScaleConfig/ScalePercentage", "20"),
    ] {
        assert_eq!(
            described.text(element),
            Some(expected),
            "case {member}: {element} must report the change, got {:?}",
            described.text(element)
        );
    }
    assert!(
        !described.body.contains("another-very-long-secret-token"),
        "the auth token must never be echoed, got {}",
        described.body
    );

    // `NotificationTopicStatus: inactive` PAUSES the notifications and leaves the
    // topic bound: "notifications are sent only if the status is active", so the
    // cluster still reports the same `TopicArn`, now `inactive`.
    let paused = emulator
        .call(
            "ModifyCacheCluster",
            &params(&[
                ("CacheClusterId", "applied-at-once"),
                ("NotificationTopicStatus", "inactive"),
            ]),
        )
        .await;
    assert_eq!(
        paused.status, 200,
        "the pause must answer 200, got {}",
        paused.body
    );
    let after = describe_cluster(&emulator, "applied-at-once").await;
    for (member, expected) in [
        (
            "NotificationConfiguration/TopicArn",
            "arn:aws:sns:us-east-1:000000000000:later",
        ),
        ("NotificationConfiguration/TopicStatus", "inactive"),
    ] {
        assert_eq!(
            after.text(member),
            Some(expected),
            "case {member}: a paused notification configuration keeps its topic and reports \
             the status asked for, got {:?} in {}",
            after.text(member),
            after.body
        );
    }

    // And `active` sets the SAME configuration going again, without the caller
    // re-supplying the ARN.
    let resumed = emulator
        .call(
            "ModifyCacheCluster",
            &params(&[
                ("CacheClusterId", "applied-at-once"),
                ("NotificationTopicStatus", "active"),
            ]),
        )
        .await;
    assert_eq!(
        resumed.status, 200,
        "the resume must answer 200, got {}",
        resumed.body
    );
    let resumed = describe_cluster(&emulator, "applied-at-once").await;
    for (member, expected) in [
        (
            "NotificationConfiguration/TopicArn",
            "arn:aws:sns:us-east-1:000000000000:later",
        ),
        ("NotificationConfiguration/TopicStatus", "active"),
    ] {
        assert_eq!(
            resumed.text(member),
            Some(expected),
            "case {member}: a re-activated configuration is the same topic, got {:?} in {}",
            resumed.text(member),
            resumed.body
        );
    }

    // `AuthTokenUpdateStrategy: DELETE` clears the flag.
    let deleted = emulator
        .call(
            "ModifyCacheCluster",
            &params(&[
                ("CacheClusterId", "applied-at-once"),
                ("AuthTokenUpdateStrategy", "DELETE"),
            ]),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "the token delete must answer 200, got {}",
        deleted.body
    );
    let cleared = describe_cluster(&emulator, "applied-at-once").await;
    assert_eq!(
        cleared.text("CacheCluster/AuthTokenEnabled"),
        Some("false"),
        "a DELETE strategy clears AuthTokenEnabled, got {:?}",
        cleared.text("CacheCluster/AuthTokenEnabled")
    );
}

#[tokio::test]
async fn an_unrelated_immediate_modify_leaves_an_earlier_pending_value_pending() {
    let emulator = Emulator::new();
    create_available_cluster(
        &emulator,
        "pending-kept",
        &[("Engine", "memcached"), ("NumCacheNodes", "1")],
    )
    .await;

    // A deferred modify records the node count; the status does not move, so a
    // second modify is accepted straight away.
    let deferred = emulator
        .call(
            "ModifyCacheCluster",
            &params(&[("CacheClusterId", "pending-kept"), ("NumCacheNodes", "3")]),
        )
        .await;
    assert_eq!(
        deferred.text("PendingModifiedValues/NumCacheNodes"),
        Some("3"),
        "the deferred count must be recorded, got {}",
        deferred.body
    );

    // An immediate modify of an unrelated member clears only what it applied.
    let unrelated = emulator
        .call(
            "ModifyCacheCluster",
            &params(&[
                ("CacheClusterId", "pending-kept"),
                ("ApplyImmediately", "true"),
                ("PreferredMaintenanceWindow", "wed:06:00-wed:07:00"),
            ]),
        )
        .await;
    assert_eq!(
        unrelated.status, 200,
        "the unrelated modify must answer 200, got {}",
        unrelated.body
    );
    let described = describe_cluster(&emulator, "pending-kept").await;
    assert_eq!(
        described.text("PendingModifiedValues/NumCacheNodes"),
        Some("3"),
        "a pending value this request did not touch must stay pending, got {}",
        described.body
    );
    assert_eq!(
        described.text("CacheCluster/PreferredMaintenanceWindow"),
        Some("wed:06:00-wed:07:00"),
        "the applied member must be applied, got {:?}",
        described.text("CacheCluster/PreferredMaintenanceWindow")
    );
    assert_eq!(
        described.text("CacheCluster/NumCacheNodes"),
        Some("1"),
        "the still-pending count must not have been applied, got {:?}",
        described.text("CacheCluster/NumCacheNodes")
    );

    // A later modify that DOES name the count applies it and clears it.
    std::thread::sleep(PAST_TRANSITION);
    let applied = emulator
        .call(
            "ModifyCacheCluster",
            &params(&[
                ("CacheClusterId", "pending-kept"),
                ("ApplyImmediately", "true"),
                ("NumCacheNodes", "2"),
            ]),
        )
        .await;
    assert_eq!(
        applied.status, 200,
        "the applied modify must answer 200, got {}",
        applied.body
    );
    let after = describe_cluster(&emulator, "pending-kept").await;
    assert_eq!(
        after.text("CacheCluster/NumCacheNodes"),
        Some("2"),
        "the named count applies, got {:?}",
        after.text("CacheCluster/NumCacheNodes")
    );
    assert!(
        after.all("PendingModifiedValues/NumCacheNodes").is_empty(),
        "applying the count clears it from PendingModifiedValues, got {}",
        after.body
    );
}

#[tokio::test]
async fn a_log_delivery_request_replaces_or_removes_the_stored_one() {
    let emulator = Emulator::new();
    create_available_cluster(
        &emulator,
        "logs",
        &[
            ("LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1.LogType", "slow-log"),
            (
                "LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1.DestinationType",
                "cloudwatch-logs",
            ),
            ("LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1.Enabled", "true"),
        ],
    )
    .await;

    // A request for the same log type replaces the stored configuration.
    let replaced = emulator
        .call(
            "ModifyCacheCluster",
            &params(&[
                ("CacheClusterId", "logs"),
                ("LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1.LogType", "slow-log"),
                (
                    "LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1.DestinationType",
                    "kinesis-firehose",
                ),
                ("LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1.Enabled", "true"),
            ]),
        )
        .await;
    assert_eq!(replaced.status, 200, "the replace must succeed: {}", replaced.body);
    let described = describe_cluster(&emulator, "logs").await;
    assert_eq!(
        described.all("LogDeliveryConfiguration/DestinationType"),
        vec!["kinesis-firehose"],
        "a request for the same log type replaces it, got {:?}",
        described.all("LogDeliveryConfiguration/DestinationType")
    );

    // `Enabled: false` removes it.
    let removed = emulator
        .call(
            "ModifyCacheCluster",
            &params(&[
                ("CacheClusterId", "logs"),
                ("LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1.LogType", "slow-log"),
                ("LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1.Enabled", "false"),
            ]),
        )
        .await;
    assert_eq!(removed.status, 200, "the removal must succeed: {}", removed.body);
    let after = describe_cluster(&emulator, "logs").await;
    assert!(
        after.all("LogDeliveryConfiguration/LogType").is_empty(),
        "a disabled request removes the configuration, got {}",
        after.body
    );
}

#[tokio::test]
async fn an_unknown_or_unsettled_cluster_is_refused() {
    let emulator = Emulator::new();

    // An id nothing holds is the not-found refusal, whichever operation asks.
    let unknown: [(&str, &str, u16, &str); 3] = [
        ("modify unknown", "ModifyCacheCluster", 404, "CacheClusterNotFound"),
        ("delete unknown", "DeleteCacheCluster", 404, "CacheClusterNotFound"),
        (
            "describe unknown",
            "DescribeCacheClusters",
            404,
            "CacheClusterNotFound",
        ),
    ];
    for (case, action, status, code) in unknown {
        let reply = emulator
            .call(action, &params(&[("CacheClusterId", "nope")]))
            .await;
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
    }

    // The state gate: a call that CHANGES a cluster still being created is
    // refused with the status the cluster has. Each case creates a cluster of
    // its own and calls straight after it, because the gate is what a mutation
    // arriving INSIDE the transition span is answered with, while a report of
    // that same cluster is answered only once the span has passed — so nothing
    // may read the cluster between its create and the call under test.
    let gated: [(&str, &str); 2] = [
        ("modify while creating", "ModifyCacheCluster"),
        ("delete while creating", "DeleteCacheCluster"),
    ];
    for (index, (case, action)) in gated.into_iter().enumerate() {
        let id = format!("gated-{index}");
        create_cluster(&emulator, &id, &[]).await;
        let reply = emulator
            .call(action, &params(&[("CacheClusterId", &id)]))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected HTTP 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidCacheClusterState",
            "case {case}: expected error code InvalidCacheClusterState, got {}",
            reply.body
        );
    }

    // A describe is a read: it reports the cluster rather than refusing it, and
    // is answered once the work the clock owed it is done.
    create_cluster(&emulator, "fresh", &[]).await;
    let described = emulator
        .call("DescribeCacheClusters", &params(&[("CacheClusterId", "fresh")]))
        .await;
    assert_eq!(
        described.status, 200,
        "case describe while creating: expected HTTP 200, got {} {}",
        described.status, described.body
    );
    assert_eq!(
        described.text("CacheCluster/CacheClusterStatus"),
        Some("available"),
        "case describe while creating: the read is answered settled, got {:?}",
        described.text("CacheCluster/CacheClusterStatus")
    );
}

/// One row of the subnet-group table.
#[derive(Debug, Clone)]
struct SubnetGroupCase {
    name: &'static str,
    stored_name: &'static str,
    description: &'static str,
    /// How many of the account's subnets (as EC2 reports them) the request
    /// puts in the group — the axis the group's size varies along, since those
    /// are the only ids the service accepts.
    subnet_count: usize,
}

#[tokio::test]
async fn a_created_subnet_group_is_read_back_from_persisted_state() {
    // The axis: the name's case, the description, and the number of subnets.
    let emulator = Emulator::new();
    let account_subnets = emulator.account_subnets().await;
    let account_subnet_ids: Vec<String> = account_subnets
        .iter()
        .map(|subnet| subnet.id.clone())
        .collect();
    let cases = [
        SubnetGroupCase {
            name: "demo-subnets",
            stored_name: "demo-subnets",
            description: "demo",
            subnet_count: 2,
        },
        SubnetGroupCase {
            name: "Single-Subnet",
            stored_name: "single-subnet",
            description: "one subnet, mixed-case name",
            subnet_count: 1,
        },
        SubnetGroupCase {
            name: "wide-group",
            stored_name: "wide-group",
            description: "every subnet the account has",
            subnet_count: account_subnets.len(),
        },
    ];

    for case in &cases {
        let subnets = &account_subnet_ids[..case.subnet_count];
        let pairs = subnet_group_params(case.name, case.description, subnets);
        let created = emulator.call("CreateCacheSubnetGroup", &pairs).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: create must answer 200, got {}",
            created.body
        );

        let described = emulator
            .call(
                "DescribeCacheSubnetGroups",
                &params(&[("CacheSubnetGroupName", case.name)]),
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: describe must answer 200, got {}",
            described.body
        );
        assert_eq!(
            described.text("CacheSubnetGroup/CacheSubnetGroupName"),
            Some(case.stored_name),
            "case {case:?}: the name is stored lowercase, got {:?}",
            described.text("CacheSubnetGroup/CacheSubnetGroupName")
        );
        assert_eq!(
            described.text("CacheSubnetGroup/CacheSubnetGroupDescription"),
            Some(case.description),
            "case {case:?}: the description must round-trip, got {:?}",
            described.text("CacheSubnetGroup/CacheSubnetGroupDescription")
        );
        assert_eq!(
            described.all("Subnets/Subnet/SubnetIdentifier"),
            subnets.iter().map(String::as_str).collect::<Vec<&str>>(),
            "case {case:?}: the stored subnets must round-trip in order, got {:?}",
            described.all("Subnets/Subnet/SubnetIdentifier")
        );
        // The zone of each subnet is the zone that subnet SITS in, read off the
        // account's network rather than chosen by position.
        let expected_zones: Vec<String> = account_subnets
            .iter()
            .take(case.subnet_count)
            .map(|subnet| subnet.zone.clone())
            .collect();
        assert_eq!(
            described.all("SubnetAvailabilityZone/Name"),
            expected_zones
                .iter()
                .map(String::as_str)
                .collect::<Vec<&str>>(),
            "case {case:?}: every subnet reports its own zone, got {:?}",
            described.all("SubnetAvailabilityZone/Name")
        );
        // Stored once: the create answer and the describe answer must agree.
        for stored in ["CacheSubnetGroup/VpcId", "CacheSubnetGroup/ARN"] {
            assert_eq!(
                created.text(stored),
                described.text(stored),
                "case {case:?}: {stored} must be stored once, got {:?} then {:?}",
                created.text(stored),
                described.text(stored)
            );
        }
        assert_eq!(
            described.text("CacheSubnetGroup/ARN"),
            Some(
                format!(
                    "arn:aws:elasticache:{REGION}:{}:subnetgroup:{}",
                    engine::emulator_account_id(),
                    case.stored_name
                )
                .as_str()
            ),
            "case {case:?}: the ARN must name the group, got {:?}",
            described.text("CacheSubnetGroup/ARN")
        );
    }

    // Every group is listed, and each reports the VPC its subnets belong to —
    // the VPC the service that OWNS subnets reports for them, not an id invented
    // per group.
    let listed = emulator.call("DescribeCacheSubnetGroups", &[]).await;
    let names = listed.all("CacheSubnetGroups/CacheSubnetGroup/CacheSubnetGroupName");
    assert_eq!(
        names.len(),
        cases.len(),
        "the unfiltered describe lists every stored group, got {names:?}"
    );
    let vpc_ids: BTreeMap<&str, ()> = listed
        .all("CacheSubnetGroup/VpcId")
        .into_iter()
        .map(|id| (id, ()))
        .collect();
    let default_vpc = account_subnets[0].vpc.clone();
    assert_eq!(
        vpc_ids.keys().copied().collect::<Vec<&str>>(),
        vec![default_vpc.as_str()],
        "every group's subnets belong to the VPC EC2 reports for them, got {vpc_ids:?}"
    );
}

#[tokio::test]
async fn a_subnet_group_request_that_cannot_hold_is_refused() {
    let emulator = Emulator::new();
    let account_subnets = account_subnet_ids(&emulator).await;
    let first = emulator
        .call(
            "CreateCacheSubnetGroup",
            &subnet_group_params("taken", "first", &account_subnets[..1]),
        )
        .await;
    assert_eq!(first.status, 200, "the first create must succeed: {}", first.body);

    // Each row: the case name, the request, and the status plus code it must be
    // answered with. The rows are built rather than listed, because a subnet id
    // the account HAS is one EC2 minted.
    let cases: [BuiltRefusalCase; 7] = [
        (
            "duplicate name",
            subnet_group_params("TAKEN", "second", &account_subnets[1..2]),
            400,
            "CacheSubnetGroupAlreadyExists",
        ),
        (
            "absent SubnetIds",
            params(&[
                ("CacheSubnetGroupName", "empty"),
                ("CacheSubnetGroupDescription", "none"),
            ]),
            400,
            // `SubnetIds` is `@required`, so the absence is refused before the
            // handler runs — as the bad parameter value this service states it
            // (`service::absent_parameter`).
            "InvalidParameterValue",
        ),
        (
            "an empty subnet id",
            subnet_group_params("empty-id", "none", &[String::new()]),
            400,
            "InvalidSubnet",
        ),
        (
            "not a subnet id",
            subnet_group_params("bad-id", "bad", &["vpc-11111111".to_string()]),
            400,
            "InvalidSubnet",
        ),
        (
            // Subnet-SHAPED but naming no subnet of the account: the account
            // lookup refuses it, not the form check.
            "a subnet that does not exist",
            subnet_group_params(
                "ghost-subnet",
                "ghost",
                &["subnet-0123456789abcdef0".to_string()],
            ),
            400,
            "InvalidParameterValue",
        ),
        (
            "one nonexistent subnet among real ones",
            subnet_group_params(
                "mixed-ids",
                "mixed",
                &[account_subnets[0].clone(), "subnet-00000000deadbeef".to_string()],
            ),
            400,
            "InvalidParameterValue",
        ),
        (
            "one bad subnet id among good ones",
            subnet_group_params(
                "mixed-forms",
                "mixed",
                &[account_subnets[0].clone(), "not-a-subnet".to_string()],
            ),
            400,
            "InvalidSubnet",
        ),
    ];
    for (case, pairs, status, code) in cases {
        let reply = emulator.call("CreateCacheSubnetGroup", &pairs).await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case}: expected {code}, got {}",
            reply.body
        );
    }

    // The duplicate did not overwrite the stored group.
    let described = emulator
        .call(
            "DescribeCacheSubnetGroups",
            &params(&[("CacheSubnetGroupName", "taken")]),
        )
        .await;
    assert_eq!(
        described.text("CacheSubnetGroup/CacheSubnetGroupDescription"),
        Some("first"),
        "a refused duplicate must leave the stored group alone, got {}",
        described.body
    );

    let unknown = emulator
        .call(
            "DescribeCacheSubnetGroups",
            &params(&[("CacheSubnetGroupName", "nope")]),
        )
        .await;
    assert_eq!(
        unknown.status, 400,
        "case describe unknown: expected HTTP 400, got {} {}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.error_code(),
        "CacheSubnetGroupNotFoundFault",
        "case describe unknown: expected CacheSubnetGroupNotFoundFault, got {}",
        unknown.body
    );
}

#[tokio::test]
async fn subnet_group_pages_are_bounded_and_resumable() {
    let emulator = Emulator::new();
    let account_subnets = account_subnet_ids(&emulator).await;
    // One more than the smallest page size the service serves.
    let total = 21;
    for index in 0..total {
        let reply = emulator
            .call(
                "CreateCacheSubnetGroup",
                &subnet_group_params(
                    &format!("group-{index:03}"),
                    "paged",
                    &account_subnets[..1],
                ),
            )
            .await;
        assert_eq!(reply.status, 200, "group {index} must be created: {}", reply.body);
    }

    let first = emulator
        .call("DescribeCacheSubnetGroups", &params(&[("MaxRecords", "20")]))
        .await;
    let page_one = first.all("CacheSubnetGroups/CacheSubnetGroup/CacheSubnetGroupName");
    assert_eq!(
        page_one.len(),
        20,
        "case page 1: expected 20 groups, got {page_one:?}"
    );
    let marker = first
        .marker()
        .expect("a 21st group exists, so a marker must be issued")
        .to_string();

    let second = emulator
        .call(
            "DescribeCacheSubnetGroups",
            &params(&[("MaxRecords", "20"), ("Marker", &marker)]),
        )
        .await;
    let page_two = second.all("CacheSubnetGroups/CacheSubnetGroup/CacheSubnetGroupName");
    assert_eq!(
        page_two,
        vec!["group-020"],
        "case page 2: the marker resumes after page 1, got {page_two:?}"
    );
    assert!(
        second.marker().is_none(),
        "case page 2: the last page carries no marker, got {}",
        second.body
    );

    // A refused page size is refused here too, through the operation's own
    // channel.
    let refused = emulator
        .call("DescribeCacheSubnetGroups", &params(&[("MaxRecords", "5")]))
        .await;
    assert_eq!(
        refused.status, 400,
        "case MaxRecords=5: must be refused 400, got {} {}",
        refused.status, refused.body
    );
}

#[tokio::test]
async fn the_documented_identifier_port_and_tag_constraints_are_enforced() {
    let emulator = Emulator::new();

    // `CacheClusterId`: "A name must contain from 1 to 50 alphanumeric
    // characters or hyphens. The first character must be a letter. A name
    // cannot end with a hyphen or contain two consecutive hyphens." Each clause
    // is a row, with the length bound and one past it.
    let fifty = "a".repeat(50);
    let fifty_one = "a".repeat(51);
    let id_cases: [(&str, bool); 9] = [
        ("a", true),
        ("demo-cluster-1", true),
        (fifty.as_str(), true),
        (fifty_one.as_str(), false),
        ("", false),
        ("1abc", false),
        ("-abc", false),
        ("abc-", false),
        ("a--b", false),
    ];
    for (id, accepted) in id_cases {
        let reply = create_cluster(&emulator, id, &[]).await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case id {id:?}: must be accepted, got {} {}",
                reply.status, reply.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case id {id:?}: must be refused 400, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "InvalidParameterValue",
                "case id {id:?}: expected InvalidParameterValue, got {}",
                reply.body
            );
        }
    }

    // `Port` is "the port number on which each of the cache nodes accepts
    // connections" — a TCP port, so each end of the range is a row and so is
    // one past it.
    let port_cases: [(&str, bool); 5] = [
        ("1", true),
        ("65535", true),
        ("0", false),
        ("65536", false),
        ("-1", false),
    ];
    for (index, (port, accepted)) in port_cases.iter().enumerate() {
        let id = format!("port-{index}");
        let reply = create_cluster(&emulator, &id, &[("Port", port)]).await;
        if *accepted {
            assert_eq!(
                reply.status, 200,
                "case port {port}: must be accepted, got {} {}",
                reply.status, reply.body
            );
            // A `Port` inside the range is ACCEPTED; what the cluster then
            // publishes is the port its own socket answers on, which the core
            // assigns (see `a_created_cluster_is_read_back_from_persisted_state`).
            // A `Port` inside the range is ACCEPTED; what the cluster then
            // publishes is the port its own socket answers on, which the core
            // assigns — the same port every later read of the cluster reports,
            // which is what this asserts rather than any port at all.
            let published = reply
                .text("ConfigurationEndpoint/Port")
                .map(str::to_string);
            assert!(
                published
                    .as_deref()
                    .and_then(|port| port.parse::<u16>().ok())
                    .is_some_and(|port| port > 0),
                "case port {port}: the endpoint must carry the port the cache answers on, got \
                 {published:?}"
            );
            let described = describe_cluster(&emulator, &id).await;
            assert_eq!(
                described.text("ConfigurationEndpoint/Port"),
                published.as_deref(),
                "case port {port}: every read must report the one port the cache answers on, got \
                 {:?} then {:?}",
                published,
                described.text("ConfigurationEndpoint/Port")
            );
            assert!(
                described
                    .all("CacheNodes/CacheNode/Endpoint/Port")
                    .iter()
                    .all(|node| Some(*node) == published.as_deref()),
                "case port {port}: every node must report that same port, got {:?}",
                described.all("CacheNodes/CacheNode/Endpoint/Port")
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case port {port}: must be refused 400, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "InvalidParameterValue",
                "case port {port}: expected InvalidParameterValue, got {}",
                reply.body
            );
        }
    }

    // "The maximum number of tags permitted on a resource is 50": the bound is a
    // row and so is one past it.
    for (count, accepted) in [(50usize, true), (51usize, false)] {
        let mut pairs = params(&[("CacheClusterId", &format!("tags-{count}"))]);
        for index in 1..=count {
            pairs.push((format!("Tags.Tag.{index}.Key"), format!("k{index}")));
            pairs.push((format!("Tags.Tag.{index}.Value"), format!("v{index}")));
        }
        let reply = emulator.call("CreateCacheCluster", &pairs).await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case {count} tags: must be accepted, got {} {}",
                reply.status, reply.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {count} tags: must be refused 400, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "TagQuotaPerResourceExceeded",
                "case {count} tags: expected TagQuotaPerResourceExceeded, got {}",
                reply.body
            );
        }
    }

    // `Tag$Key` "may not be null" while the value may be, so a keyless tag is
    // refused rather than dropped.
    let keyless = emulator
        .call(
            "CreateCacheCluster",
            &params(&[
                ("CacheClusterId", "keyless-tag"),
                ("Tags.Tag.1.Value", "orphan"),
            ]),
        )
        .await;
    assert_eq!(
        keyless.status, 400,
        "case keyless tag: must be refused 400, got {} {}",
        keyless.status, keyless.body
    );
    assert_eq!(
        keyless.error_code(),
        "InvalidParameterValue",
        "case keyless tag: expected InvalidParameterValue, got {}",
        keyless.body
    );
}

#[tokio::test]
async fn the_documented_subnet_group_constraints_are_enforced() {
    let emulator = Emulator::new();
    let account_subnets = account_subnet_ids(&emulator).await;

    // `CacheSubnetGroupName` "must contain no more than 255 alphanumeric
    // characters or hyphens": the bound is a row and so is one past it. This
    // operation models no invalid-parameter error for the NAME, so a refusal is
    // the framework's `ValidationException`.
    let longest = "g".repeat(255);
    let too_long = "g".repeat(256);
    let name_cases: [(&str, bool); 5] = [
        ("demo-subnets", true),
        (longest.as_str(), true),
        (too_long.as_str(), false),
        ("has space", false),
        ("slash/name", false),
    ];
    for (name, accepted) in name_cases {
        let reply = emulator
            .call(
                "CreateCacheSubnetGroup",
                &subnet_group_params(name, "constraint row", &account_subnets[..1]),
            )
            .await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case name {name:?}: must be accepted, got {} {}",
                reply.status, reply.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case name {name:?}: must be refused 400, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "ValidationException",
                "case name {name:?}: expected ValidationException, got {}",
                reply.body
            );
        }
    }

    // The per-group subnet bound: the account's whole network fits in one
    // group, and one subnet past the bound is refused. The over-bound row names
    // subnet-SHAPED ids the account does not have, because the account has far
    // fewer subnets than the bound: the COUNT is what this row is about, and the
    // handler checks it before it resolves the ids.
    assert!(
        state::SUBNETS_PER_GROUP > account_subnets.len(),
        "the bound is unreachable with ids the account has ({} subnets vs a bound of {}), which \
         is why the over-bound row names ids that resolve to nothing",
        account_subnets.len(),
        state::SUBNETS_PER_GROUP
    );
    for (count, accepted) in [
        (account_subnets.len(), true),
        (state::SUBNETS_PER_GROUP + 1, false),
    ] {
        let mut pairs = params(&[
            ("CacheSubnetGroupName", &format!("subnets-{count}")),
            ("CacheSubnetGroupDescription", "bound row"),
        ]);
        let ids: Vec<String> = if accepted {
            account_subnets.clone()
        } else {
            (1..=count).map(|index| format!("subnet-{index:08x}")).collect()
        };
        pairs.extend(subnet_pairs("SubnetIdentifier", &ids));
        let reply = emulator.call("CreateCacheSubnetGroup", &pairs).await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case {count} subnets: must be accepted, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.all("Subnets/Subnet/SubnetIdentifier").len(),
                count,
                "case {count} subnets: every subnet must be stored, got {:?}",
                reply.all("Subnets/Subnet/SubnetIdentifier")
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {count} subnets: must be refused 400, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "CacheSubnetQuotaExceededFault",
                "case {count} subnets: expected CacheSubnetQuotaExceededFault, got {}",
                reply.body
            );
        }
    }

    // A subnet cannot be in one group twice.
    let repeated = emulator
        .call(
            "CreateCacheSubnetGroup",
            &subnet_group_params(
                "repeated-subnet",
                "duplicate",
                &[account_subnets[0].clone(), account_subnets[0].clone()],
            ),
        )
        .await;
    assert_eq!(
        repeated.status, 400,
        "case repeated subnet: must be refused 400, got {} {}",
        repeated.status, repeated.body
    );
    assert_eq!(
        repeated.error_code(),
        "InvalidSubnet",
        "case repeated subnet: expected InvalidSubnet, got {}",
        repeated.body
    );

    // The same 50-tag bound as a cluster.
    let mut pairs = subnet_group_params("too-many-tags", "tags", &account_subnets[..1]);
    for index in 1..=51 {
        pairs.push((format!("Tags.Tag.{index}.Key"), format!("k{index}")));
    }
    let tagged = emulator.call("CreateCacheSubnetGroup", &pairs).await;
    assert_eq!(
        tagged.status, 400,
        "case 51 tags: must be refused 400, got {} {}",
        tagged.status, tagged.body
    );
    assert_eq!(
        tagged.error_code(),
        "TagQuotaPerResourceExceeded",
        "case 51 tags: expected TagQuotaPerResourceExceeded, got {}",
        tagged.body
    );
}

#[tokio::test]
async fn arns_and_endpoints_name_the_region_the_request_was_signed_for() {
    // The axis: the signing region, which is where the resource lives.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let emulator = Emulator::new();
        let created = emulator
            .call_in(
                region,
                "CreateCacheCluster",
                &params(&[("CacheClusterId", "regional")]),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {region}: create must answer 200, got {}",
            created.body
        );
        assert_eq!(
            created.text("CacheCluster/ARN"),
            Some(
                format!(
                    "arn:aws:elasticache:{region}:{}:cluster:regional",
                    engine::emulator_account_id()
                )
                .as_str()
            ),
            "case {region}: the ARN must carry the signing region, got {:?}",
            created.text("CacheCluster/ARN")
        );
        // The ENDPOINT is not regional: a cache this emulator serves a RESP data
        // plane for is reached at the address its own socket answers on, which is
        // where the emulator runs rather than where the request was signed. What
        // the signing region decides is the ARN above and the zone below.
        assert!(
            created
                .text("ConfigurationEndpoint/Address")
                .is_some_and(is_own_host),
            "case {region}: the endpoint must be an address the emulator answers on, got {:?}",
            created.text("ConfigurationEndpoint/Address")
        );
        assert!(
            created
                .text("CacheCluster/PreferredAvailabilityZone")
                .is_some_and(|zone| zone.starts_with(region)),
            "case {region}: the chosen zone must be in the signing region, got {:?}",
            created.text("CacheCluster/PreferredAvailabilityZone")
        );
    }
}

/// One reboot row: the cluster to create, the nodes the reboot names, and the
/// `CacheNodeStatus` every node of the cluster must then report, in node-id
/// order.
#[derive(Debug, Clone)]
struct RebootCase {
    id: &'static str,
    /// The form pairs sent beside `CacheClusterId` on the create.
    extra: &'static [(&'static str, &'static str)],
    /// `CacheNodeIdsToReboot`.
    ids_to_reboot: &'static [&'static str],
    /// What each node reports while the reboot is in flight.
    node_statuses: &'static [&'static str],
}

/// Reboot `ids` of the cluster `id`. An empty `ids` sends no
/// `CacheNodeIdsToReboot` member at all.
async fn reboot_cluster(emulator: &Emulator, id: &str, ids: &[&str]) -> Reply {
    let mut pairs = params(&[("CacheClusterId", id)]);
    for (index, node) in ids.iter().enumerate() {
        pairs.push((
            // `CacheNodeIdsList`'s member carries `xmlName: CacheNodeId`
            // (`smithy_query(query="shape", shape="CacheNodeIdsList")`), which is
            // the name an awsQuery client flattens the list under.
            format!("CacheNodeIdsToReboot.CacheNodeId.{}", index + 1),
            (*node).to_string(),
        ));
    }
    emulator.call("RebootCacheCluster", &pairs).await
}

#[tokio::test]
async fn a_reboot_moves_only_the_named_nodes_and_settles_back() {
    // The axis: the engine, the number of nodes the cluster has, and how many
    // of them the request names — one, some, or all ("to reboot an entire
    // cluster, specify all of the cache node IDs"). The recorded single-node
    // Valkey / Redis OSS cluster is one row.
    let cases = [
        RebootCase {
            id: "reboot-redis",
            extra: &[("Engine", "redis")],
            ids_to_reboot: &["0001"],
            node_statuses: &["rebooting"],
        },
        RebootCase {
            id: "reboot-valkey",
            extra: &[("Engine", "valkey")],
            ids_to_reboot: &["0001"],
            node_statuses: &["rebooting"],
        },
        RebootCase {
            id: "reboot-one-of-three",
            extra: &[("Engine", "memcached"), ("NumCacheNodes", "3")],
            ids_to_reboot: &["0002"],
            node_statuses: &["available", "rebooting", "available"],
        },
        RebootCase {
            id: "reboot-two-of-four",
            extra: &[("Engine", "memcached"), ("NumCacheNodes", "4")],
            ids_to_reboot: &["0001", "0004"],
            node_statuses: &["rebooting", "available", "available", "rebooting"],
        },
        RebootCase {
            id: "reboot-all-of-three",
            extra: &[("Engine", "memcached"), ("NumCacheNodes", "3")],
            ids_to_reboot: &["0001", "0002", "0003"],
            node_statuses: &["rebooting", "rebooting", "rebooting"],
        },
    ];

    let emulator = Emulator::new();
    for case in cases {
        create_available_cluster(&emulator, case.id, case.extra).await;
        let node_count = case.node_statuses.len();

        let rebooted = reboot_cluster(&emulator, case.id, case.ids_to_reboot).await;
        assert_eq!(
            rebooted.status, 200,
            "case {case:?}: the reboot must succeed, got {} {}",
            rebooted.status, rebooted.body
        );
        // "During the reboot, the cluster status is set to REBOOTING", which
        // `CacheClusterStatus`'s value list spells `rebooting cluster nodes`.
        assert_eq!(
            rebooted.text("CacheCluster/CacheClusterStatus"),
            Some("rebooting cluster nodes"),
            "case {case:?}: expected the cluster rebooting, got {:?}",
            rebooted.text("CacheCluster/CacheClusterStatus")
        );
        assert_eq!(
            rebooted.all("CacheNodes/CacheNode/CacheNodeStatus"),
            case.node_statuses,
            "case {case:?}: expected only the named nodes rebooting, got {:?}",
            rebooted.all("CacheNodes/CacheNode/CacheNodeStatus")
        );

        // Persisted, not merely reported: the next read reads the cluster back
        // node for node — answered once the reboot the clock owed is over, so
        // the cluster and every node it restarted are available again — and the
        // reboot changed nothing else about the cluster.
        let settled = describe_cluster(&emulator, case.id).await;
        assert_eq!(
            settled.text("CacheCluster/CacheClusterStatus"),
            Some("available"),
            "case {case:?}: the read is answered once the reboot is over, got {:?}",
            settled.text("CacheCluster/CacheClusterStatus")
        );
        let reported_ids = settled.all("CacheNodes/CacheNode/CacheNodeId");
        let expected_ids = (1..=node_count)
            .map(|ordinal| format!("{ordinal:04}"))
            .collect::<Vec<String>>();
        assert_eq!(
            reported_ids, expected_ids,
            "case {case:?}: a reboot removes no node, got {reported_ids:?}"
        );
        assert_eq!(
            settled.text("CacheCluster/NumCacheNodes"),
            Some(node_count.to_string().as_str()),
            "case {case:?}: a reboot changes no node count, got {:?}",
            settled.text("CacheCluster/NumCacheNodes")
        );
        let settled_statuses = settled.all("CacheNodes/CacheNode/CacheNodeStatus");
        assert_eq!(
            settled_statuses,
            vec!["available"; node_count],
            "case {case:?}: every node settles with the cluster, got {settled_statuses:?}"
        );
    }
}

#[tokio::test]
async fn a_reboot_that_cannot_be_performed_is_refused() {
    let emulator = Emulator::new();
    create_available_cluster(
        &emulator,
        "reboot-refusals",
        &[("Engine", "memcached"), ("NumCacheNodes", "2")],
    )
    .await;
    // Created last and not waited out, so it is still `creating`: the span is
    // never zero.
    create_cluster(&emulator, "reboot-fresh", &[]).await;

    // `(case, cluster id, CacheNodeIdsToReboot, HTTP status, awsQuery code)`.
    let cases: [(&str, &str, &[&str], u16, &str); 5] = [
        (
            "unknown cluster",
            "nope",
            &["0001"],
            404,
            "CacheClusterNotFound",
        ),
        (
            "cluster still creating",
            "reboot-fresh",
            &["0001"],
            400,
            "InvalidCacheClusterState",
        ),
        (
            "a node the cluster does not hold",
            "reboot-refusals",
            &["0009"],
            400,
            "ValidationException",
        ),
        (
            "an empty node id",
            "reboot-refusals",
            &[""],
            400,
            "ValidationException",
        ),
        (
            // `CacheNodeIdsToReboot` is `@required`, so the absence is refused
            // before the handler runs, as the bad parameter value this service
            // states it (`service::absent_parameter`).
            "absent CacheNodeIdsToReboot",
            "reboot-refusals",
            &[],
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, id, ids, status, code) in cases {
        let reply = reboot_cluster(&emulator, id, ids).await;
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
    }

    // A refused reboot rebooted nothing: the cluster it named is still settled.
    let untouched = describe_cluster(&emulator, "reboot-refusals").await;
    assert_eq!(
        untouched.text("CacheCluster/CacheClusterStatus"),
        Some("available"),
        "a refused reboot leaves the cluster available, got {:?}",
        untouched.text("CacheCluster/CacheClusterStatus")
    );
    assert_eq!(
        untouched.all("CacheNodes/CacheNode/CacheNodeStatus"),
        vec!["available", "available"],
        "a refused reboot leaves every node available, got {:?}",
        untouched.all("CacheNodes/CacheNode/CacheNodeStatus")
    );

    // A cluster already rebooting "is not in the available state", so a second
    // reboot inside the span is refused rather than restarting the first.
    let first = reboot_cluster(&emulator, "reboot-refusals", &["0001"]).await;
    assert_eq!(
        first.status, 200,
        "the first reboot must succeed, got {} {}",
        first.status, first.body
    );
    let second = reboot_cluster(&emulator, "reboot-refusals", &["0002"]).await;
    assert_eq!(
        second.status, 400,
        "a reboot of a rebooting cluster is refused, got {} {}",
        second.status, second.body
    );
    assert_eq!(
        second.error_code(),
        "InvalidCacheClusterState",
        "a reboot of a rebooting cluster is refused with the modelled state fault, got {}",
        second.body
    );
}

#[tokio::test]
async fn the_transition_span_is_the_one_the_state_module_publishes() {
    // The handlers store a due instant rather than sleeping, so the span is a
    // published constant the tests wait out — and it is never zero, which is
    // what makes the transient status observable at all.
    assert!(
        state::CLUSTER_TRANSITION > Duration::ZERO,
        "the transition span must not be zero, got {:?}",
        state::CLUSTER_TRANSITION
    );
    assert!(
        PAST_TRANSITION > state::CLUSTER_TRANSITION,
        "the tests must wait past the span: {:?} vs {:?}",
        PAST_TRANSITION,
        state::CLUSTER_TRANSITION
    );
}
