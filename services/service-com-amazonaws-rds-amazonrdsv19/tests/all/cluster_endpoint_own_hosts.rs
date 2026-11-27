// SPDX-License-Identifier: CC0-1.0

//! WHICH HOSTS a DB cluster envelope publishes, on every operation that answers with one.
//!
//! The rule under test: an endpoint member a client is handed is an address it RESOLVES and
//! then DIALS, so every such member a cluster whose data plane this emulator SERVES publishes
//! — `DBCluster$Endpoint` ("the connection endpoint for the primary instance of the DB
//! cluster") and `DBCluster$ReaderEndpoint` (the member that "load-balances connections across
//! the Aurora Replicas") — is a host this emulator ANSWERS on: the address the core reports the
//! cluster's own listener at, the only host of this process's a client of the emulator reaches
//! that plane at. Neither member sits in a zone AWS serves, which would resolve nowhere near
//! this process. A cluster this emulator serves no data plane for keeps the published AWS DNS
//! form for both members, the pair of names the CLUSTER mints from its own identifier under the
//! `cluster-` and `cluster-ro-` labels, beneath the endpoint domain of the control plane that
//! owns its engine: there is no local plane for such a cluster to lead a client to.
//!
//! The rows span what the rule generalises over: both engine wires this emulator speaks
//! (PostgreSQL and MySQL), both cluster types (an Aurora cluster, including a Serverless v2
//! one with no member instance at all, and a Multi-AZ DB cluster), an engine whose data plane
//! is a wire of its own this suite dials no session on (Amazon DocumentDB's document wire,
//! driven end to end by `tests/all/docdb_document_plane.rs`), and an engine this emulator
//! serves no data plane for at the address these requests reach it on, which is the contrast
//! row. Each row is read through EVERY operation that answers with a cluster envelope —
//! `CreateDBCluster`, `DescribeDBClusters`, `ModifyDBCluster` and `DeleteDBCluster`, the three
//! this issue's replay binds plus the mutation between them — and each published pair is then
//! RESOLVED, looked up in the register an in-process client of this emulator dials through,
//! and (for the writer, the member a client composes with the one published `Port`) DIALLED
//! over a real TCP connection carrying a round-tripped row, so a member naming an address this
//! emulator does not answer on could not pass.

use std::net::SocketAddr;

use crate::db_cluster_engine_endpoint::MULTI_AZ_SETTINGS;
use crate::db_clusters::{assert_endpoint_name_pair, assert_served_endpoint_pair, the_cluster};
use crate::db_instance_engine_endpoint::{
    wire_login as login, Wire, DATABASE, MASTER_PASSWORD, MASTER_USER,
};
use crate::db_instances::{call, emulator_with_engine, expect_ok, text, REGION};
use crate::lifecycle_poll_loops::poll_until;

/// One cluster of the table.
struct ClusterRow {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    /// The wire a client opens a session on, for a row whose engine speaks one this suite's
    /// session harness drives — `None` for a row driven by the suite of its own wire.
    wire: Option<Wire>,
    /// The DNS domain this row's endpoint NAMES sit in, for a cluster that publishes names at
    /// all: the endpoint domain of the control plane that owns the engine. `None` for a cluster
    /// whose data plane this emulator serves, which publishes the address it answers it at.
    unserved_domain: Option<&'static str>,
    /// The parameters the create adds beyond the identifier, engine, database and master user.
    extra: &'static [(&'static str, &'static str)],
}

