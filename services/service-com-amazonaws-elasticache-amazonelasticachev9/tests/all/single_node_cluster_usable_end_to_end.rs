// SPDX-License-Identifier: CC0-1.0

//! A SINGLE-NODE cache cluster from the call that provisions it to the value a client stores
//! in it and reads back, as ONE sequence — the whole flow a client of a cache runs.
//!
//! RULE: a cache cluster whose engine speaks RESP is usable as soon as it reports itself
//! settled. The call that creates it answers the transient status with cluster-level
//! information only ("if the cluster is in the creating state, only cluster-level information
//! is displayed"); the FIRST read taken once the span the clock owes that cluster has passed
//! reports `available` TOGETHER with its node list, each node carrying the status `available`
//! and the `Endpoint` the model documents as "the information required for client programs to
//! connect to a cache node" (`Endpoint$Address` "the DNS hostname of the cache node",
//! `Endpoint$Port` "the port number that the cache engine is listening on"); and that very
//! address and port is one this emulator accepts a connection on and serves the cluster's own
//! keyspace at, from the vantage point that endpoint is reachable from — a cache PLACED in a
//! VPC of the account publishes the address its node holds there, which a client inside that
//! VPC reaches, and a cache in no VPC publishes the address its socket answers at. A client
//! therefore needs no read beyond the one that told it the cluster was
//! ready: what that read published is what it dials, and what answers there keeps the value
//! it writes.
//!
//! The rows differ along what the rule generalises over: WHICH engine of the two that speak
//! the wire this emulator serves the cluster runs, which PLACEMENT it was given (a cache
//! subnet group over two of the account's own subnets, guarded by its VPC's security group,
//! against a request that names no group at all), and the SHAPE of the value stored in it
//! (plain text, no bytes at all, multi-byte text, a kibibyte). The recorded single-node
//! `SET` / `GET` is one row of that table.
//!
//! What a row does NOT assert is the VALUE of the published port: the port a cache of this
//! emulator answers on is the one the core assigned the socket it opened for that cache, so
//! the port is a per-run value and the requirement over it is the one asserted here — that
//! whatever is published is a port the emulator accepts a connection on and serves this
//! cache's keyspace at. Nor does a row say anything about `CacheCluster$ConfigurationEndpoint`,
//! which the model documents for the cluster endpoint of a Memcached cluster ("will always have
//! `.cfg` in it"): the endpoint a client of a cache cluster running the wire this suite speaks
//! dials is its NODE's, which is the one the rule is about.
//!
//! The sibling suites take the halves this one joins, and what they already pin is NOT
//! repeated here — the engines and ports a cache may be created with, and one cache's keyspace
//! being unreachable from another's endpoint, are theirs:
//! [`crate::first_read_sees_the_settled_state`] is about what the first read past a due change
//! answers for every resource of this service, [`crate::cache_data_plane`] about every KIND of
//! endpoint a cache publishes being an address its RESP server answers at, and
//! `core/router/tests/all/elasticache_data_plane_reached_from_an_in_vpc_function.rs` about the
//! only vantage point a VPC-only cache endpoint is reachable from. What is asserted here is
//! that one client doing all of it in order gets through: nothing in the sequence needs a
//! second read, a second create or a wait of its own.
//!
//! Nothing here sleeps for a span: the cluster is waited for by LOOKING AGAIN at the condition
//! (the crate's own `poll_until`), which is what a client polling a status does, so what this
//! suite asserts does not depend on how fast the host runs. Every assertion goes over the wire
//! through [`crate::harness`] or over a real socket opened at the published address.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::cache_cluster_lifecycle::subnet_group_params;
use crate::cache_data_plane::{command, open, open_in_vpc, Resp, Session};
use crate::harness::{assert_own_endpoint, params, poll_until, Emulator, REGION};

/// The cache subnet group the placed row is created in: the account's own subnets, in two
/// Availability Zones, which is the placement a client gives a cache of its own.
const SUBNET_GROUP: &str = "single-node-flow-subnets";

/// A key no row ever writes, so what a cache answers for it is the absence of a value.
const NEVER_WRITTEN: &str = "flow:never-written";

