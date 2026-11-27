// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the READ-ONLY engine endpoints: a DB cluster's `ReaderEndpoint` and
//! the endpoint of a DB instance created as a read replica.
//!
//! The rule under test: every address the control plane publishes for a resource whose
//! engine wire this emulator serves is an address a client can open a session on, and a
//! session on an address AWS serves read-only — a cluster's reader endpoint, which
//! "load-balances connections across the Aurora Replicas that are available in a DB
//! cluster" (those replicas being read-only instances), and a read replica's own endpoint —
//! reports itself read-only and refuses the statements that write, with the engine's own
//! code and sentence. Its reads are the writer's committed rows.
//!
//! The clients are the wire clients the sibling endpoint suites drive
//! ([`crate::db_instance_engine_endpoint`]), so what is exercised is what a stock driver
//! speaks, in ONE process.
//!
//! The rows span what the rule generalises over: both wires (PostgreSQL and MySQL), both
//! kinds of read-only endpoint (a cluster's reader endpoint and a replica instance's own),
//! both cluster types (an Aurora cluster and a Multi-AZ DB cluster), both MySQL-family
//! read-only conditions (an Aurora node reading a volume it cannot write, and an RDS for
//! MySQL replica held read-only by the server's own option), and — for the
//! statements a read-only session refuses — a write of each class the engines list (a row
//! insert, a row update, a row delete, a table create and a table drop) beside the reads
//! the same session serves.

use std::sync::Arc;

use crate::db_clusters::{describe_one, the_cluster};
use crate::db_instance_engine_endpoint::{
    available_instance, endpoint_of, wire_login as login, Refusal, Session, Wire, DATABASE,
    MASTER_PASSWORD, MASTER_USER,
};
use crate::db_instances::{
    call, emulator, emulator_with_engine, expect_ok, text, wait_for_settle, REGION,
};

// The session type and the login this suite drives both wires through live beside the wire
// clients themselves (`crate::db_instance_engine_endpoint`), so every suite whose rows span
// both wires opens its sessions the same way.

/// The statement that asks a session whether the server it is on serves writes, and the
/// answers a read-only and a read-write session give.
///
/// Each engine has its own: MySQL's `read_only` system variable ("the server permits no
/// client updates" when it is on), and PostgreSQL's `transaction_read_only`, which a session
/// on a standby is in and cannot leave.
fn read_only_probe(wire: Wire) -> (&'static str, &'static str, &'static str) {
    match wire {
        Wire::Postgres => ("SHOW transaction_read_only", "on", "off"),
        Wire::MySql => ("SELECT @@read_only", "1", "0"),
    }
}

/// The writes a read-only session refuses, one of each class the engines list, over the
/// table the writer endpoint created below.
///
/// Source: the commands PostgreSQL's `SET TRANSACTION` page names for a read-only
/// transaction (`INSERT`, `UPDATE`, `DELETE`, and the `CREATE`/`DROP` commands), which is
/// the same line MySQL's `read_only` draws.
const REFUSED_WRITES: &[&str] = &[
    "INSERT INTO ledger (id, note) VALUES (9, 'from the reader')",
    "UPDATE ledger SET note = 'rewritten' WHERE id = 1",
    "DELETE FROM ledger WHERE id = 1",
    "CREATE TABLE reader_made (id integer)",
    "DROP TABLE ledger",
];

/// What each wire's server answers a refused write with: the code a client branches on and
/// the sentence it reports.
///
/// PostgreSQL names the command it refused, under `read_only_sql_transaction` (25006),
/// whichever PostgreSQL-family engine serves the endpoint. The MySQL family has one sentence
/// per read-only CONDITION, and which condition a node is in follows its ENGINE: an Aurora
/// MySQL node that is not the cluster's writer reads a volume it cannot write and reports
/// `ER_READ_ONLY_MODE` (1836), while an RDS for MySQL or MariaDB read replica is held
/// read-only by the server's own option and reports `ER_OPTION_PREVENTS_STATEMENT` (1290).
fn expected_refusal(wire: Wire, engine: &str, statement: &str) -> Refusal {
    match wire {
        Wire::Postgres => {
            let command = match statement.split_whitespace().next().unwrap_or_default() {
                "CREATE" => "CREATE TABLE".to_string(),
                "DROP" => "DROP TABLE".to_string(),
                verb => verb.to_string(),
            };
            Refusal {
                code: "25006".to_string(),
                message: format!("cannot execute {command} in a read-only transaction"),
            }
        }
        Wire::MySql if engine.starts_with("aurora") => Refusal {
            code: "1836".to_string(),
            message: "Running in read-only mode".to_string(),
        },
        Wire::MySql => Refusal {
            code: "1290".to_string(),
            message: "The MySQL server is running with the --read-only option so it cannot \
                      execute this statement"
                .to_string(),
        },
    }
}

