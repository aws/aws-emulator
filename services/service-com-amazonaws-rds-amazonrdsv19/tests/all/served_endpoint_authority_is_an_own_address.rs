// SPDX-License-Identifier: CC0-1.0

//! WHAT a client of this emulator is handed to DIAL, for every resource of this service whose
//! data plane the emulator serves.
//!
//! The rule under test: an endpoint member a client dials names a host the responding service
//! ANSWERS on. Where this emulator serves a resource's own data-plane listener, that is the
//! listener's own address — the host the core bound it on, with the port it assigned — because a
//! client resolves and dials exactly what the control plane published and the only hosts that
//! reach this process are its own. No member names a host in a zone AWS serves, and none names
//! one this service announced beneath a host of its own: neither is an address a client of this
//! emulator can reach the resource at.
//!
//! A resource this emulator serves NO data plane for keeps the AWS DNS form, because there is
//! no local plane for it to lead a client to; those are the contrast rows at the end.
//!
//! The rows span what the rule generalises over: the RESOURCE KINDS this service publishes a
//! dialable member for (a DB cluster's `DBCluster$Endpoint`, a custom endpoint created in
//! front of one, a member DB instance of an Aurora cluster, and a standalone DB instance's
//! `Endpoint$Address`), both wires this emulator speaks (PostgreSQL and MySQL), and the
//! operations that publish such a member (the create, the describe and
//! `DescribeDBClusterEndpoints`). Every published pair is then DIALLED over a real TCP
//! connection and a row round-tripped through it, so what is asserted is an address a session
//! really opens on rather than a string of an accepted shape.

use std::net::IpAddr;
use std::sync::Arc;

use crate::db_clusters::the_cluster;
use crate::db_instance_engine_endpoint::{
    endpoint_of, wire_login as login, Wire, DATABASE, MASTER_PASSWORD, MASTER_USER,
};
use crate::db_instances::{call, emulator_with_engine, expect_ok, subtrees, text, REGION};
use crate::lifecycle_poll_loops::poll_until;
use service_com_amazonaws_rds_amazonrdsv19::state as st;

/// The instance class a member of an Aurora cluster is created with.
const AURORA_MEMBER_CLASS: &str = "db.r5.large";

/// The instance class a standalone DB instance is created with: the smallest burstable one,
/// which is enough for a session.
const STANDALONE_CLASS: &str = "db.t3.micro";

/// One address the control plane published, and where it has to lead.
struct Published {
    /// What this row is a case of.
    case: String,
    /// The member it was read out of, for the assertion messages.
    member: &'static str,
    /// The host the member published.
    host: String,
    /// The port a client composes with it: the resource's own `Port` member, or the one the
    /// member itself carries for a shape with no port member of its own.
    port: u16,
    /// The host the listener serving that resource is bound on, read from this service's own
    /// store — the address the published host has to be.
    listener_host: String,
    /// The wire a session on it speaks.
    wire: Wire,
}

/// Create a DB cluster of `engine` and answer with the envelope the create returned.
async fn create_cluster(
    router: &smithy_server_runtime::MultiServiceRouter,
    identifier: &str,
    engine: &str,
) -> String {
    the_cluster(
        identifier,
        &expect_ok(
            identifier,
            call(
                router,
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", identifier),
                    ("Engine", engine),
                    ("DatabaseName", DATABASE),
                    ("MasterUsername", MASTER_USER),
                    ("MasterUserPassword", MASTER_PASSWORD),
                ],
            )
            .await,
        ),
    )
}

/// Create a DB instance, either a member of `cluster` or a standalone one of `engine`.
async fn create_instance(
    router: &smithy_server_runtime::MultiServiceRouter,
    identifier: &str,
    engine: &str,
    cluster: Option<&str>,
) {
    let mut request: Vec<(&str, String)> = vec![
        ("DBInstanceIdentifier", identifier.to_string()),
        ("Engine", engine.to_string()),
    ];
    match cluster {
        Some(cluster) => {
            request.push(("DBClusterIdentifier", cluster.to_string()));
            request.push(("DBInstanceClass", AURORA_MEMBER_CLASS.to_string()));
        }
        None => {
            request.push(("DBInstanceClass", STANDALONE_CLASS.to_string()));
            request.push(("AllocatedStorage", "20".to_string()));
            request.push(("MasterUsername", MASTER_USER.to_string()));
            request.push(("MasterUserPassword", MASTER_PASSWORD.to_string()));
            request.push(("DBName", DATABASE.to_string()));
        }
    }
    expect_ok(identifier, call(router, "CreateDBInstance", &request).await);
}

