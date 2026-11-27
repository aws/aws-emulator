// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DB CLUSTER engine endpoint: the `Endpoint`/`Port` a DB cluster
//! publishes, and the PostgreSQL and MySQL sessions a client opens on it.
//!
//! The rule under test: for every DB cluster whose engine speaks a wire this emulator
//! serves, the address the cluster publishes as its `Endpoint`/`Port` is a listener that
//! SERVES that engine — a client completes the engine's own handshake on it within seconds
//! of the cluster reporting `available`, authenticating as the cluster's `MasterUsername`
//! with the password RDS holds in the managed `MasterUserSecret`, into the cluster's
//! `DatabaseName`, and the session reports that same database and login. A cluster whose
//! engine speaks neither wire publishes no listener and keeps the DNS-form endpoint it
//! always had, and a cluster that is deleted takes its listener with it.
//!
//! The client is the one the sibling DB INSTANCE endpoint suite drives
//! ([`crate::db_instance_engine_endpoint`]): both wires written against the protocol
//! itself, so what is exercised here is what a stock driver speaks. Secrets Manager and KMS
//! are mounted beside the RDS service, because the password a session logs in with is the
//! one RDS generated into the managed secret and is read back through Secrets Manager's own
//! `GetSecretValue` — the sequence a client that asks RDS to manage its master password
//! follows.
//!
//! The rows span what the rule generalises over: both wires (Aurora PostgreSQL, Aurora
//! MySQL), both cluster types (an Aurora cluster and a Multi-AZ DB cluster), a cluster that
//! names its own `DatabaseName` beside one that leaves it to the engine's own default, and
//! an engine whose wire this emulator does not serve.

use serde_json::Value;

use crate::db_clusters::{describe_one, the_cluster};
use crate::db_instance_engine_endpoint::{mysql_login, postgres_login, Transport};
use crate::db_instances::{call, expect_ok, optional_text, text, wait_for_settle};
use crate::managed_master_user_password::{emulator, secrets_manager};

/// Which wire a row's cluster is dialled on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wire {
    Postgres,
    MySql,
}

/// One cluster this suite provisions and then opens a session on.
struct Row {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    master_username: &'static str,
    /// The `DatabaseName` the create names, or `None` for a cluster that names none — whose
    /// sessions land on the engine's own default database.
    database_name: Option<&'static str>,
    /// The database a session that names none is expected to land in.
    expected_database: &'static str,
    wire: Wire,
    /// The members this row adds beyond the required ones.
    extra: &'static [(&'static str, &'static str)],
}

/// The four settings a Multi-AZ DB cluster is required to be created with, each documented
/// "This setting is required to create a Multi-AZ DB cluster", with an `Iops` inside the
/// documented ratio of the allocated storage.
///
/// `pub(crate)` so the sibling suites of this one test target create the other cluster type
/// the same way rather than each writing the four settings again.
pub(crate) const MULTI_AZ_SETTINGS: &[(&str, &str)] = &[
    ("AllocatedStorage", "400"),
    ("DBClusterInstanceClass", "db.m5d.large"),
    ("Iops", "3000"),
    ("StorageType", "gp3"),
];

const ROWS: &[Row] = &[
    Row {
        case: "an Aurora PostgreSQL Serverless v2 cluster with a database of its own",
        identifier: "emu-cluster-wire-pg",
        engine: "aurora-postgresql",
        master_username: "pgadmin",
        database_name: Some("appdb"),
        expected_database: "appdb",
        wire: Wire::Postgres,
        extra: &[
            ("ServerlessV2ScalingConfiguration.MinCapacity", "0.5"),
            ("ServerlessV2ScalingConfiguration.MaxCapacity", "4.0"),
        ],
    },
    Row {
        case: "an Aurora MySQL cluster with a database of its own",
        identifier: "emu-cluster-wire-mysql",
        engine: "aurora-mysql",
        master_username: "myadmin",
        database_name: Some("shopdb"),
        expected_database: "shopdb",
        wire: Wire::MySql,
        extra: &[],
    },
    Row {
        case: "an Aurora PostgreSQL cluster that names no database",
        identifier: "emu-cluster-wire-default",
        engine: "aurora-postgresql",
        master_username: "pgadmin2",
        database_name: None,
        // The database RDS creates with every PostgreSQL engine, which a session that names
        // none connects to.
        expected_database: "postgres",
        wire: Wire::Postgres,
        extra: &[],
    },
    Row {
        case: "a Multi-AZ DB cluster, the other cluster type",
        identifier: "emu-cluster-wire-multiaz",
        engine: "postgres",
        master_username: "pgadmin3",
        database_name: Some("ledger"),
        expected_database: "ledger",
        wire: Wire::Postgres,
        extra: MULTI_AZ_SETTINGS,
    },
];