/// Assert the whole rule over one pair of endpoints: the one that writes and the read-only
/// one beside it, whose storage holds the writer's committed rows.
async fn the_read_only_endpoint_serves_reads_and_refuses_writes(
    case: &str,
    wire: Wire,
    engine: &str,
    writes: &mut Session,
    reads: &mut Session,
) {
    // The read-only endpoint reads what the writer committed.
    let read_back = reads.value(case, "SELECT note FROM ledger WHERE id = 1").await;
    assert_eq!(
        read_back, "committed by the writer",
        "case {case:?}: expected the read-only endpoint to read the writer's committed row, \
         got {read_back:?}"
    );

    // Each server reports itself as the kind of server it is.
    let (probe, when_read_only, when_writable) = read_only_probe(wire);
    let reported = reads.value(case, probe).await;
    assert_eq!(
        reported, when_read_only,
        "case {case:?}: expected the read-only endpoint to answer {probe:?} with \
         {when_read_only:?}, got {reported:?}"
    );
    let reported = writes.value(case, probe).await;
    assert_eq!(
        reported, when_writable,
        "case {case:?}: expected the writing endpoint to answer {probe:?} with \
         {when_writable:?}, got {reported:?}"
    );

    // Every class of write is refused on the read-only endpoint, with the engine's own code
    // and sentence.
    for statement in REFUSED_WRITES {
        let refusal = reads.query(statement).await.err().unwrap_or_else(|| {
            panic!("case {case:?}: {statement:?} must be refused on the read-only endpoint")
        });
        assert_eq!(
            refusal,
            expected_refusal(wire, engine, statement),
            "case {case:?}: expected the engine's own refusal of {statement:?}, got {refusal:?}"
        );
    }
    // Nothing of those refused statements reached the storage the read-only session reads:
    // the table and its one row are as the writer left them.
    let rows = reads.value(case, "SELECT count(*) FROM ledger").await;
    assert_eq!(
        rows, "1",
        "case {case:?}: expected the refused writes to have changed nothing, got {rows:?} \
         rows"
    );
}

/// The table and the row a writing session leaves for the read-only one to read.
async fn seed_the_writer(case: &str, writes: &mut Session) {
    writes
        .query("CREATE TABLE ledger (id integer PRIMARY KEY, note text)")
        .await
        .unwrap_or_else(|refusal| {
            panic!("case {case:?}: the writing endpoint must create a table, got {refusal:?}")
        });
    writes
        .query("INSERT INTO ledger (id, note) VALUES (1, 'committed by the writer')")
        .await
        .unwrap_or_else(|refusal| {
            panic!("case {case:?}: the writing endpoint must insert a row, got {refusal:?}")
        });
}

// ---------------------------------------------------------------------------
// A DB cluster's reader endpoint.
// ---------------------------------------------------------------------------

/// One cluster this suite provisions and then opens sessions on both endpoints of.
struct ClusterRow {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    database: &'static str,
    wire: Wire,
    /// The members this row adds beyond the required ones.
    extra: &'static [(&'static str, &'static str)],
}

/// The four settings a Multi-AZ DB cluster is required to be created with, with an `Iops`
/// inside the documented ratio of the allocated storage.
const MULTI_AZ_SETTINGS: &[(&str, &str)] = &[
    ("AllocatedStorage", "400"),
    ("DBClusterInstanceClass", "db.m5d.large"),
    ("Iops", "3000"),
    ("StorageType", "gp3"),
];

