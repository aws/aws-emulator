// SPDX-License-Identifier: CC0-1.0

//! The two SURFACES of one DB cluster, over the real wire of each.
//!
//! RULE: a DB cluster's RDS Data API HTTP endpoint and its engine endpoint are two doors
//! onto ONE database per (cluster, database name) — the Data API is a signed HTTP front door
//! onto the same cluster, not a store of its own. So a committed write made through either
//! door is read by the other, in either direction; and what an open TRANSACTION of one door
//! has written is invisible to the other door until that transaction commits, which is what
//! makes the two doors one database rather than one shared buffer.
//!
//! The rows span what the rule generalises over: both engine families the Data API serves
//! and both wires their endpoints speak (Aurora PostgreSQL, Aurora MySQL), both directions
//! of the crossing (a table and rows created through the Data API and read natively, and a
//! row inserted natively and read through the Data API), and both the committed and the
//! uncommitted state of a Data API transaction.
//!
//! Every surface is driven through its own front door: the control plane and the Data API as
//! restJson1/awsQuery calls through the router, and the engine endpoint as a client speaking
//! the engine's own protocol on the published address. The service that answers the Data API
//! is MOUNTED beside Amazon RDS, Secrets Manager and KMS on one shared engine, exactly as
//! the assembled router mounts them — the Data API resolves the cluster through the RDS
//! control plane and its credentials through Secrets Manager, so both have to be the same
//! emulator's.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_clusters::{describe_one, the_cluster};
use crate::db_instance_engine_endpoint::{mysql_login, postgres_login, Rows, Transport};
use crate::db_instances::{call, expect_ok, optional_text, text, REGION};
use crate::lifecycle_poll_loops::poll_until;

/// Which wire a row's cluster publishes, and so which client this suite dials it with.
///
/// `pub(crate)` here and below so the sibling suite whose subject is the ISOLATION the two
/// doors read one another's writes under
/// ([`crate::engine_statement_snapshots_and_lock_conditions`]) provisions its cluster with
/// this one's harness rather than standing up a second one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wire {
    Postgres,
    MySql,
}

/// One cluster this suite provisions and then writes to through both of its doors.
pub(crate) struct Row {
    pub(crate) case: &'static str,
    pub(crate) identifier: &'static str,
    pub(crate) engine: &'static str,
    pub(crate) master_username: &'static str,
    /// The `DatabaseName` the create names, which is the database BOTH doors address.
    pub(crate) database: &'static str,
    pub(crate) wire: Wire,
    /// A statement over the SERVER's own catalogs of that database, asked after both doors
    /// have written to it: the catalogs of a database are served inside it, so a database
    /// both doors write must leave them readable.
    pub(crate) catalog_query: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "an Aurora PostgreSQL cluster",
        identifier: "emu-surfaces-pg",
        engine: "aurora-postgresql",
        master_username: "pgadmin",
        database: "appdb",
        wire: Wire::Postgres,
        catalog_query: "SELECT count(*) FROM pg_class",
    },
    Row {
        case: "an Aurora MySQL cluster",
        identifier: "emu-surfaces-mysql",
        engine: "aurora-mysql",
        master_username: "myadmin",
        database: "shopdb",
        wire: Wire::MySql,
        catalog_query: "SELECT count(*) FROM information_schema.SCHEMATA",
    },
];

/// A router carrying Amazon RDS, the service that answers the RDS Data API, Secrets Manager
/// and KMS over ONE shared engine.
///
/// The same mounting as [`crate::managed_master_user_password::emulator`] — which the
/// managed master password needs — plus the Data API's own service, because the crossing
/// under test needs both of a cluster's doors on one engine.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Verifying signatures is the emulator's concern, not this service's; these tests sign
    // nothing, so the check is off and the scope header is read for routing only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_rds_amazonrdsv19::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_rdsdata_rdsdataservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// What one provisioned cluster offers its two doors: where its engine endpoint answers, the
/// password its master user logs in with, and the ARNs the Data API addresses it by.
pub(crate) struct Cluster {
    pub(crate) address: String,
    pub(crate) port: u16,
    pub(crate) password: String,
    pub(crate) arn: String,
    pub(crate) secret_arn: String,
}