/// Create the row's cluster the way a client that lets RDS manage the master password
/// does, wait for it to settle, and answer with the address it publishes and the password
/// the managed secret holds.
async fn available_cluster(
    router: &smithy_server_runtime::MultiServiceRouter,
    secrets: &aws_sdk_secretsmanager::Client,
    row: &Row,
) -> (String, u16, String) {
    let mut params: Vec<(&str, String)> = vec![
        ("DBClusterIdentifier", row.identifier.to_string()),
        ("Engine", row.engine.to_string()),
        ("MasterUsername", row.master_username.to_string()),
        ("ManageMasterUserPassword", "true".to_string()),
    ];
    // The HTTP endpoint the RDS Data API is served over is "valid for Aurora DB clusters
    // only", so only an Aurora row turns it on — the Data API surface a client uses beside
    // the engine's own wire.
    if row.engine.starts_with("aurora") {
        params.push(("EnableHttpEndpoint", "true".to_string()));
    }
    if let Some(database) = row.database_name {
        params.push(("DatabaseName", database.to_string()));
    }
    params.extend(
        row.extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    let created = the_cluster(
        row.case,
        &expect_ok(row.case, call(router, "CreateDBCluster", &params).await),
    );
    let published = (text(&created, "Endpoint"), text(&created, "Port"));

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
    assert_eq!(
        (text(&settled, "Endpoint"), text(&settled, "Port")),
        published,
        "case {:?}: the endpoint the create published must be the one the describe reports",
        row.case
    );
    let (address, port) = published;
    let port: u16 = port
        .parse()
        .unwrap_or_else(|_| panic!("case {:?}: expected a port, got {port:?}", row.case));

    // The password a session logs in with is the one RDS generated into the secret it
    // manages for this cluster, read through Secrets Manager's own operation.
    let secret_arn = text(&settled, "SecretArn");
    let value = secrets
        .get_secret_value()
        .secret_id(&secret_arn)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!(
                "case {:?}: the managed secret {secret_arn} must be readable, got {error}",
                row.case
            )
        });
    let document: Value = serde_json::from_str(value.secret_string().unwrap_or_default())
        .unwrap_or_else(|error| {
            panic!(
                "case {:?}: the managed secret holds a credentials document, got {error}",
                row.case
            )
        });
    let password = document["password"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: the managed secret holds a password, got {document}",
                row.case
            )
        })
        .to_string();
    assert_eq!(
        document["username"].as_str(),
        Some(row.master_username),
        "case {:?}: the managed secret holds the cluster's master user, got {document}",
        row.case
    );
    (address, port, password)
}