/// Poll a cluster until it reports `available`, which is the state a client dials it in, and
/// answer with the envelope the read served.
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

/// Poll a DB instance until it reports `available`, and answer with the envelope.
async fn available_instance(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> String {
    let params = [("DBInstanceIdentifier", identifier)];
    let (body, _polls) = poll_until(
        case,
        "the DB instance",
        || call(router, "DescribeDBInstances", &params),
        |status, body| status == 200 && text(body, "DBInstanceStatus") == "available",
    )
    .await;
    body
}

/// The host the listener this emulator opened for the DB CLUSTER `identifier` is bound on,
/// read from this service's own store.
fn cluster_listener_host(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    identifier: &str,
) -> String {
    let state = st::service_state(&**engine).expect("this service's store");
    let store = state.lock();
    store
        .cluster_engine_endpoint(&(REGION.to_string(), identifier.to_string()))
        .unwrap_or_else(|| panic!("case {case:?}: the cluster must have opened an engine endpoint"))
        .endpoint
        .host
        .clone()
}

/// The host the listener this emulator opened for the DB INSTANCE `identifier` is bound on.
fn instance_listener_host(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    identifier: &str,
) -> String {
    let state = st::service_state(&**engine).expect("this service's store");
    let store = state.lock();
    store
        .engine_endpoint(&(REGION.to_string(), identifier.to_string()))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the DB instance must have opened an engine endpoint")
        })
        .endpoint
        .host
        .clone()
}

/// Whether this emulator opened an engine listener for the DB cluster `identifier`.
fn cluster_has_a_listener(engine: &Arc<engine::InMemoryEngine>, identifier: &str) -> bool {
    let state = st::service_state(&**engine).expect("this service's store");
    let store = state.lock();
    store
        .cluster_engine_endpoint(&(REGION.to_string(), identifier.to_string()))
        .is_some()
}

/// The `Port` a cluster envelope publishes beside its endpoint members.
fn cluster_port(case: &str, cluster: &str) -> u16 {
    let port = text(cluster, "Port");
    port.parse().unwrap_or_else(|_| {
        panic!("case {case:?}: expected the port the cluster's engine listens on, got {port:?}")
    })
}

/// The address of one entry of `DescribeDBClusterEndpoints`, by the `EndpointType` it carries.
async fn reported_endpoint(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    cluster: &str,
    endpoint_type: &str,
) -> String {
    let params = [("DBClusterIdentifier", cluster)];
    let (body, _polls) = poll_until(
        case,
        "the cluster's endpoint report",
        || call(router, "DescribeDBClusterEndpoints", &params),
        |status, body| {
            status == 200
                && subtrees(body, "DBClusterEndpointList")
                    .iter()
                    .any(|entry| text(entry, "EndpointType") == endpoint_type)
        },
    )
    .await;
    let entry = subtrees(&body, "DBClusterEndpointList")
        .into_iter()
        .find(|entry| text(entry, "EndpointType") == endpoint_type)
        .unwrap_or_else(|| panic!("case {case:?}: the report must carry a {endpoint_type} entry"));
    text(&entry, "Endpoint")
}