/// Create the row's cluster with its Data API door turned on and RDS managing its master
/// password, wait for it to report `available`, and read back what both doors need.
pub(crate) async fn available_cluster(router: &MultiServiceRouter, row: &Row) -> Cluster {
    let created = the_cluster(
        row.case,
        &expect_ok(
            row.case,
            call(
                router,
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", row.identifier.to_string()),
                    ("Engine", row.engine.to_string()),
                    ("MasterUsername", row.master_username.to_string()),
                    ("ManageMasterUserPassword", "true".to_string()),
                    // "The HTTP endpoint for the Aurora DB cluster", which is the door the
                    // Data API is served over.
                    ("EnableHttpEndpoint", "true".to_string()),
                    ("DatabaseName", row.database.to_string()),
                ],
            )
            .await,
        ),
    );
    let published = (text(&created, "Endpoint"), text(&created, "Port"));

    // A client connects once the cluster reports `available`, which the clock makes due: the
    // loop waits for THAT condition rather than for a span of time.
    let lookup = [("DBClusterIdentifier", row.identifier)];
    let _settling = poll_until(
        row.case,
        &format!("{} reaching available", row.identifier),
        || call(router, "DescribeDBClusters", &lookup),
        |status, body| {
            status == 200
                && optional_text(&the_cluster(row.case, body), "Status").as_deref()
                    == Some("available")
        },
    )
    .await;
    let settled = describe_one(router, row.case, row.identifier).await;
    let secret_arn = text(&settled, "SecretArn");
    let arn = text(&settled, "DBClusterArn");
    let (address, port) = published;
    let port: u16 = port
        .parse()
        .unwrap_or_else(|_| panic!("case {:?}: expected a port, got {port:?}", row.case));

    // The password either door authenticates with is the one RDS generated into the secret
    // it manages for this cluster — the Data API reads it out of that same secret.
    let credentials = crate::managed_master_user_password::held_credentials(
        row.case,
        router,
        &secret_arn,
    )
    .await;
    assert_eq!(
        credentials.username.as_deref(),
        Some(row.master_username),
        "case {:?}: the managed secret holds the cluster's master user",
        row.case
    );
    Cluster {
        address,
        port,
        password: credentials.password,
        arn,
        secret_arn,
    }
}

