// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the address a DB cluster publishes when its DATA PLANE is an AWS
//! API this emulator serves at its own endpoint rather than a native engine wire.
//!
//! The rule under test: `DBCluster$Endpoint` is "the connection endpoint for the primary
//! instance of the DB cluster" — the HOST a client reaches the cluster's data plane at. An
//! Amazon Neptune cluster's data plane is its graph API — restJson1, signing name
//! `neptune-db`, with no regional endpoint of its own, addressed at the cluster's connection
//! endpoint and nowhere else — and this emulator serves that API on its OWN endpoint. So
//! such a cluster, and every member instance of it, publishes that host: a
//! `*.neptune.amazonaws.com` name resolves nowhere here, so publishing it leaves the whole
//! graph API unreachable.
//!
//! `DBCluster$Port` / `Endpoint$Port` — "the port that the database engine is listening on"
//! — is NOT that address's. It is the port the create CONFIGURED (`CreateDBCluster$Port`,
//! "the port number on which the instances in the DB cluster accept connections"), a stored
//! property of the resource: the same on every read of it and across restarts, where the port
//! this process is bound to differs between two clusters and between two boots. So a resource
//! whose plane is served as an API publishes that address through its endpoint HOST and keeps
//! its own configured port; only a resource with a LISTENER of its own publishes that
//! listener's port, which is the port its engine really answers on.
//!
//! The table spans what the rule generalises over: the address the client is talking to
//! (loopback by number and by name, with and without a port), the port the request asks
//! for (none, the engine default, another value), an engine whose data plane this emulator
//! serves as a native WIRE instead (which publishes its own listener's port, and so does the
//! one whose wire is the document protocol), an engine whose data plane it does not serve at
//! all (which keeps its AWS DNS name and its engine's port), and a request addressed to this
//! service's own AWS host on an emulator served on no port (which has no host of its own to
//! publish).

use crate::db_clusters::{describe_one, the_cluster};
use crate::db_instance_engine_endpoint::endpoint_of;
use crate::db_instances::{
    call, call_addressed_to, emulator, expect_ok, subtrees, text, wait_for_settle, AWS_ENDPOINT,
    REGION,
};
use smithy_server_runtime::MultiServiceRouter;

/// The instance class the orderable-options report offers for a member of a cluster of one
/// of the sibling control planes' engines.
const CLUSTER_MEMBER_CLASS: &str = "db.r5.large";

/// What a row expects the cluster to publish.
#[derive(Debug, PartialEq, Eq)]
enum Expected {
    /// The exact host this emulator answers the cluster's data plane on, beside the port the
    /// create configured the cluster's engine with.
    Address(&'static str, &'static str),
    /// A listener of the cluster's own: the two DNS names the cluster mints from its
    /// identifier, in the zone the interface that listener is bound on is named by, with the
    /// listener's port beside them.
    OwnListener,
    /// The DNS form of the engine's own control plane, with the port given.
    DnsName(&'static str, &'static str),
}

/// One row of the published-address table.
struct AddressCase {
    name: &'static str,
    /// The engine the cluster is created with.
    engine: &'static str,
    /// The endpoint the client is talking to.
    endpoint: &'static str,
    /// Request members beyond `DBClusterIdentifier` and `Engine`.
    extra: &'static [(&'static str, &'static str)],
    expected: Expected,
}

const ADDRESS_CASES: &[AddressCase] = &[
    AddressCase {
        name: "a Neptune cluster created through this emulator's default data port",
        engine: "neptune",
        endpoint: "https://127.0.0.1:9297/",
        extra: &[],
        // `CreateDBClusterMessage$Port` of the Neptune model: "Default: 8182" — the port a
        // create that names none configures.
        expected: Expected::Address("127.0.0.1", "8182"),
    },
    AddressCase {
        name: "a Neptune cluster whose request asked for the Neptune engine port",
        engine: "neptune",
        endpoint: "https://127.0.0.1:44903/",
        extra: &[("Port", "8182")],
        expected: Expected::Address("127.0.0.1", "8182"),
    },
    AddressCase {
        name: "a Neptune cluster reached by loopback NAME, asking for another port",
        engine: "neptune",
        endpoint: "https://localhost:41509/",
        extra: &[("Port", "9999")],
        expected: Expected::Address("localhost", "9999"),
    },
    AddressCase {
        name: "a Neptune cluster reached at an endpoint that names no port",
        engine: "neptune",
        endpoint: "https://127.0.0.1/",
        // The floor of the range an endpoint port may fall in ("Valid Values: 1150-65535").
        extra: &[("Port", "1150")],
        expected: Expected::Address("127.0.0.1", "1150"),
    },
    AddressCase {
        name: "a Neptune cluster created through this service's own AWS host",
        engine: "neptune",
        endpoint: AWS_ENDPOINT,
        extra: &[],
        // `CreateDBClusterMessage$Port` of the Neptune model: "Default: 8182".
        expected: Expected::DnsName("neptune.amazonaws.com", "8182"),
    },
    AddressCase {
        name: "a DocumentDB cluster, whose data plane is the document wire served on a \
               listener of its own",
        engine: "docdb",
        endpoint: "https://127.0.0.1:9297/",
        extra: &[],
        expected: Expected::OwnListener,
    },
    AddressCase {
        name: "an Aurora PostgreSQL cluster, whose native wire this emulator serves",
        engine: "aurora-postgresql",
        endpoint: "https://127.0.0.1:9297/",
        extra: &[],
        expected: Expected::OwnListener,
    },
];