/// The host and port one `host:port` authority spells.
fn split_authority(case: &str, authority: &str) -> (String, u16) {
    let (host, port) = authority.rsplit_once(':').unwrap_or_else(|| {
        panic!("case {case:?}: expected an address of the form <host>:<port>, got {authority:?}")
    });
    let port = port.parse().unwrap_or_else(|_| {
        panic!("case {case:?}: expected the port that listener was assigned, got {port:?}")
    });
    (host.trim_matches(['[', ']']).to_string(), port)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_served_resource_publishes_the_address_its_own_listener_answers_on() {
    let (router, engine) = emulator_with_engine();

    // The estate: a cluster on each wire this emulator speaks, a member instance of one of
    // them, a custom endpoint in front of it, and a standalone instance on each wire. Every
    // create is made first, so the transitions the clock owes them run side by side.
    let pg_cluster = "emu-sea-aurora-pg";
    let my_cluster = "emu-sea-aurora-my";
    let member = "emu-sea-aurora-pg-m0";
    let custom = "emu-sea-aurora-pg-ep";
    let pg_instance = "emu-sea-postgres";
    let my_instance = "emu-sea-mysql";
    let pg_created = create_cluster(&router, pg_cluster, "aurora-postgresql").await;
    let my_created = create_cluster(&router, my_cluster, "aurora-mysql").await;
    create_instance(&router, member, "aurora-postgresql", Some(pg_cluster)).await;
    expect_ok(
        custom,
        call(
            &router,
            "CreateDBClusterEndpoint",
            &[
                ("DBClusterEndpointIdentifier", custom),
                ("DBClusterIdentifier", pg_cluster),
                // `ANY`, so a session on it reaches the member the cluster writes through:
                // what this row is about is the ADDRESS it publishes, and a read-only endpoint
                // is the subject of the suite that pins the read/write split.
                ("EndpointType", "ANY"),
            ],
        )
        .await,
    );
    create_instance(&router, pg_instance, "postgres", None).await;
    create_instance(&router, my_instance, "mysql", None).await;

    let mut rows: Vec<Published> = Vec::new();

    // A DB CLUSTER's own connection endpoint, on both wires: the member a session is opened
    // on, beside the one `Port` the model gives the pair.
    for (case, identifier, created, wire) in [
        (
            "an Aurora PostgreSQL cluster's connection endpoint",
            pg_cluster,
            &pg_created,
            Wire::Postgres,
        ),
        (
            "an Aurora MySQL cluster's connection endpoint",
            my_cluster,
            &my_created,
            Wire::MySql,
        ),
    ] {
        let settled = available_cluster(&router, case, identifier).await;
        let host = text(&settled, "Endpoint");
        let port = cluster_port(case, &settled);
        // The create published the very same pair the describe reports: one address per
        // resource, whichever call a client read it from.
        assert_eq!(
            (text(created, "Endpoint"), text(created, "Port")),
            (host.clone(), port.to_string()),
            "case {case:?}: the create and the describe must publish one address"
        );
        // And so does the endpoint report, for the cluster class it "only applies to".
        let reported = reported_endpoint(&router, case, identifier, "WRITER").await;
        assert_eq!(
            reported, host,
            "case {case:?}: the WRITER entry of the endpoint report must publish the cluster's \
             connection endpoint {host:?}, got {reported:?}"
        );
        rows.push(Published {
            case: case.to_string(),
            member: "DBCluster$Endpoint",
            host,
            port,
            listener_host: cluster_listener_host(&engine, case, identifier),
            wire,
        });
    }

    // A CUSTOM endpoint created in front of a cluster: a listener of its own, and the modelled
    // shape has no port member, so the address it publishes carries that listener's port.
    let case = "a custom endpoint created in front of a served cluster";
    let authority = reported_endpoint(&router, case, pg_cluster, "CUSTOM").await;
    let (host, port) = split_authority(case, &authority);
    rows.push(Published {
        case: case.to_string(),
        member: "DBClusterEndpoint$Endpoint",
        host,
        port,
        // Its own listener is bound on the same interface the cluster's is: the one the core
        // binds every listener on.
        listener_host: cluster_listener_host(&engine, case, pg_cluster),
        wire: Wire::Postgres,
    });

    // A DB INSTANCE's endpoint: a member of an Aurora cluster, which serves the cluster's
    // volume, and a standalone instance on each wire, which serves its own.
    for (case, identifier, wire) in [
        (
            "a member DB instance of an Aurora cluster",
            member,
            Wire::Postgres,
        ),
        (
            "a standalone PostgreSQL DB instance",
            pg_instance,
            Wire::Postgres,
        ),
        ("a standalone MySQL DB instance", my_instance, Wire::MySql),
    ] {
        let settled = available_instance(&router, case, identifier).await;
        let (host, port) = endpoint_of(&settled);
        rows.push(Published {
            case: case.to_string(),
            member: "Endpoint$Address",
            host,
            port,
            listener_host: instance_listener_host(&engine, case, identifier),
            wire,
        });
    }

    let planes = service_common::dialable_data_planes::DialablePlanes::of(&*engine)
        .expect("the engine vends the records of the planes it has opened");
    for row in &rows {
        let Published {
            case,
            member,
            host,
            port,
            listener_host,
            wire,
        } = row;
        // The published host IS the listener's own address, as the core reports it.
        let address: IpAddr = host.trim_matches(['[', ']']).parse().unwrap_or_else(|_| {
            panic!(
                "case {case:?}: <{member}> must publish the address this emulator answers the \
                 resource's data plane at, got the name {host:?}"
            )
        });
        assert!(
            address.is_loopback(),
            "case {case:?}: <{member}> must publish an address of the interface the core bound \
             its listeners on, got {address}"
        );
        assert_eq!(
            host, listener_host,
            "case {case:?}: <{member}> must publish the host that resource's own listener is \
             bound on ({listener_host}), got {host}"
        );

        // It is an address a session really opens on: a client dials the published pair over
        // TCP, is admitted, and reads back the row it wrote.
        let mut session = login(case, *wire, host, *port, DATABASE).await;
        session
            .run(
                case,
                "CREATE TABLE dialled (id integer PRIMARY KEY, note varchar(64))",
            )
            .await;
        let note = format!("dialled {member}");
        session
            .run(
                case,
                &format!("INSERT INTO dialled (id, note) VALUES (1, '{note}')"),
            )
            .await;
        let read = session.run(case, "SELECT note FROM dialled WHERE id = 1").await;
        assert_eq!(
            read.only(),
            Some(note.as_str()),
            "case {case:?}: expected the row the session on the published pair wrote, got \
             {read:?}"
        );
        session.run(case, "DROP TABLE dialled").await;

        // And an IN-PROCESS client of this emulator reaches the same pair: the register it
        // dials through answers for the address the control plane published.
        let dialed = planes.dialable(host, *port);
        let served_by = dialed.as_ref().map(|dialed| dialed.served_by.clone());
        assert_eq!(
            served_by.as_deref(),
            Some("rds"),
            "case {case:?}: the published pair {host}:{port} must resolve to a plane this \
             emulator serves, got {dialed:?}"
        );
    }

    // No two of the resources above were handed the same address: each is reached at the port
    // the core assigned its OWN listener, which is what tells one resource's plane from
    // another's.
    let mut pairs: Vec<(&str, u16)> = rows
        .iter()
        .map(|row| (row.host.as_str(), row.port))
        .collect();
    pairs.sort_unstable();
    let published = pairs.len();
    pairs.dedup();
    assert_eq!(
        pairs.len(),
        published,
        "expected one address per resource, got {pairs:?} for {published} published members"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resource_this_emulator_serves_no_plane_for_keeps_the_published_aws_name() {
    let (router, engine) = emulator_with_engine();
    // An Amazon Neptune cluster's data plane is its graph API, served at the address THIS
    // emulator is reached at, and Oracle speaks a wire this emulator runs no server for: these
    // requests are addressed to this service's own AWS host, so there is no local plane for
    // such a resource to lead a client to and it publishes no address of this emulator's at
    // all.
    let case = "a cluster whose engine wire this emulator serves nothing for";
    let cluster = "emu-sea-neptune";
    let created = the_cluster(
        case,
        &expect_ok(
            case,
            call(
                &router,
                "CreateDBCluster",
                &[("DBClusterIdentifier", cluster), ("Engine", "neptune")],
            )
            .await,
        ),
    );
    for (member, value) in [
        ("Endpoint", text(&created, "Endpoint")),
        ("ReaderEndpoint", text(&created, "ReaderEndpoint")),
    ] {
        assert!(
            value.ends_with(&format!(".{REGION}.neptune.amazonaws.com"))
                && value.parse::<IpAddr>().is_err(),
            "case {case:?}: expected the published AWS form in <{member}>, got {value:?}"
        );
    }
    assert!(
        !cluster_has_a_listener(&engine, cluster),
        "case {case:?}: this emulator opens no engine listener for such a cluster"
    );

    let case = "a DB instance whose engine wire this emulator serves nothing for";
    let identifier = "emu-sea-oracle";
    create_instance(&router, identifier, "oracle-ee", None).await;
    let settled = available_instance(&router, case, identifier).await;
    let (host, _port) = endpoint_of(&settled);
    assert!(
        host.starts_with(&format!("{identifier}."))
            && host.ends_with(&format!(".{REGION}.rds.amazonaws.com"))
            && host.parse::<IpAddr>().is_err(),
        "case {case:?}: expected the published AWS form in <Endpoint$Address>, got {host:?}"
    );
}