const CLUSTERS: &[ClusterRow] = &[
    ClusterRow {
        case: "an Aurora PostgreSQL Serverless v2 cluster with no member instance at all",
        identifier: "emu-ceoh-aurora-pg",
        engine: "aurora-postgresql",
        wire: Some(Wire::Postgres),
        unserved_domain: None,
        extra: &[
            ("ServerlessV2ScalingConfiguration.MinCapacity", "0.5"),
            ("ServerlessV2ScalingConfiguration.MaxCapacity", "1.0"),
        ],
    },
    ClusterRow {
        case: "an Aurora MySQL cluster, the other wire",
        identifier: "emu-ceoh-aurora-mysql",
        engine: "aurora-mysql",
        wire: Some(Wire::MySql),
        unserved_domain: None,
        extra: &[],
    },
    ClusterRow {
        case: "a Multi-AZ DB cluster, the other cluster type",
        identifier: "emu-ceoh-multiaz-pg",
        engine: "postgres",
        wire: Some(Wire::Postgres),
        unserved_domain: None,
        extra: MULTI_AZ_SETTINGS,
    },
    ClusterRow {
        case: "a cluster whose data plane is the document wire",
        identifier: "emu-ceoh-docdb",
        engine: "docdb",
        // The document wire is not one this suite's session harness speaks, so this row is
        // read rather than dialled; `tests/all/docdb_document_plane.rs` drives a session on it.
        wire: None,
        unserved_domain: None,
        extra: &[],
    },
    ClusterRow {
        case: "a cluster of an engine this emulator serves no data plane for",
        identifier: "emu-ceoh-neptune",
        engine: "neptune",
        // An Amazon Neptune cluster's data plane is its graph API, served at the address THIS
        // emulator is reached at — and these requests are addressed to this service's own AWS
        // host, which is no address of this process's, so the cluster has no local plane to
        // lead a client to and keeps the names it minted
        // (`tests/all/api_plane_cluster_endpoint.rs` drives the served side of that rule).
        wire: None,
        // The endpoint domain of the control plane that owns the engine, which is where the
        // names such a cluster publishes sit.
        unserved_domain: Some("neptune.amazonaws.com"),
        extra: &[],
    },
];

/// The two endpoint members and the port one cluster envelope publishes.
fn published(case: &str, which: &str, cluster: &str) -> (String, String, u16) {
    let writer = text(cluster, "Endpoint");
    let reader = text(cluster, "ReaderEndpoint");
    let port: u16 = text(cluster, "Port").parse().unwrap_or_else(|_| {
        panic!(
            "case {case:?} ({which}): expected the port the cluster's engine is reached on, got \
             {:?}",
            text(cluster, "Port")
        )
    });
    assert!(
        port != 0,
        "case {case:?} ({which}): expected a port a client can dial, got {port}"
    );
    (writer, reader, port)
}

/// Assert the published pair of one envelope against the rule, whichever operation answered
/// with it.
fn assert_published_hosts(row: &ClusterRow, which: &str, cluster: &str) -> (String, String, u16) {
    let case = format!("{} ({which})", row.case);
    let (writer, reader, port) = published(&case, which, cluster);
    match row.unserved_domain {
        // A cluster with no plane of this emulator's keeps the published AWS form for both
        // members: there is no local plane to lead a client to.
        Some(domain) => assert_endpoint_name_pair(
            &case,
            row.identifier,
            REGION,
            domain,
            &writer,
            &reader,
        ),
        // A served cluster publishes, for both members, the address this emulator answers its
        // data plane at — and no name in a zone of AWS's own.
        None => assert_served_endpoint_pair(&case, &writer, &reader),
    }
    (writer, reader, port)
}