/// Assert one cluster envelope publishes what its row expects.
fn assert_published_address(case: &str, label: &str, cluster: &str, row: &AddressCase) {
    let case = format!("{case} ({label})");
    let endpoint = text(cluster, "Endpoint");
    let reader = text(cluster, "ReaderEndpoint");
    let port = text(cluster, "Port");
    match row.expected {
        Expected::Address(host, expected_port) => {
            assert_eq!(
                endpoint, host,
                "case {case:?}: expected <Endpoint> {host:?}, got {endpoint:?} in {cluster}"
            );
            assert_eq!(
                reader, host,
                "case {case:?}: one address serves the whole graph API, so <ReaderEndpoint> \
                 must be {host:?}, got {reader:?} in {cluster}"
            );
            assert_eq!(
                port, expected_port,
                "case {case:?}: expected <Port> {expected_port:?} — the port the create \
                 configured this cluster's engine with — got {port:?} in {cluster}"
            );
            // `<Endpoint>` carries no port of its own, so `<Endpoint>:<Port>` parses as a
            // URL naming exactly the published port.
            let dialled = format!("https://{endpoint}:{port}/")
                .parse::<http::Uri>()
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: https://{endpoint}:{port}/ must be dialable: {error}")
                });
            assert_eq!(
                dialled.port_u16().map(|found| found.to_string()).as_deref(),
                Some(expected_port),
                "case {case:?}: expected the dialed URL to name port {expected_port:?}, got \
                 {:?}",
                dialled.port_u16()
            );
        }
        Expected::OwnListener => {
            // A cluster whose own engine listener serves its data plane publishes the address
            // that listener is answered at, for both of its members.
            crate::db_clusters::assert_served_endpoint_pair(&case, &endpoint, &reader);
            assert!(
                port.parse::<u16>().is_ok_and(|port| port != 0),
                "case {case:?}: expected the port the cluster's listener answers on, got \
                 {port:?} in {cluster}"
            );
        }
        Expected::DnsName(domain, expected_port) => {
            let suffix = format!(".{REGION}.{domain}");
            assert!(
                endpoint.contains(".cluster-") && endpoint.ends_with(&suffix),
                "case {case:?}: expected an <Endpoint> of the form \
                 <identifier>.cluster-<label>{suffix}, got {endpoint:?}"
            );
            assert!(
                reader.contains(".cluster-ro-") && reader.ends_with(&suffix),
                "case {case:?}: expected a <ReaderEndpoint> of the form \
                 <identifier>.cluster-ro-<label>{suffix}, got {reader:?}"
            );
            assert_eq!(
                port, expected_port,
                "case {case:?}: expected <Port> {expected_port:?}, got {port:?} in {cluster}"
            );
        }
    }
}