/// One row: the cluster to stand up, and what a client stores in it once it is ready.
struct Row {
    /// The `CacheClusterId`, which also names the case.
    id: &'static str,
    /// What the create sends beside the identity: the engine, and the node count and type
    /// where the row names them.
    extra: &'static [(&'static str, &'static str)],
    /// Whether the cluster is placed in [`SUBNET_GROUP`] — a cache of a client's own VPC — or
    /// left to the placement the service assigns a request that names no group.
    placed: bool,
    /// The keys the session writes, each with the value it must read back.
    values: Vec<(&'static str, String)>,
}

#[tokio::test]
async fn a_single_node_cluster_serves_the_endpoint_its_first_available_read_published() {
    let emulator = Emulator::new();

    // The placement is the account's OWN network, asked of the service that owns subnets and
    // security groups rather than written down here.
    let reported = emulator.account_subnets().await;
    let placement: Vec<String> = reported
        .iter()
        .take(2)
        .map(|subnet| subnet.id.clone())
        .collect();
    let vpc = reported
        .first()
        .map(|subnet| subnet.vpc.clone())
        .expect("the account must have a subnet to place a cache in");
    let guarded_by = emulator.default_security_group_of(&vpc).await;
    let created_group = emulator
        .call(
            "CreateCacheSubnetGroup",
            &subnet_group_params(SUBNET_GROUP, "the cache's own subnets", &placement),
        )
        .await;
    assert_eq!(
        created_group.status, 200,
        "setup: CreateCacheSubnetGroup over {placement:?} must answer 200, got {} {}",
        created_group.status, created_group.body
    );

    let rows = [
        Row {
            id: "flow-redis-micro",
            extra: &[
                ("Engine", "redis"),
                ("CacheNodeType", "cache.t3.micro"),
                ("NumCacheNodes", "1"),
            ],
            placed: true,
            values: vec![
                (
                    "flow:plain",
                    "stored at the address the control plane published".to_string(),
                ),
                ("flow:no-bytes", String::new()),
            ],
        },
        Row {
            id: "flow-valkey-unplaced",
            extra: &[("Engine", "valkey")],
            placed: false,
            values: vec![
                (
                    "flow:multi-byte",
                    "a value with a — dash, a ✓ and a ß".to_string(),
                ),
                ("flow:kibibyte", "v".repeat(1024)),
            ],
        },
    ];

    // Every row is created FIRST and waited for afterwards: the span the clock owes each
    // cluster runs for all of them at once, so the suite waits for one span rather than one
    // per row.
    for row in &rows {
        let mut request = crate::harness::cluster_create_params(row.id, row.extra);
        if row.placed {
            request.extend(params(&[
                ("CacheSubnetGroupName", SUBNET_GROUP),
                ("SecurityGroupIds.member.1", guarded_by.as_str()),
            ]));
        }
        let created = emulator.call("CreateCacheCluster", &request).await;
        assert_eq!(
            created.status, 200,
            "case {}: CreateCacheCluster must answer 200, got {} {}",
            row.id, created.status, created.body
        );
        assert_eq!(
            created.text("CacheCluster/CacheClusterStatus"),
            Some(state::STATUS_CREATING),
            "case {}: the create must report the transient status {:?}, got {:?}",
            row.id,
            state::STATUS_CREATING,
            created.text("CacheCluster/CacheClusterStatus")
        );
        assert!(
            created.all("CacheNodes/CacheNode/CacheNodeId").is_empty(),
            "case {}: a cluster still being created reports cluster-level information only, \
             got {}",
            row.id,
            created.body
        );
    }

    for row in &rows {
        // The ONE read the client waits for: the first answer that reports the cluster
        // settled. Everything below is asserted on THAT answer, so a node list or an endpoint
        // that only appeared on a later read would fail here.
        let settled = poll_until(
            &emulator,
            REGION,
            row.id,
            "the cluster reporting itself available",
            "DescribeCacheClusters",
            &params(&[("CacheClusterId", row.id), ("ShowCacheNodeInfo", "true")]),
            |reply| reply.text("CacheCluster/CacheClusterStatus") == Some(state::STATUS_AVAILABLE),
        )
        .await;
        assert_eq!(
            settled.all("CacheNodes/CacheNode/CacheNodeId"),
            vec!["0001"],
            "case {}: the settled cluster must report its one node, got {}",
            row.id,
            settled.body
        );
        assert_eq!(
            settled.all("CacheNodes/CacheNode/CacheNodeStatus"),
            vec![state::STATUS_AVAILABLE],
            "case {}: the node of a settled cluster is provisioned, got {}",
            row.id,
            settled.body
        );
        let address = settled
            .text("CacheNodes/CacheNode/Endpoint/Address")
            .unwrap_or_default();
        let port = settled.text("CacheNodes/CacheNode/Endpoint/Port");
        assert_own_endpoint(row.id, "the node endpoint", address, port);

        let port = port.unwrap_or_default();
        let mut session = session_on(&emulator, row, address, port).await;
        for (key, value) in &row.values {
            assert_eq!(
                command(row.id, &mut session, &["SET", key, value]).await,
                Resp::Status("OK".to_string()),
                "case {}: a SET of {key} at the published endpoint {address}:{port} must be \
                 accepted",
                row.id
            );
            assert_eq!(
                command(row.id, &mut session, &["GET", key]).await,
                Resp::Bulk(value.clone()),
                "case {}: a GET of {key} must answer the {} bytes the SET stored",
                row.id,
                value.len()
            );
        }
        assert_eq!(
            command(row.id, &mut session, &["GET", NEVER_WRITTEN]).await,
            Resp::Nil,
            "case {}: a key no session wrote must be absent",
            row.id
        );
        // The values OUTLIVE the session that wrote them: a second connection to the same
        // published address reads every one of them back.
        let mut again = session_on(&emulator, row, address, port).await;
        for (key, value) in &row.values {
            assert_eq!(
                command(row.id, &mut again, &["GET", key]).await,
                Resp::Bulk(value.clone()),
                "case {}: a later session at {address}:{port} must read the {key} the first one \
                 wrote",
                row.id
            );
        }
    }
}

/// A RESP session on the endpoint `row`'s cluster published, opened from the vantage point
/// that endpoint is reachable from: a cluster PLACED in a VPC of the account publishes the
/// address its node holds in that VPC, which a client inside the VPC reaches (the register of
/// the data planes this emulator serves), and a cluster in no VPC publishes the address its
/// own socket answers at, which is dialled over a real socket.
async fn session_on(emulator: &Emulator, row: &Row, address: &str, port: &str) -> Session {
    match row.placed {
        true => open_in_vpc(emulator, row.id, address, port).await,
        false => open(row.id, address, port).await,
    }
}