/// Create the row's cluster, answering the envelope the create published.
async fn create_cluster(
    router: &smithy_server_runtime::MultiServiceRouter,
    row: &ClusterRow,
) -> String {
    let mut request: Vec<(&str, String)> = vec![
        ("DBClusterIdentifier", row.identifier.to_string()),
        ("Engine", row.engine.to_string()),
    ];
    // The master user credential, for an engine whose own API takes one: Amazon Neptune
    // documents `MasterUserPassword` as "Not supported by Neptune", so the row of that engine
    // names none.
    if row.engine != "neptune" {
        request.extend([
            ("DatabaseName", DATABASE.to_string()),
            ("MasterUsername", MASTER_USER.to_string()),
            ("MasterUserPassword", MASTER_PASSWORD.to_string()),
        ]);
    }
    request.extend(
        row.extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    the_cluster(
        row.case,
        &expect_ok(row.case, call(router, "CreateDBCluster", &request).await),
    )
}

/// Poll the cluster until it reports `available`, which is the state a client dials it in.
///
/// The CONDITION is waited for rather than a span: the loop is the one this test target
/// shares, whose deadline is generous beside the span a transition this service owes becomes
/// due after.
async fn available_cluster(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> String {
    let params = [("DBClusterIdentifier", identifier)];
    let (body, _polls) = poll_until(
        case,
        "the cluster",
        || call(router, "DescribeDBClusters", &params),
        |status, body| status == 200 && text(body, "Status") == "available",
    )
    .await;
    the_cluster(case, &body)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_cluster_envelope_publishes_endpoint_hosts_this_emulator_answers_on() {
    let (router, engine) = emulator_with_engine();
    for row in CLUSTERS {
        let created = create_cluster(&router, row).await;
        let created_pair = assert_published_hosts(row, "CreateDBCluster", &created);

        let settled = available_cluster(&router, row.case, row.identifier).await;
        let read_pair = assert_published_hosts(row, "DescribeDBClusters", &settled);
        assert_eq!(
            created_pair, read_pair,
            "case {:?}: the read path must report the pair the create published",
            row.case
        );

        // A MUTATION answers with the same envelope, and the pair is the cluster's own rather
        // than a value recomputed per response: a modification that touches none of it leaves
        // it as it stood.
        let modified = the_cluster(
            row.case,
            &expect_ok(
                row.case,
                call(
                    &router,
                    "ModifyDBCluster",
                    &[
                        ("DBClusterIdentifier", row.identifier),
                        ("BackupRetentionPeriod", "7"),
                        ("ApplyImmediately", "true"),
                    ],
                )
                .await,
            ),
        );
        let modified_pair = assert_published_hosts(row, "ModifyDBCluster", &modified);
        assert_eq!(
            modified_pair, read_pair,
            "case {:?}: a modification that touches no endpoint must leave the published pair \
             as it stood",
            row.case
        );

        // Both published members RESOLVE, and to the interface this emulator's listeners are
        // bound on — which is what makes the pair an address a client can open.
        if row.wire.is_some() {
            for (which, host) in [("writer", &read_pair.0), ("reader", &read_pair.1)] {
                let resolved: Vec<SocketAddr> =
                    tokio::net::lookup_host((host.as_str(), read_pair.2))
                        .await
                        .unwrap_or_else(|error| {
                            panic!(
                                "case {:?}: the {which} endpoint {host:?} must resolve, got \
                                 {error}",
                                row.case
                            )
                        })
                        .collect();
                assert!(
                    !resolved.is_empty()
                        && resolved.iter().all(|address| address.ip().is_loopback()),
                    "case {:?}: the {which} endpoint {host:?} must resolve to the interface \
                     this emulator's listeners are bound on, got {resolved:?}",
                    row.case
                );
            }

            // And an in-process client of this emulator, which dials through the register of
            // the planes this process has opened, reaches a plane at each published pair.
            let planes = service_common::dialable_data_planes::DialablePlanes::of(&*engine)
                .expect("the engine vends the records of the planes it has opened");
            for (which, host) in [("writer", &read_pair.0), ("reader", &read_pair.1)] {
                let dialed = planes.dialable(host, read_pair.2);
                assert!(
                    dialed.is_some(),
                    "case {:?}: the {which} pair {host}:{} must resolve to a plane this \
                     emulator serves, got {dialed:?}",
                    row.case,
                    read_pair.2
                );
            }
        }

        // The WRITER pair is the one a client composes from the endpoint and the single
        // `Port`, so it is dialled for real: the session is admitted and reads back the row it
        // wrote.
        if let Some(wire) = row.wire {
            let mut session = login(
                row.case,
                wire,
                &read_pair.0,
                read_pair.2,
                DATABASE,
            )
            .await;
            session
                .run(
                    row.case,
                    "CREATE TABLE published_host (id integer PRIMARY KEY, note varchar(64))",
                )
                .await;
            session
                .run(
                    row.case,
                    "INSERT INTO published_host (id, note) VALUES (1, 'dialled a published host')",
                )
                .await;
            let read = session
                .run(row.case, "SELECT note FROM published_host WHERE id = 1")
                .await;
            assert_eq!(
                read.only(),
                Some("dialled a published host"),
                "case {:?}: expected the row the session wrote, got {read:?}",
                row.case
            );
        }

        // The DELETE answers with the cluster as it stood, so it publishes the same pair: a
        // client that diffs the delete envelope against the cluster it created sees the
        // connection coordinates it was given.
        let deleted = the_cluster(
            row.case,
            &expect_ok(
                row.case,
                call(
                    &router,
                    "DeleteDBCluster",
                    &[
                        ("DBClusterIdentifier", row.identifier),
                        ("SkipFinalSnapshot", "true"),
                    ],
                )
                .await,
            ),
        );
        let deleted_pair = assert_published_hosts(row, "DeleteDBCluster", &deleted);
        assert_eq!(
            deleted_pair, read_pair,
            "case {:?}: the delete must publish the pair the cluster was created with",
            row.case
        );
    }
}