/// `CreateDBCluster` addressed to `endpoint`.
async fn create_cluster(
    router: &MultiServiceRouter,
    endpoint: &str,
    identifier: &str,
    row: &AddressCase,
) -> (u16, String) {
    let mut params: Vec<(&str, String)> = vec![
        ("DBClusterIdentifier", identifier.to_string()),
        ("Engine", row.engine.to_string()),
    ];
    params.extend(
        row.extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    call_addressed_to(router, endpoint, REGION, "CreateDBCluster", &params).await
}

#[tokio::test]
async fn a_cluster_publishes_the_address_its_data_plane_is_served_at() {
    let router = emulator();

    for (index, row) in ADDRESS_CASES.iter().enumerate() {
        let identifier = format!("emu-api-plane-{index}");
        let created = the_cluster(
            row.name,
            &expect_ok(
                row.name,
                create_cluster(&router, row.endpoint, &identifier, row).await,
            ),
        );
        assert_published_address(row.name, "CreateDBCluster", &created, row);

        // ROUND TRIP: the address is PERSISTED on the record, so the describe reports the
        // same one — read here through this service's own AWS host, which is a different
        // request from the one that created the cluster.
        wait_for_settle();
        let read_back = describe_one(&router, row.name, &identifier).await;
        assert_published_address(row.name, "DescribeDBClusters", &read_back, row);
    }
}

/// What a member instance must publish.
enum MemberAddress {
    /// The host this emulator answers an API data plane on, with the port its CLUSTER
    /// publishes: the plane is served at this process's host, and the port a member is
    /// reached at is the one the cluster manages.
    ApiPlane(&'static str, &'static str),
    /// A listener of the member's OWN, for an engine whose native wire this emulator serves:
    /// the address and port of the socket the core opened for it.
    OwnListener,
}

/// One row of the member table: the case, the engine, and what the member must publish.
type MemberRow = (&'static str, &'static str, MemberAddress);

#[tokio::test]
async fn a_cluster_member_publishes_the_same_address_as_its_cluster() {
    let router = emulator();
    // A member instance is a second address onto the same data plane, so the rule holds for
    // it too: the rows are the Neptune cluster whose API this emulator serves at its own
    // endpoint, and the DocumentDB cluster whose wire it serves on a listener of its own —
    // where each member is reached at an endpoint of ITS own, because such a cluster is
    // reachable only from inside the virtual private cloud it was placed in
    // (`crate::state::document_plane`).
    let rows: [MemberRow; 2] = [
        (
            "a Neptune cluster member",
            "neptune",
            // The host this emulator answers the graph API on, beside the port its CLUSTER
            // was configured with — `Port` "doesn't apply" to such a member, so the port the
            // member's own create asks for below is not its port.
            MemberAddress::ApiPlane("127.0.0.1", "8182"),
        ),
        (
            "a DocumentDB cluster member",
            "docdb",
            // The address and port of the socket the core opened for this member — never the
            // instance `Port` the create asks for below, which the core's own port range
            // takes no account of.
            MemberAddress::OwnListener,
        ),
    ];
    let endpoint = "https://127.0.0.1:9297/";

    for (index, (case, engine, expected)) in rows.iter().enumerate() {
        let cluster = format!("emu-api-member-cluster-{index}");
        let row = AddressCase {
            name: case,
            engine,
            endpoint,
            extra: &[],
            expected: Expected::OwnListener,
        };
        expect_ok(case, create_cluster(&router, endpoint, &cluster, &row).await);
        let instance = format!("emu-api-member-{index}");
        let created = expect_ok(
            case,
            call_addressed_to(
                &router,
                endpoint,
                REGION,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", instance.as_str()),
                    ("Engine", engine),
                    ("DBInstanceClass", CLUSTER_MEMBER_CLASS),
                    ("DBClusterIdentifier", cluster.as_str()),
                    // `Port` "doesn't apply" to a member whose cluster manages it, so this
                    // is not the port such a member is reached at.
                    ("Port", "7777"),
                ],
            )
            .await,
        );
        let (address, port) = endpoint_of(&created);
        match expected {
            MemberAddress::ApiPlane(host, expected_port) => {
                assert_eq!(
                    address, *host,
                    "case {case:?}: expected the instance <Address> {host:?}, got {address:?}"
                );
                assert_eq!(
                    port.to_string(),
                    *expected_port,
                    "case {case:?}: expected the instance <Port> {expected_port:?}, got {port}"
                );
            }
            MemberAddress::OwnListener => {
                assert!(
                    address.parse::<std::net::IpAddr>().is_ok(),
                    "case {case:?}: expected the <Address> its own socket is bound on, got \
                     {address:?}"
                );
                assert!(
                    !address.ends_with("amazonaws.com"),
                    "case {case:?}: a served member must publish no name in the engine's AWS \
                     DNS zone, got {address:?}"
                );
                assert!(
                    port > 0 && port.to_string() != "7777",
                    "case {case:?}: expected the port the core assigned its socket rather \
                     than the one the create asked for, got {port}"
                );
            }
        }

        // ROUND TRIP: the describe reports the address the create published.
        wait_for_settle();
        let described = expect_ok(
            case,
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", instance.as_str())],
            )
            .await,
        );
        let one = subtrees(&described, "DBInstance")
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: the describe must report the instance"));
        let (read_address, read_port) = endpoint_of(&one);
        assert_eq!(
            (read_address.as_str(), read_port),
            (address.as_str(), port),
            "case {case:?}: DescribeDBInstances must report the address the create published, \
             got {read_address:?}:{read_port}"
        );
    }
}