const CLUSTERS: &[ClusterRow] = &[
    ClusterRow {
        case: "an Aurora PostgreSQL cluster",
        identifier: "emu-reader-pg",
        engine: "aurora-postgresql",
        database: "appdb",
        wire: Wire::Postgres,
        extra: &[],
    },
    ClusterRow {
        case: "an Aurora MySQL cluster",
        identifier: "emu-reader-mysql",
        engine: "aurora-mysql",
        database: "appdb",
        wire: Wire::MySql,
        extra: &[],
    },
    ClusterRow {
        case: "a Multi-AZ DB cluster, the other cluster type",
        identifier: "emu-reader-multiaz",
        engine: "postgres",
        database: "appdb",
        wire: Wire::Postgres,
        extra: MULTI_AZ_SETTINGS,
    },
];

/// Create the row's cluster, wait for it to settle, and answer with the writer address it
/// publishes and the reader address beside it.
async fn available_cluster(
    router: &smithy_server_runtime::MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    row: &ClusterRow,
) -> ((String, u16), (String, u16)) {
    let mut params: Vec<(&str, String)> = vec![
        ("DBClusterIdentifier", row.identifier.to_string()),
        ("Engine", row.engine.to_string()),
        ("DatabaseName", row.database.to_string()),
        ("MasterUsername", MASTER_USER.to_string()),
        ("MasterUserPassword", MASTER_PASSWORD.to_string()),
    ];
    params.extend(
        row.extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    let created = the_cluster(
        row.case,
        &expect_ok(row.case, call(router, "CreateDBCluster", &params).await),
    );
    // A client connects once the cluster reports `available`, which the clock makes due.
    wait_for_settle();
    let settled = describe_one(router, row.case, row.identifier).await;
    let status = text(&settled, "Status");
    assert_eq!(
        status, "available",
        "case {:?}: the cluster must settle to available before a client connects, got \
         {status:?}",
        row.case
    );
    for member in ["Endpoint", "ReaderEndpoint", "Port"] {
        assert_eq!(
            text(&settled, member),
            text(&created, member),
            "case {:?}: the {member} the create published must be the one the describe \
             reports",
            row.case
        );
    }
    let writer_port: u16 = text(&settled, "Port").parse().unwrap_or_else(|_| {
        panic!(
            "case {:?}: the cluster must publish a port, got {:?}",
            row.case,
            text(&settled, "Port")
        )
    });
    // Both endpoint members are HOSTS: the model gives a DB cluster ONE `Port` for the pair,
    // so neither carries a port of its own. Each is the address this emulator answers the
    // cluster's data plane at — the host the core bound its listeners on — which is the only
    // host a client of this emulator reaches that plane at; the read-only listener's own port
    // is no member of the control plane, so a suite that opens a session on it reads that port
    // where the listener lives.
    let reader_host = text(&settled, "ReaderEndpoint");
    crate::db_clusters::assert_served_endpoint_pair(
        row.case,
        &text(&settled, "Endpoint"),
        &reader_host,
    );
    (
        (text(&settled, "Endpoint"), writer_port),
        (reader_host, reader_listener_port(engine, row)),
    )
}

/// The port the cluster's READ-ONLY listener answers on, read from this service's own
/// store.
///
/// The core assigns every listener its port and a DB cluster has ONE modelled `Port` —
/// its engine endpoint's — so the second listener's port is not a value any member of the
/// control plane carries: a suite that opens a session on that listener reads the port
/// where the listener itself lives.
fn reader_listener_port(engine: &Arc<engine::InMemoryEngine>, row: &ClusterRow) -> u16 {
    let state = service_com_amazonaws_rds_amazonrdsv19::state::service_state(&**engine)
        .expect("this service's store");
    let store = state.lock();
    let endpoint = store
        .cluster_engine_endpoint(&(REGION.to_string(), row.identifier.to_string()))
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: the cluster must have opened an engine endpoint",
                row.case
            )
        });
    let reader = endpoint.reader.as_ref().unwrap_or_else(|| {
        panic!(
            "case {:?}: the cluster must have opened its read-only listener",
            row.case
        )
    });
    u16::try_from(reader.port).unwrap_or_else(|_| {
        panic!(
            "case {:?}: expected the read-only listener's port, got {}",
            row.case, reader.port
        )
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_clusters_reader_endpoint_serves_the_writers_rows_and_refuses_every_write() {
    let (router, engine) = emulator_with_engine();
    for row in CLUSTERS {
        let (writer, reader) = available_cluster(&router, &engine, row).await;
        assert_eq!(
            reader.0, writer.0,
            "case {:?}: both endpoints in front of one served plane name the address it is \
             answered at, got {:?} and {:?}",
            row.case, writer.0, reader.0
        );
        assert_ne!(
            reader.1, writer.1,
            "case {:?}: the reader endpoint is a listener of its own, got the writer's port \
             {} for both",
            row.case, writer.1
        );
        // The cluster's writer endpoint is "the connection endpoint for the primary instance
        // of the DB cluster": the rows it commits are the rows its reader endpoint reads.
        let mut writes = login(row.case, row.wire, &writer.0, writer.1, row.database).await;
        seed_the_writer(row.case, &mut writes).await;
        let mut reads = login(row.case, row.wire, &reader.0, reader.1, row.database).await;
        the_read_only_endpoint_serves_reads_and_refuses_writes(
            row.case,
            row.wire,
            row.engine,
            &mut writes,
            &mut reads,
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_a_cluster_closes_the_reader_port_with_the_writer_one() {
    let (router, engine) = emulator_with_engine();
    let row = &CLUSTERS[0];
    let (writer, reader) = available_cluster(&router, &engine, row).await;
    // Both endpoints serve while the cluster exists.
    {
        let _reads = login(row.case, row.wire, &reader.0, reader.1, row.database).await;
        let _writes = login(row.case, row.wire, &writer.0, writer.1, row.database).await;
    }

    expect_ok(
        "delete the cluster",
        call(
            &router,
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", row.identifier),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await,
    );
    // Once the clock has taken the cluster out of the store, the listeners the record held
    // are gone with it — both of them. The removal is materialised by the next call, which
    // is the describe that reports the cluster gone.
    wait_for_settle();
    let described = call(
        &router,
        "DescribeDBClusters",
        &[("DBClusterIdentifier", row.identifier)],
    )
    .await;
    assert_eq!(
        described.0, 404,
        "case {:?}: the deleted cluster must be gone from the store, got {described:?}",
        row.case
    );
    for (endpoint, which) in [(writer, "writer"), (reader, "reader")] {
        // The ports the cluster published were the core's own listeners, which it binds on
        // loopback — the interface the names the cluster published resolve to — so that is
        // the address this dials, and nothing may answer there any more.
        let resolved: Vec<std::net::SocketAddr> =
            tokio::net::lookup_host((endpoint.0.as_str(), endpoint.1))
                .await
                .unwrap_or_else(|error| {
                    panic!(
                        "case {:?}: the {which} endpoint {:?} must resolve, got {error}",
                        row.case, endpoint.0
                    )
                })
                .collect();
        assert!(
            !resolved.is_empty() && resolved.iter().all(|address| address.ip().is_loopback()),
            "case {:?}: the {which} endpoint must resolve to this machine's loopback \
             interface, got {resolved:?}",
            row.case
        );
        assert!(
            resolved.iter().any(|address| address.is_ipv4()),
            "case {:?}: the {which} endpoint must resolve to this machine's IPv4 loopback \
             address, which is the one the core binds on, got {resolved:?}",
            row.case
        );
        let connected = tokio::net::TcpStream::connect(("127.0.0.1", endpoint.1)).await;
        assert!(
            connected.is_err(),
            "case {:?}: the {which} port a deleted cluster published must no longer accept a \
             connection",
            row.case
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cluster_whose_wire_this_emulator_does_not_serve_keeps_its_dns_reader_name() {
    let router = emulator();
    // An Amazon Neptune cluster's data plane is its graph API, served at the address THIS
    // emulator is reached at — and this request is addressed to this service's own AWS host,
    // which is no address of this process's. The cluster therefore has no local plane to lead
    // a client to and publishes a name for both endpoints rather than a port that would answer
    // nothing.
    let created = the_cluster(
        "neptune",
        &expect_ok(
            "neptune",
            call(
                &router,
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", "emu-reader-neptune".to_string()),
                    ("Engine", "neptune".to_string()),
                ],
            )
            .await,
        ),
    );
    let reader = text(&created, "ReaderEndpoint");
    assert!(
        reader.contains(".cluster-ro-") && reader.ends_with(".neptune.amazonaws.com"),
        "a cluster this emulator serves no wire for keeps its DNS-form reader endpoint, got \
         {reader:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cluster_members_endpoint_serves_the_cluster_volume_in_the_role_the_cluster_gives_it() {
    let router = emulator();
    // The rows span both wires; each cluster gets two members, which is what makes one of
    // them a read-only Aurora Replica.
    for (case, engine, wire, cluster) in [
        (
            "the members of an Aurora PostgreSQL cluster",
            "aurora-postgresql",
            Wire::Postgres,
            "emu-member-pg",
        ),
        (
            "the members of an Aurora MySQL cluster",
            "aurora-mysql",
            Wire::MySql,
            "emu-member-mysql",
        ),
    ] {
        expect_ok(
            case,
            call(
                &router,
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", cluster.to_string()),
                    ("Engine", engine.to_string()),
                    ("DatabaseName", DATABASE.to_string()),
                    ("MasterUsername", MASTER_USER.to_string()),
                    ("MasterUserPassword", MASTER_PASSWORD.to_string()),
                ],
            )
            .await,
        );
        // "For an Aurora DB cluster, you can call this operation multiple times to add more
        // than one DB instance to the cluster": the first is its primary and the second an
        // Aurora Replica.
        let mut endpoints = Vec::new();
        for member in ["primary", "replica"] {
            let identifier = format!("{cluster}-{member}");
            let created = expect_ok(
                case,
                call(
                    &router,
                    "CreateDBInstance",
                    &[
                        ("DBInstanceIdentifier", identifier.clone()),
                        ("Engine", engine.to_string()),
                        // An Aurora member's class, from the published Aurora instance
                        // classes.
                        ("DBInstanceClass", "db.r5.large".to_string()),
                        ("DBClusterIdentifier", cluster.to_string()),
                    ],
                )
                .await,
            );
            endpoints.push((identifier, endpoint_of(&created)));
        }
        // Each member joins a cluster that is still coming up, so it is ready one span after
        // that cluster is — the chain costs one span per level. This waits for the CONDITION
        // each member's own engine listener answers on, so nothing here depends on how fast
        // the host runs.
        for (identifier, _) in &endpoints {
            let request = [("DBInstanceIdentifier", identifier.clone())];
            crate::lifecycle_poll_loops::poll_until(
                case,
                "the cluster's member reaching available",
                || call(&router, "DescribeDBInstances", &request),
                |status, body| status == 200 && text(body, "DBInstanceStatus") == "available",
            )
            .await;
        }
        let [(primary, primary_endpoint), (replica, replica_endpoint)] = endpoints.as_slice()
        else {
            panic!("case {case:?}: two members were created");
        };
        assert_ne!(
            primary_endpoint.1, replica_endpoint.1,
            "case {case:?}: each member publishes a listener of its own, got the port {} for \
             both",
            primary_endpoint.1
        );

        // The member instances read ONE cluster volume, and only the member the cluster
        // writes through serves writes.
        let mut writes = login(
            case,
            wire,
            &primary_endpoint.0,
            primary_endpoint.1,
            DATABASE,
        )
        .await;
        seed_the_writer(case, &mut writes).await;
        let mut reads = login(
            case,
            wire,
            &replica_endpoint.0,
            replica_endpoint.1,
            DATABASE,
        )
        .await;
        the_read_only_endpoint_serves_reads_and_refuses_writes(case, wire, engine, &mut writes, &mut reads)
            .await;

        // A failover moves the write role, so it moves which member's endpoint serves a
        // write: "failover for a DB cluster promotes one of the Aurora Replicas to be the
        // primary DB instance (the cluster writer)".
        expect_ok(
            case,
            call(
                &router,
                "FailoverDBCluster",
                &[
                    ("DBClusterIdentifier", cluster),
                    ("TargetDBInstanceIdentifier", replica.as_str()),
                ],
            )
            .await,
        );
        let (probe, when_read_only, when_writable) = read_only_probe(wire);
        let promoted = reads.value(case, probe).await;
        assert_eq!(
            promoted, when_writable,
            "case {case:?}: expected the promoted member {replica} to serve writes, got \
             {probe:?} {promoted:?}"
        );
        reads
            .query("INSERT INTO ledger (id, note) VALUES (2, 'committed by the promoted member')")
            .await
            .unwrap_or_else(|refusal| {
                panic!("case {case:?}: the promoted member must accept a write, got {refusal:?}")
            });
        let demoted = writes.value(case, probe).await;
        assert_eq!(
            demoted, when_read_only,
            "case {case:?}: expected the demoted member {primary} to serve reads only, got \
             {probe:?} {demoted:?}"
        );
        let refusal = writes
            .query("INSERT INTO ledger (id, note) VALUES (3, 'from the demoted member')")
            .await
            .err()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the demoted member must refuse a write")
            });
        assert_eq!(
            refusal,
            expected_refusal(wire, engine, "INSERT"),
            "case {case:?}: expected the engine's own read-only refusal, got {refusal:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A read replica DB instance's own endpoint.
// ---------------------------------------------------------------------------

/// One source/replica pair this suite provisions.
struct ReplicaRow {
    case: &'static str,
    engine: &'static str,
    source: &'static str,
    replica: &'static str,
    wire: Wire,
}

const REPLICAS: &[ReplicaRow] = &[
    ReplicaRow {
        case: "a read replica of an RDS for PostgreSQL instance",
        engine: "postgres",
        source: "emu-ro-src-pg",
        replica: "emu-ro-replica-pg",
        wire: Wire::Postgres,
    },
    ReplicaRow {
        case: "a read replica of an RDS for MySQL instance",
        engine: "mysql",
        source: "emu-ro-src-mysql",
        replica: "emu-ro-replica-mysql",
        wire: Wire::MySql,
    },
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_replicas_endpoint_serves_reads_and_refuses_every_write() {
    let router = emulator();
    for row in REPLICAS {
        // A replica is built from the source's automated backups, so the source keeps them.
        let source = available_instance(
            &router,
            row.source,
            row.engine,
            &[("BackupRetentionPeriod", "1".to_string())],
        )
        .await;
        // The rows the SOURCE holds when the replica is created are the rows the replica
        // serves: "RDS begins by taking a snapshot of the source DB instance".
        let mut writes = login(
            row.case,
            row.wire,
            &source.0,
            source.1,
            crate::db_instance_engine_endpoint::DATABASE,
        )
        .await;
        seed_the_writer(row.case, &mut writes).await;
        let created = expect_ok(
            row.case,
            call(
                &router,
                "CreateDBInstanceReadReplica",
                &[
                    ("DBInstanceIdentifier", row.replica),
                    ("SourceDBInstanceIdentifier", row.source),
                ],
            )
            .await,
        );
        let replica = endpoint_of(&created);
        wait_for_settle();
        let described = expect_ok(
            row.case,
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", row.replica)],
            )
            .await,
        );
        assert_eq!(
            text(&described, "DBInstanceStatus"),
            "available",
            "case {:?}: the replica must settle to available before a client connects",
            row.case
        );
        assert_ne!(
            replica.1, source.1,
            "case {:?}: the replica is its own resource with its own listener, got the \
             source's port {} for both",
            row.case, source.1
        );
        // The replica's endpoint is the read-only one of the pair: the source's own endpoint
        // is where writes go.
        let mut reads = login(
            row.case,
            row.wire,
            &replica.0,
            replica.1,
            crate::db_instance_engine_endpoint::DATABASE,
        )
        .await;
        the_read_only_endpoint_serves_reads_and_refuses_writes(
            row.case,
            row.wire,
            row.engine,
            &mut writes,
            &mut reads,
        )
        .await;
    }
}
