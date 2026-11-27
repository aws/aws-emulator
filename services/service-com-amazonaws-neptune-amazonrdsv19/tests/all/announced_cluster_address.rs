// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS a DB cluster and its instances announce, end to end over the awsQuery wire.
//!
//! The rule under test: `DBCluster$Endpoint` / `Endpoint$Address` is the HOST a client
//! connects to, so it is the host this emulator is actually reached at — a bare host with no
//! scheme and no port of its own — because a `*.neptune.amazonaws.com` name resolves nowhere
//! here and the Neptune data plane is addressed at the cluster's own endpoint.
//! `DBCluster$Port` / `Endpoint$Port` is "the port that the database engine is listening on",
//! which `CreateDBCluster$Port` CONFIGURES ("Default: 8182"), so it is the port the request
//! asked for — or that default — on every read and whatever address the request arrived at,
//! and a member instance reports its CLUSTER's, the port "managed by the DB cluster".
//!
//! Only an emulator served on no port at all — an in-process call with no socket behind it —
//! has no host of this process to announce; there the AWS-shaped DNS name is reported, which
//! the second test below pins.
//!
//! Each test states the rule over a TABLE of addresses: the loopback address a replay run
//! publishes is one row.

use crate::db_cluster_lifecycle::{Neptune, Reply};

/// The region the harness's SigV4 credential scope names; the AWS-shaped host uses it.
const REGION: &str = "us-east-1";

/// One row of the announced-address table.
struct AddressCase {
    name: &'static str,
    /// The `Host` header the request carries — the address the client is talking to — or
    /// `None` for a request that names no host (an in-process call).
    host: Option<&'static str>,
    /// The URL this emulator's data port is published at, or `None` for a router served on
    /// no port.
    served_base: Option<&'static str>,
    /// The `Port` the `CreateDBCluster` request asks for, when it asks for one.
    requested_port: Option<&'static str>,
    /// The host every endpoint of the cluster and of its instances must report.
    expected_host: &'static str,
    /// The port the cluster and its instances must report: the one the create asked for,
    /// else the engine default `CreateDBCluster$Port` publishes.
    expected_port: &'static str,
}

#[tokio::test]
async fn a_cluster_announces_the_address_this_emulator_is_reached_at() {
    let cases = [
        AddressCase {
            name: "the front door on the emulator's default data port",
            host: Some("127.0.0.1:9297"),
            served_base: None,
            requested_port: None,
            expected_host: "127.0.0.1",
            expected_port: "8182",
        },
        AddressCase {
            name: "an ephemeral data port, with the request asking for the engine port",
            host: Some("127.0.0.1:44903"),
            served_base: None,
            requested_port: Some("8182"),
            expected_host: "127.0.0.1",
            expected_port: "8182",
        },
        AddressCase {
            name: "loopback by name, with the request asking for another port",
            host: Some("localhost:41509"),
            served_base: None,
            requested_port: Some("9999"),
            expected_host: "localhost",
            expected_port: "9999",
        },
        AddressCase {
            name: "an IPv6 loopback literal, whose own colons live inside its brackets",
            host: Some("[::1]:38001"),
            served_base: None,
            requested_port: None,
            expected_host: "[::1]",
            expected_port: "8182",
        },
        AddressCase {
            name: "an authority that names no port, asking for the published floor",
            host: Some("127.0.0.1"),
            served_base: None,
            requested_port: Some("1150"),
            expected_host: "127.0.0.1",
            expected_port: "1150",
        },
        AddressCase {
            name: "an in-process call on an emulator that IS served on a port",
            host: None,
            served_base: Some("https://127.0.0.1:9297"),
            requested_port: Some("8182"),
            expected_host: "127.0.0.1",
            expected_port: "8182",
        },
        AddressCase {
            name: "a request that addressed an AWS host on an emulator served on a port",
            host: Some("rds.us-east-1.amazonaws.com"),
            served_base: Some("https://127.0.0.1:35117"),
            requested_port: Some("65535"),
            expected_host: "127.0.0.1",
            expected_port: "65535",
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let neptune = match case.served_base {
            Some(base) => Neptune::served_at(base),
            None => Neptune::new(),
        };
        let identifier = format!("announced-graph-{index}");
        let mut params: Vec<(&str, &str)> = vec![
            ("DBClusterIdentifier", identifier.as_str()),
            ("Engine", "neptune"),
        ];
        if let Some(port) = case.requested_port {
            params.push(("Port", port));
        }
        let created = call(&neptune, case.host, "CreateDBCluster", &params).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateDBCluster must succeed, got {} with body {}",
            case.name, created.status, created.body
        );

        for member in ["Endpoint", "ReaderEndpoint"] {
            let announced = created.field(&["CreateDBClusterResult", "DBCluster", member]);
            assert_eq!(
                announced, case.expected_host,
                "case {:?}: expected {member} {:?}, got {announced:?}",
                case.name, case.expected_host
            );
            assert!(
                !announced.contains("://"),
                "case {:?}: {member} is a host, not a URL, got {announced:?}",
                case.name
            );
        }
        let endpoint = created.field(&["CreateDBClusterResult", "DBCluster", "Endpoint"]);
        let port = created.field(&["CreateDBClusterResult", "DBCluster", "Port"]);
        assert_eq!(
            port, case.expected_port,
            "case {:?}: expected Port {:?}, got {port:?}",
            case.name, case.expected_port
        );
        // `<Endpoint>` carries no port of its own, so `<Endpoint>:<Port>` — the pair a client
        // composes — parses as a URL naming exactly one port: the announced one.
        let dialled = format!("https://{endpoint}:{port}/")
            .parse::<http::Uri>()
            .unwrap_or_else(|error| {
                panic!(
                    "case {:?}: https://{endpoint}:{port}/ must be a dialable URL: {error}",
                    case.name
                )
            });
        assert_eq!(
            dialled.port_u16().map(|found| found.to_string()),
            Some(case.expected_port.to_string()),
            "case {:?}: expected the dialed URL to name port {:?}, got {:?}",
            case.name,
            case.expected_port,
            dialled.port_u16()
        );

        // An instance of the cluster is reached at the same host, and at its CLUSTER's port
        // whatever port its own create asks for: `CreateDBInstance$Port` is "Not applicable.
        // The port is managed by the DB cluster".
        let instance = format!("{identifier}-1");
        let created_instance = call(
            &neptune,
            case.host,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", instance.as_str()),
                ("DBInstanceClass", "db.t4g.medium"),
                ("Engine", "neptune"),
                ("DBClusterIdentifier", identifier.as_str()),
                ("Port", "7777"),
            ],
        )
        .await;
        assert_eq!(
            created_instance.status, 200,
            "case {:?}: CreateDBInstance must succeed, got {} with body {}",
            case.name, created_instance.status, created_instance.body
        );
        for (member, expected) in [
            ("Address", case.expected_host),
            ("Port", case.expected_port),
        ] {
            let read = created_instance.field(&[
                "CreateDBInstanceResult",
                "DBInstance",
                "Endpoint",
                member,
            ]);
            assert_eq!(
                read, expected,
                "case {:?}: expected the instance Endpoint.{member} {expected:?}, got \
                 {read:?}",
                case.name
            );
        }

        // ROUND TRIP: the describes report the same address the creates announced.
        let described = call(
            &neptune,
            case.host,
            "DescribeDBClusters",
            &[("DBClusterIdentifier", identifier.as_str())],
        )
        .await;
        for (member, expected) in [
            ("Endpoint", case.expected_host),
            ("ReaderEndpoint", case.expected_host),
            ("Port", case.expected_port),
        ] {
            let read = described.field(&[
                "DescribeDBClustersResult",
                "DBClusters",
                "DBCluster",
                member,
            ]);
            assert_eq!(
                read, expected,
                "case {:?}: DescribeDBClusters must report {member} {expected:?}, got \
                 {read:?}",
                case.name
            );
        }
        let described_instance = call(
            &neptune,
            case.host,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", instance.as_str())],
        )
        .await;
        for (member, expected) in [
            ("Address", case.expected_host),
            ("Port", case.expected_port),
        ] {
            let read = described_instance.field(&[
                "DescribeDBInstancesResult",
                "DBInstances",
                "DBInstance",
                "Endpoint",
                member,
            ]);
            assert_eq!(
                read, expected,
                "case {:?}: DescribeDBInstances must report Endpoint.{member} {expected:?}, \
                 got {read:?}",
                case.name
            );
        }
    }
}