/// One Data API call through the router, the way an SDK client sends it.
pub(crate) async fn data_api(router: &MultiServiceRouter, path: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://rds-data.{REGION}.amazonaws.com{path}"))
        .header("host", format!("rds-data.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// `ExecuteStatement` on the cluster's Data API door, which must succeed.
pub(crate) async fn execute(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &Cluster,
    database: &str,
    sql: &str,
    transaction: Option<&str>,
) -> Value {
    let mut members = json!({
        "resourceArn": cluster.arn,
        "secretArn": cluster.secret_arn,
        "database": database,
        "sql": sql,
    });
    if let Some(transaction) = transaction {
        members["transactionId"] = json!(transaction);
    }
    let (status, body) = data_api(router, "/Execute", members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the Data API statement {sql:?} must be served, got {status} {body}"
    );
    body
}

/// The rows a `SELECT` through the Data API answered, as `(id, label)` pairs.
pub(crate) fn data_api_rows(body: &Value) -> Vec<(i64, String)> {
    body["records"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|record| {
            let fields = record.as_array().cloned().unwrap_or_default();
            (
                fields
                    .first()
                    .and_then(|field| field["longValue"].as_i64())
                    .unwrap_or_default(),
                fields
                    .get(1)
                    .and_then(|field| field["stringValue"].as_str())
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// The rows a result set off the engine's own wire carries, as the same `(id, label)` pairs:
/// both wires report the values as text, which is what the comparison is made on.
fn engine_rows(rows: &Rows) -> Vec<(i64, String)> {
    rows.rows
        .iter()
        .map(|row| {
            (
                row.first()
                    .cloned()
                    .flatten()
                    .unwrap_or_default()
                    .parse()
                    .unwrap_or_default(),
                row.get(1).cloned().flatten().unwrap_or_default(),
            )
        })
        .collect()
}

/// A session on the cluster's engine endpoint, which runs `sql` and answers its rows.
///
/// One session per statement rather than one per row, so each statement of a case is a
/// fresh session on the published endpoint — which is what a client that connects, reads and
/// disconnects does.
async fn on_the_engine(row: &Row, cluster: &Cluster, sql: &str) -> Rows {
    match row.wire {
        Wire::Postgres => {
            let mut session = postgres_login(
                &cluster.address,
                cluster.port,
                Transport::Tls,
                row.master_username,
                &cluster.password,
                Some(row.database),
            )
            .await
            .unwrap_or_else(|refusal| {
                panic!(
                    "case {:?}: the master user must be admitted on the cluster's endpoint, \
                     got {refusal:?}",
                    row.case
                )
            });
            session.query(sql).await.unwrap_or_else(|refusal| {
                panic!(
                    "case {:?}: the engine must run {sql:?}, got {refusal:?}",
                    row.case
                )
            })
        }
        Wire::MySql => {
            let mut session = mysql_login(
                &cluster.address,
                cluster.port,
                Transport::Plaintext,
                row.master_username,
                &cluster.password,
                Some(row.database),
            )
            .await
            .unwrap_or_else(|refusal| {
                panic!(
                    "case {:?}: the master user must be admitted on the cluster's endpoint, \
                     got {refusal:?}",
                    row.case
                )
            });
            session.query(sql).await.unwrap_or_else(|refusal| {
                panic!(
                    "case {:?}: the engine must run {sql:?}, got {refusal:?}",
                    row.case
                )
            })
        }
    }
}

/// The statement every case reads the crossing table back with, on either surface.
const SELECT_ROWS: &str = "SELECT id, label FROM crossing ORDER BY id";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_doors_of_a_db_cluster_read_and_write_one_database() {
    let router = emulator();
    for row in ROWS {
        let cluster = available_cluster(&router, row).await;

        // The Data API's door creates the table and seeds a row.
        execute(
            &router,
            row.case,
            &cluster,
            row.database,
            "CREATE TABLE crossing (id INT PRIMARY KEY, label VARCHAR(32) NOT NULL)",
            None,
        )
        .await;
        execute(
            &router,
            row.case,
            &cluster,
            row.database,
            "INSERT INTO crossing (id, label) VALUES (1, 'through the data api')",
            None,
        )
        .await;

        // A session on the cluster's own engine endpoint reads them: the table the other
        // door created is a table of this database.
        let read_natively = on_the_engine(row, &cluster, SELECT_ROWS).await;
        assert_eq!(
            engine_rows(&read_natively),
            vec![(1, "through the data api".to_string())],
            "case {:?}: a native session must read what the Data API committed, got {:?}",
            row.case,
            read_natively.rows
        );

        // And the crossing holds the other way: a row the engine's own wire inserted is read
        // through the Data API.
        on_the_engine(
            row,
            &cluster,
            "INSERT INTO crossing (id, label) VALUES (2, 'through the engine')",
        )
        .await;
        let read_through_the_api =
            execute(&router, row.case, &cluster, row.database, SELECT_ROWS, None).await;
        assert_eq!(
            data_api_rows(&read_through_the_api),
            vec![
                (1, "through the data api".to_string()),
                (2, "through the engine".to_string()),
            ],
            "case {:?}: the Data API must read what a native session committed, got \
             {read_through_the_api}",
            row.case
        );

        // The server's own catalogs of that database are still served once both doors have
        // written to it: they live inside the database a session is on.
        let catalogs = on_the_engine(row, &cluster, row.catalog_query).await;
        let counted: i64 = catalogs
            .only()
            .unwrap_or_default()
            .parse()
            .unwrap_or_default();
        assert!(
            counted > 0,
            "case {:?}: {:?} must still answer once both doors have written, got {:?}",
            row.case,
            row.catalog_query,
            catalogs.rows
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_open_data_api_transaction_is_invisible_on_the_engine_until_it_commits() {
    let router = emulator();
    for row in ROWS {
        let cluster = available_cluster(&router, row).await;
        execute(
            &router,
            row.case,
            &cluster,
            row.database,
            "CREATE TABLE crossing (id INT PRIMARY KEY, label VARCHAR(32) NOT NULL)",
            None,
        )
        .await;
        execute(
            &router,
            row.case,
            &cluster,
            row.database,
            "INSERT INTO crossing (id, label) VALUES (1, 'committed')",
            None,
        )
        .await;

        // A transaction of the Data API door, with a row written in it and not committed.
        let (status, begun) = data_api(
            &router,
            "/BeginTransaction",
            json!({
                "resourceArn": cluster.arn,
                "secretArn": cluster.secret_arn,
                "database": row.database,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: BeginTransaction must be served, got {status} {begun}",
            row.case
        );
        let transaction = begun["transactionId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: BeginTransaction answers a transactionId, got {begun}",
                    row.case
                )
            })
            .to_string();
        execute(
            &router,
            row.case,
            &cluster,
            row.database,
            "INSERT INTO crossing (id, label) VALUES (2, 'uncommitted')",
            Some(&transaction),
        )
        .await;

        // The engine's own wire sees only what is COMMITTED.
        let before = on_the_engine(row, &cluster, SELECT_ROWS).await;
        assert_eq!(
            engine_rows(&before),
            vec![(1, "committed".to_string())],
            "case {:?}: a native session must not see a Data API transaction's uncommitted \
             row, got {:?}",
            row.case,
            before.rows
        );

        // Once the transaction commits, that row is part of the database both doors read.
        let (status, committed) = data_api(
            &router,
            "/CommitTransaction",
            json!({
                "resourceArn": cluster.arn,
                "secretArn": cluster.secret_arn,
                "transactionId": transaction,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: CommitTransaction must be served, got {status} {committed}",
            row.case
        );
        let after = on_the_engine(row, &cluster, SELECT_ROWS).await;
        assert_eq!(
            engine_rows(&after),
            vec![
                (1, "committed".to_string()),
                (2, "uncommitted".to_string()),
            ],
            "case {:?}: a native session must read the committed transaction's row, got {:?}",
            row.case,
            after.rows
        );
    }
}