/// What a session reports about itself: the database it is in and the login it holds.
async fn session_identity(row: &Row, address: &str, port: u16, password: &str) -> (String, String) {
    match row.wire {
        Wire::Postgres => {
            let mut session = postgres_login(
                address,
                port,
                Transport::Tls,
                row.master_username,
                password,
                row.database_name,
            )
            .await
            .unwrap_or_else(|refusal| {
                panic!(
                    "case {:?}: the master user must be admitted on the cluster's endpoint, \
                     got {refusal:?}",
                    row.case
                )
            });
            let answer = session
                .query("SELECT current_database(), current_user")
                .await
                .unwrap_or_else(|refusal| {
                    panic!(
                        "case {:?}: the identity query must run, got {refusal:?}",
                        row.case
                    )
                });
            let row_values = answer.rows.first().cloned().unwrap_or_default();
            (
                row_values.first().cloned().flatten().unwrap_or_default(),
                row_values.get(1).cloned().flatten().unwrap_or_default(),
            )
        }
        Wire::MySql => {
            let mut session = mysql_login(
                address,
                port,
                Transport::Plaintext,
                row.master_username,
                password,
                row.database_name,
            )
            .await
            .unwrap_or_else(|refusal| {
                panic!(
                    "case {:?}: the master user must be admitted on the cluster's endpoint, \
                     got {refusal:?}",
                    row.case
                )
            });
            let answer = session
                .query("SELECT DATABASE(), CURRENT_USER()")
                .await
                .unwrap_or_else(|refusal| {
                    panic!(
                        "case {:?}: the identity query must run, got {refusal:?}",
                        row.case
                    )
                });
            let row_values = answer.rows.first().cloned().unwrap_or_default();
            (
                row_values.first().cloned().flatten().unwrap_or_default(),
                row_values.get(1).cloned().flatten().unwrap_or_default(),
            )
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_clusters_published_endpoint_serves_its_engines_wire() {
    let router = emulator();
    let secrets = secrets_manager(&router);
    for row in ROWS {
        let (address, port, password) = available_cluster(&router, &secrets, row).await;
        let (database, user) = session_identity(row, &address, port, &password).await;
        assert_eq!(
            database, row.expected_database,
            "case {:?}: expected the session to be in the cluster's database {:?}, got \
             {database:?}",
            row.case, row.expected_database
        );
        // The MySQL master user holds the account RDS grants it, `'<master>'@'%'`, which is
        // what that engine reports for the login; PostgreSQL reports the role name.
        let expected_user = match row.wire {
            Wire::Postgres => row.master_username.to_string(),
            Wire::MySql => format!("{}@%", row.master_username),
        };
        assert_eq!(
            user, expected_user,
            "case {:?}: expected the session to hold the cluster's master user \
             {expected_user:?}, got {user:?}",
            row.case
        );

        // The credential is the cluster's own: another password for the same user is not
        // admitted on the same endpoint.
        let refused = match row.wire {
            Wire::Postgres => postgres_login(
                &address,
                port,
                Transport::Tls,
                row.master_username,
                "not-the-managed-password",
                row.database_name,
            )
            .await
            .err(),
            Wire::MySql => mysql_login(
                &address,
                port,
                Transport::Plaintext,
                row.master_username,
                "not-the-managed-password",
                row.database_name,
            )
            .await
            .err(),
        };
        assert!(
            refused.is_some(),
            "case {:?}: a login with another password must be refused",
            row.case
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cluster_whose_wire_this_emulator_does_not_serve_keeps_its_dns_endpoint() {
    let router = emulator();
    // An Amazon Neptune cluster's data plane is its graph API, served at the address THIS
    // emulator is reached at — and this request is addressed to this service's own AWS host,
    // which is no address of this process's: the cluster publishes a name rather than a port
    // that would answer nothing.
    let created = the_cluster(
        "neptune",
        &expect_ok(
            "neptune",
            call(
                &router,
                "CreateDBCluster",
                &[
                    (
                        "DBClusterIdentifier",
                        "emu-cluster-wire-neptune".to_string(),
                    ),
                    ("Engine", "neptune".to_string()),
                ],
            )
            .await,
        ),
    );
    let endpoint = text(&created, "Endpoint");
    assert!(
        endpoint.ends_with(".neptune.amazonaws.com"),
        "a cluster this emulator serves no wire for keeps its DNS-form endpoint, got \
         {endpoint:?}"
    );
    let port = text(&created, "Port");
    assert_eq!(
        port, "8182",
        "such a cluster publishes the port its engine's control plane documents, got \
         {port:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_a_cluster_closes_the_port_it_published() {
    let router = emulator();
    let secrets = secrets_manager(&router);
    let row = &ROWS[0];
    let (address, port, password) = available_cluster(&router, &secrets, row).await;
    // The endpoint serves while the cluster exists.
    let (database, _) = session_identity(row, &address, port, &password).await;
    assert_eq!(
        database, row.expected_database,
        "the endpoint must serve the cluster's database before the delete, got {database:?}"
    );

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
    // Once the clock has taken the cluster out of the store the listener is gone with it:
    // the record held it, so there is nothing left to accept a connection.
    wait_for_settle();
    let described = call(
        &router,
        "DescribeDBClusters",
        &[("DBClusterIdentifier", row.identifier)],
    )
    .await;
    assert_eq!(
        described.0, 404,
        "the deleted cluster must be gone from the store, got {described:?}"
    );
    // The port the cluster published was the core's own listener, bound on loopback — the
    // interface the name the cluster published resolves to — which is where this dials.
    let resolved: Vec<std::net::SocketAddr> = tokio::net::lookup_host((address.as_str(), port))
        .await
        .unwrap_or_else(|error| {
            panic!("the published endpoint {address:?} must resolve, got {error}")
        })
        .collect();
    assert!(
        !resolved.is_empty() && resolved.iter().all(|address| address.ip().is_loopback()),
        "the published endpoint must resolve to this machine's loopback interface, got \
         {resolved:?} for {address:?}"
    );
    assert!(
        resolved.iter().any(|address| address.is_ipv4()),
        "the published endpoint must resolve to this machine's IPv4 loopback address, which \
         is the one the core binds on, got {resolved:?}"
    );
    // The listener the record held is dropped with the record, and the socket it was
    // accepting on closes as that drop is worked through — so what is waited for is the
    // CONDITION (the port refusing a connection) rather than a span of time, with a deadline
    // far longer than any teardown this emulator performs.
    let closed = {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
                Err(_) => break true,
                Ok(_) if std::time::Instant::now() >= deadline => break false,
                Ok(_) => {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
            }
        }
    };
    assert!(
        closed,
        "the port a deleted cluster published must no longer accept a connection"
    );
    // Nothing else reports the cluster either: the endpoint is the resource's, and it left
    // with the resource.
    assert_eq!(
        optional_text(&described.1, "Endpoint"),
        None,
        "a 404 carries no endpoint, got {described:?}"
    );
}