#[tokio::test]
async fn an_emulator_served_on_no_port_announces_the_aws_shaped_host_and_the_engine_port() {
    // Port — "Default: 8182"; a request that names one gets the one it named.
    let cases = [
        ("no Port member, so the engine default", None, "8182"),
        ("a Port member the request chose", Some("9999"), "9999"),
    ];

    for (name, requested_port, expected_port) in cases {
        let neptune = Neptune::new();
        let identifier = format!("unserved-graph-{expected_port}");
        let mut params: Vec<(&str, &str)> = vec![
            ("DBClusterIdentifier", identifier.as_str()),
            ("Engine", "neptune"),
        ];
        if let Some(port) = requested_port {
            params.push(("Port", port));
        }
        // Addressed at one of AWS's own hosts, on a router served on no port: there is no
        // address of this process to announce.
        let created = call(
            &neptune,
            Some("rds.us-east-1.amazonaws.com"),
            "CreateDBCluster",
            &params,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: CreateDBCluster must succeed, got {} with body {}",
            created.status, created.body
        );
        let endpoint = created.field(&["CreateDBClusterResult", "DBCluster", "Endpoint"]);
        let reader = created.field(&["CreateDBClusterResult", "DBCluster", "ReaderEndpoint"]);
        let port = created.field(&["CreateDBClusterResult", "DBCluster", "Port"]);
        assert!(
            endpoint.starts_with(&format!("{identifier}.cluster-"))
                && endpoint.ends_with(&format!(".{REGION}.neptune.amazonaws.com")),
            "case {name:?}: unexpected Endpoint {endpoint:?}"
        );
        assert!(
            reader.starts_with(&format!("{identifier}.cluster-ro-"))
                && reader.ends_with(&format!(".{REGION}.neptune.amazonaws.com")),
            "case {name:?}: unexpected ReaderEndpoint {reader:?}"
        );
        assert_eq!(
            port, expected_port,
            "case {name:?}: expected Port {expected_port:?}, got {port:?}"
        );
    }
}

/// Drive one awsQuery call, with or without a `Host` header.
async fn call(
    neptune: &Neptune,
    host: Option<&str>,
    action: &str,
    params: &[(&str, &str)],
) -> Reply {
    match host {
        Some(host) => neptune.call_addressed_to(host, action, params).await,
        None => neptune.call(action, params).await,
    }
}
