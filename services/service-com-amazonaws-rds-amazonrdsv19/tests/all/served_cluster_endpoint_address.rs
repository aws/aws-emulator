// SPDX-License-Identifier: CC0-1.0

//! WHERE a client opens a session on a DB cluster whose engine wire this emulator serves.
//!
//! The rule under test: the endpoint member a client DIALS carries the host the listener the
//! core opened for that cluster is answered at — the address this emulator serves that plane on
//! — beside that listener's port in the one `DBCluster$Port` ("the port that the database engine
//! is listening on"), so the pair the client composes reaches the plane that serves it, which a
//! name in a zone AWS serves, or one this service announced beneath a host of its own, would
//! not. The member the cluster publishes for its read path carries that same address.
//!
//! A cluster this emulator serves NO data plane for keeps the minted form under the published
//! AWS domain — there is no local plane for such a cluster to lead a client to — which is the
//! contrast row below.
//!
//! The rows span what the rule generalises over: both wires this emulator speaks (PostgreSQL
//! and MySQL), both cluster types (an Aurora cluster and a Multi-AZ DB cluster), a cluster
//! with no member instance at all, every operation that answers with a cluster envelope (the
//! create, the describe and `DescribeDBClusterEndpoints`), and an engine whose wire this
//! emulator serves nothing for. Each published pair is then RESOLVED and DIALLED over a real
//! TCP connection, in one process, and a row is round-tripped through it; the last test opens
//! the Amazon S3 import on the published pair, so what a client at that address reads is the
//! engine's own refusal of an import with no role associated rather than a transport failure.

use std::net::IpAddr;
use std::sync::Arc;

use crate::db_cluster_engine_endpoint::MULTI_AZ_SETTINGS;
use crate::db_clusters::{assert_served_endpoint_pair, the_cluster};
use crate::db_instance_engine_endpoint::{
    wire_login as login, Wire, DATABASE, MASTER_PASSWORD, MASTER_USER,
};
use crate::db_instances::{call, emulator_with_engine, expect_ok, subtrees, text, REGION};
use crate::lifecycle_poll_loops::poll_until;
use crate::rds_s3_integration::{create_bucket, put_object, role};
use service_com_amazonaws_rds_amazonrdsv19::state as st;

/// One cluster of the table.
struct ClusterRow {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    wire: Wire,
    /// Whether the engine is an Aurora one, which is the only cluster class
    /// `DescribeDBClusterEndpoints` "applies to".
    aurora: bool,
    /// The parameters the create adds beyond the identifier, engine, database and master user.
    extra: &'static [(&'static str, &'static str)],
}

const CLUSTERS: &[ClusterRow] = &[
    ClusterRow {
        case: "an Aurora MySQL cluster with no member instance at all",
        identifier: "emu-sca-aurora-mysql",
        engine: "aurora-mysql",
        wire: Wire::MySql,
        aurora: true,
        extra: &[],
    },
    ClusterRow {
        case: "an Aurora PostgreSQL Serverless v2 cluster",
        identifier: "emu-sca-aurora-pg",
        engine: "aurora-postgresql",
        wire: Wire::Postgres,
        aurora: true,
        extra: &[
            ("ServerlessV2ScalingConfiguration.MinCapacity", "0.5"),
            ("ServerlessV2ScalingConfiguration.MaxCapacity", "1.0"),
        ],
    },
    ClusterRow {
        case: "a Multi-AZ DB cluster, the other cluster type",
        identifier: "emu-sca-multiaz-pg",
        engine: "postgres",
        wire: Wire::Postgres,
        aurora: false,
        extra: MULTI_AZ_SETTINGS,
    },
    ClusterRow {
        case: "a Multi-AZ DB cluster on the other wire",
        identifier: "emu-sca-multiaz-mysql",
        engine: "mysql",
        wire: Wire::MySql,
        aurora: false,
        extra: MULTI_AZ_SETTINGS,
    },
];

/// Create the row's cluster and answer with the envelope the create returned.
async fn create_cluster(
    router: &smithy_server_runtime::MultiServiceRouter,
    row: &ClusterRow,
) -> String {
    let mut request: Vec<(&str, String)> = vec![
        ("DBClusterIdentifier", row.identifier.to_string()),
        ("Engine", row.engine.to_string()),
        ("DatabaseName", DATABASE.to_string()),
        ("MasterUsername", MASTER_USER.to_string()),
        ("MasterUserPassword", MASTER_PASSWORD.to_string()),
    ];
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

/// The `Endpoint` and `Port` one cluster envelope publishes, asserting on the way that the
/// endpoint member is a host carrying no port of its own.
fn published_pair(case: &str, which: &str, cluster: &str) -> (String, u16) {
    let endpoint = text(cluster, "Endpoint");
    let port: u16 = text(cluster, "Port").parse().unwrap_or_else(|_| {
        panic!(
            "case {case:?} ({which}): expected the port the engine listens on, got {:?}",
            text(cluster, "Port")
        )
    });
    assert!(
        !endpoint.is_empty() && !endpoint.ends_with(&port.to_string()),
        "case {case:?} ({which}): expected the endpoint to leave the port to Port, got \
         {endpoint:?}"
    );
    (endpoint, port)
}

/// The address the cluster's OWN engine listener is bound to, read from this service's store —
/// the address the published pair has to be.
fn listener_address(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    identifier: &str,
) -> (String, u16) {
    let state = st::service_state(&**engine).expect("this service's store");
    let store = state.lock();
    let endpoint = store
        .cluster_engine_endpoint(&(REGION.to_string(), identifier.to_string()))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the cluster must have opened an engine endpoint")
        });
    let port = u16::try_from(endpoint.endpoint.port).unwrap_or_else(|_| {
        panic!(
            "case {case:?}: expected the listener's port, got {}",
            endpoint.endpoint.port
        )
    });
    (endpoint.endpoint.host.clone(), port)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_served_clusters_published_endpoint_is_the_address_its_engine_listener_answers_on() {
    let (router, engine) = emulator_with_engine();
    for row in CLUSTERS {
        let created = create_cluster(&router, row).await;
        let settled = available_cluster(&router, row.case, row.identifier).await;

        // The pair the create published and the pair the read reports are one address.
        let (created_endpoint, created_port) = published_pair(row.case, "CreateDBCluster", &created);
        let (endpoint, port) = published_pair(row.case, "DescribeDBClusters", &settled);
        assert_eq!(
            (created_endpoint.as_str(), created_port),
            (endpoint.as_str(), port),
            "case {:?}: the create and the describe must publish one address",
            row.case
        );

        // And that address IS the listener's own: the published host is the host the core bound
        // the cluster's listener on, with that listener's port beside it — so what the client
        // dials is the interface the plane answers on. The member beside it carries the same
        // address, that being where the cluster's read path is answered too.
        let reader = text(&settled, "ReaderEndpoint");
        assert_served_endpoint_pair(row.case, &endpoint, &reader);
        let listener = listener_address(&engine, row.case, row.identifier);
        assert_eq!(
            (endpoint.as_str(), port),
            (listener.0.as_str(), listener.1),
            "case {:?}: expected the address the cluster's own listener answers on ({}:{}), got \
             {endpoint}:{port}",
            row.case,
            listener.0,
            listener.1
        );
        let resolved: Vec<std::net::SocketAddr> =
            tokio::net::lookup_host((endpoint.as_str(), port))
                .await
                .unwrap_or_else(|error| {
                    panic!(
                        "case {:?}: the published endpoint {endpoint:?} must resolve, got {error}",
                        row.case
                    )
                })
                .collect();
        let bound: IpAddr = listener
            .0
            .parse()
            .unwrap_or_else(|_| panic!("case {:?}: the listener is bound on an address", row.case));
        assert!(
            resolved.contains(&std::net::SocketAddr::new(bound, port)),
            "case {:?}: expected the published pair to resolve to the cluster's own listener \
             {bound}:{port}, got {resolved:?}",
            row.case
        );

        // `DescribeDBClusterEndpoints` reports the same writer address, for the cluster class
        // that report "only applies to".
        if row.aurora {
            let reported = expect_ok(
                row.case,
                call(
                    &router,
                    "DescribeDBClusterEndpoints",
                    &[("DBClusterIdentifier", row.identifier)],
                )
                .await,
            );
            let writers: Vec<String> = subtrees(&reported, "DBClusterEndpointList")
                .into_iter()
                .filter(|entry| text(entry, "EndpointType") == "WRITER")
                .map(|entry| text(&entry, "Endpoint"))
                .collect();
            assert_eq!(
                writers,
                vec![endpoint.clone()],
                "case {:?}: expected the WRITER endpoint {endpoint:?} in {reported}",
                row.case
            );
        }

        // The published pair is an address this emulator really answers the engine wire on: a
        // client dials it over TCP, is admitted, and reads back the row it wrote.
        let mut session = login(row.case, row.wire, &endpoint, port, DATABASE).await;
        session
            .run(
                row.case,
                "CREATE TABLE ledger (id integer PRIMARY KEY, note varchar(64))",
            )
            .await;
        session
            .run(
                row.case,
                "INSERT INTO ledger (id, note) VALUES (1, 'dialled the published endpoint')",
            )
            .await;
        let read = session
            .run(row.case, "SELECT note FROM ledger WHERE id = 1")
            .await;
        assert_eq!(
            read.only(),
            Some("dialled the published endpoint"),
            "case {:?}: expected the row the session wrote, got {read:?}",
            row.case
        );

        // An in-process client of this emulator reaches the same pair: the register it dials
        // through answers for the address the control plane published, served by this service.
        let planes = service_common::dialable_data_planes::DialablePlanes::of(&*engine)
            .expect("the engine vends the records of the planes it has opened");
        let dialed = planes.dialable(&endpoint, port);
        let served_by = dialed.as_ref().map(|dialed| dialed.served_by.clone());
        assert_eq!(
            served_by.as_deref(),
            Some("rds"),
            "case {:?}: the published pair {endpoint}:{port} must resolve to a plane this \
             emulator serves, got {dialed:?}",
            row.case
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cluster_this_emulator_serves_no_wire_for_publishes_its_control_planes_dns_name() {
    let (router, engine) = emulator_with_engine();
    // An Amazon Neptune cluster's data plane is its graph API, served at the address THIS
    // emulator is reached at — and this request is addressed to this service's own AWS host,
    // which is no address of this process's: the cluster has no local plane to lead a client
    // to, so it keeps the published AWS form.
    let case = "a cluster whose engine wire this emulator serves nothing for";
    let identifier = "emu-sca-neptune";
    let created = the_cluster(
        case,
        &expect_ok(
            case,
            call(
                &router,
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", identifier),
                    ("Engine", "neptune"),
                ],
            )
            .await,
        ),
    );
    let endpoint = text(&created, "Endpoint");
    assert!(
        endpoint.starts_with(&format!("{identifier}.cluster-"))
            && endpoint.ends_with(&format!(".{REGION}.neptune.amazonaws.com")),
        "case {case:?}: expected the published DNS form of the cluster endpoint, got \
         {endpoint:?}"
    );
    assert!(
        endpoint.parse::<IpAddr>().is_err(),
        "case {case:?}: a cluster with no plane of this emulator's publishes no address of \
         it, got {endpoint:?}"
    );
    // And there is no plane behind it, which is what makes the name the only thing it can
    // publish.
    let state = st::service_state(&*engine).expect("this service's store");
    let opened = state
        .lock()
        .cluster_engine_endpoint(&(REGION.to_string(), identifier.to_string()))
        .is_some();
    assert!(
        !opened,
        "case {case:?}: this emulator opens no engine listener for the cluster"
    );
}

/// The bucket and object the import below reads.
const BUCKET: &str = "emu-sca-s3-import";
const OBJECT_KEY: &str = "import/rows.csv";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_at_the_published_endpoint_reads_the_engines_own_refusal_of_an_unconfigured_import()
{
    let (router, _engine) = crate::rds_s3_integration::emulator();
    create_bucket(&router, BUCKET, REGION).await;
    put_object(&router, BUCKET, OBJECT_KEY, b"1,alice\n2,bob\n".to_vec()).await;
    // A role this emulator's IAM holds, which the cluster is NOT associated with for the
    // import feature: the configuration the import is refused for.
    let _unassociated = role(&router, "rds-sca-import", st::RDS_SERVICE_PRINCIPAL, None).await;

    let case = "an Aurora MySQL cluster with no import role associated";
    let identifier = "emu-sca-s3-cluster";
    let row = ClusterRow {
        case,
        identifier,
        engine: "aurora-mysql",
        wire: Wire::MySql,
        aurora: true,
        extra: &[],
    };
    create_cluster(&router, &row).await;
    let settled = available_cluster(&router, case, identifier).await;
    let (endpoint, port) = published_pair(case, "DescribeDBClusters", &settled);

    // What the issue this pins is about: a client that dials the address the control plane
    // published opens a session and is answered by the ENGINE, so the import's refusal names
    // the Amazon S3 capability the cluster has not got instead of a transport failure.
    let mut session = login(case, row.wire, &endpoint, port, DATABASE).await;
    session
        .run(case, "CREATE TABLE landing (id int, name varchar(32))")
        .await;
    let statement = format!(
        "LOAD DATA FROM S3 's3://{BUCKET}/{OBJECT_KEY}' INTO TABLE landing FIELDS TERMINATED BY ','"
    );
    let refusal = session
        .query(&statement)
        .await
        .expect_err("the import must be refused for a cluster with no role associated");
    assert!(
        refusal.message.contains("S3"),
        "case {case:?}: expected the engine's own refusal to name the missing Amazon S3 \
         capability, got {refusal:?}"
    );
    // And it loaded nothing.
    let loaded = session.run(case, "SELECT count(*) FROM landing").await;
    assert_eq!(
        loaded.only(),
        Some("0"),
        "case {case:?}: expected the refused import to have loaded no row, got {loaded:?}"
    );
}
