// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the PORT a DB cluster — and a member of it — reports.
//!
//! The rule under test: `DBCluster$Port` ("the port that the database engine is listening
//! on") is the port the request CONFIGURED — `CreateDBCluster$Port` is "the port number on
//! which the instances in the DB cluster accept connections" with the engine's documented
//! default where the request names none, and a later `ModifyDBCluster$Port` moves it — so it
//! is a stored property of the cluster: every read of the cluster answers the value its own
//! create or modification carried, whatever address the request arrived at. The port the
//! serving process happens to be bound to is a property of that PROCESS, not of the cluster,
//! so a cluster whose data plane this emulator serves as an AWS API publishes that address
//! through its endpoint HOST alone. A member instance whose cluster MANAGES its port
//! (`CreateDBInstance$Port`: "doesn't apply … The port number is managed by the cluster") is
//! reached at the port its cluster publishes and at no port of its own.
//!
//! The rows span what the rule generalises over: the port the create asks for (none, both
//! published bounds of the range an endpoint port may fall in, the engine default and two
//! values away from it), the address the client is configured with (loopback by number and by
//! name, an IPv6 literal, four different ports), and whether the member instance's own
//! create named a port at all.
//!
//! The boundary of the rule — a cluster whose own engine LISTENER this emulator opened, which
//! publishes the port that listener answers on — is pinned by
//! `api_plane_cluster_endpoint.rs`, and the refusal of a port outside the published range by
//! `endpoint_port_and_busy_cluster_wording.rs`. Every call here goes over the awsQuery wire
//! through the harness in `db_instances.rs`, and every wait is for the CONDITION through the
//! poll loop this test target shares.

use service_com_amazonaws_rds_amazonrdsv19::state::{NEPTUNE_DEFAULT_PORT, PORT_RANGE};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_clusters::the_cluster;
use crate::db_instances::{
    call, call_addressed_to, emulator, expect_error, expect_ok, subtrees, text, REGION,
};
use crate::lifecycle_poll_loops::poll_until;

/// The engine whose data plane this emulator serves as an AWS API at its OWN endpoint, so
/// that a cluster of it publishes this process's host — the case the rule is about, because
/// nothing else could tempt the projection to publish this process's PORT as well.
const GRAPH_ENGINE: &str = "neptune";

/// The instance class the orderable-options report offers for a member of such a cluster.
const MEMBER_CLASS: &str = "db.r5.large";

/// The status a cluster and its members reach once the transition their create owes has
/// settled.
const AVAILABLE: &str = "available";

/// The Query API error code AWS answers a parameter value it does not accept with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// One row of the configured-port table.
struct PortCase {
    case: &'static str,
    /// The endpoint the client is configured with: the address this emulator is reached at,
    /// whose own port is never the cluster's.
    endpoint: &'static str,
    /// The host of that endpoint, which every endpoint member of the cluster must report.
    host: &'static str,
    /// The `Port` the create asks for, or `None` for a create that asks for none.
    requested: Option<i32>,
    /// The port every read of the cluster must answer.
    expected: i32,
}

/// One step of the modification walk: the case, the parameters the `ModifyDBCluster` carries
/// beyond the identifier, and the port every later read must answer after it.
type ModifyStep = (&'static str, Vec<(&'static str, String)>, i32);

/// The `Port` of one `<DBCluster>` envelope, as the number it carries.
fn reported_port(case: &str, cluster: &str) -> i32 {
    let port = text(cluster, "Port");
    port.parse().unwrap_or_else(|error| {
        panic!("case {case:?}: expected a numeric <Port>, got {port:?} ({error}) in {cluster}")
    })
}

/// Poll `DescribeDBClusters` until the cluster reports [`AVAILABLE`], and hand back the
/// `<DBCluster>` it settled on.
async fn available_cluster(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let params = vec![("DBClusterIdentifier", identifier.to_string())];
    let read = || call(router, "DescribeDBClusters", &params);
    let (body, _) = poll_until(
        case,
        &format!("{identifier} reaching {AVAILABLE}"),
        read,
        |http_status, body| {
            http_status == 200
                && subtrees(body, "DBCluster")
                    .first()
                    .is_some_and(|cluster| text(cluster, "Status") == AVAILABLE)
        },
    )
    .await;
    the_cluster(case, &body)
}

/// Poll `DescribeDBInstances` until the instance reports [`AVAILABLE`], and hand back the
/// `<DBInstance>` it settled on.
async fn available_instance(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let params = vec![("DBInstanceIdentifier", identifier.to_string())];
    let read = || call(router, "DescribeDBInstances", &params);
    let (body, _) = poll_until(
        case,
        &format!("{identifier} reaching {AVAILABLE}"),
        read,
        |http_status, body| {
            http_status == 200
                && subtrees(body, "DBInstance")
                    .first()
                    .is_some_and(|instance| text(instance, "DBInstanceStatus") == AVAILABLE)
        },
    )
    .await;
    subtrees(&body, "DBInstance")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the describe must report the instance"))
}

/// `CreateDBCluster` for `row`, addressed to the endpoint its client is configured with.
async fn create_cluster(
    router: &MultiServiceRouter,
    row: &PortCase,
    identifier: &str,
) -> (u16, String) {
    let mut params: Vec<(&str, String)> = vec![
        ("DBClusterIdentifier", identifier.to_string()),
        ("Engine", GRAPH_ENGINE.to_string()),
    ];
    if let Some(port) = row.requested {
        params.push(("Port", port.to_string()));
    }
    call_addressed_to(router, row.endpoint, REGION, "CreateDBCluster", &params).await
}

#[tokio::test]
async fn a_cluster_reports_the_port_its_create_configured_on_every_read() {
    let router = emulator();
    // `CreateDBClusterMessage$Port` of the graph control plane's model documents the default
    // this emulator holds as `NEPTUNE_DEFAULT_PORT`; the two bounds come from the range an
    // endpoint port may fall in ("Valid Values: 1150-65535", `PORT_RANGE`), so the table
    // cannot drift from either.
    let floor = *PORT_RANGE.start();
    let ceiling = *PORT_RANGE.end();
    let cases = [
        PortCase {
            case: "a create that names no port at all",
            endpoint: "https://127.0.0.1:9297/",
            host: "127.0.0.1",
            requested: None,
            expected: NEPTUNE_DEFAULT_PORT,
        },
        PortCase {
            case: "the engine default, asked for explicitly",
            endpoint: "https://127.0.0.1:44903/",
            host: "127.0.0.1",
            requested: Some(NEPTUNE_DEFAULT_PORT),
            expected: NEPTUNE_DEFAULT_PORT,
        },
        PortCase {
            case: "one port above the engine default",
            endpoint: "https://localhost:41509/",
            host: "localhost",
            requested: Some(NEPTUNE_DEFAULT_PORT + 1),
            expected: NEPTUNE_DEFAULT_PORT + 1,
        },
        PortCase {
            case: "the floor of the published range",
            endpoint: "https://127.0.0.1:36751/",
            host: "127.0.0.1",
            requested: Some(floor),
            expected: floor,
        },
        PortCase {
            case: "the ceiling of the published range",
            endpoint: "https://[::1]:38001/",
            host: "[::1]",
            requested: Some(ceiling),
            expected: ceiling,
        },
        PortCase {
            case: "a port well away from the engine default",
            endpoint: "https://127.0.0.1:34723/",
            host: "127.0.0.1",
            requested: Some(9999),
            expected: 9999,
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let case = row.case;
        let identifier = format!("emu-cfgport-{index}");
        let created = the_cluster(
            case,
            &expect_ok(case, create_cluster(&router, row, &identifier).await),
        );
        assert_eq!(
            reported_port(case, &created),
            row.expected,
            "case {case:?}: expected <Port> {} — the port this create configured — got {} \
             in {created}",
            row.expected,
            reported_port(case, &created)
        );
        // The address this emulator is reached at is published through the HOST members, so
        // the pair is still an address of this process and the port is still the cluster's.
        for member in ["Endpoint", "ReaderEndpoint"] {
            let published = text(&created, member);
            assert_eq!(
                published, row.host,
                "case {case:?}: expected <{member}> {:?}, got {published:?} in {created}",
                row.host
            );
        }

        // ROUND TRIP: the port is stored on the record, so the reads answer it unchanged —
        // the same value while the cluster is still coming up and once it is available.
        let settled = available_cluster(&router, case, &identifier).await;
        assert_eq!(
            reported_port(case, &settled),
            row.expected,
            "case {case:?}: DescribeDBClusters must report <Port> {}, got {} in {settled}",
            row.expected,
            reported_port(case, &settled)
        );
        let again = available_cluster(&router, case, &identifier).await;
        assert_eq!(
            reported_port(case, &again),
            reported_port(case, &settled),
            "case {case:?}: two reads of one cluster must answer one port, got {} then {}",
            reported_port(case, &settled),
            reported_port(case, &again)
        );
    }
}

#[tokio::test]
async fn a_modification_moves_the_port_and_leaves_it_alone_when_it_is_refused_or_unrelated() {
    let router = emulator();
    let endpoint = "https://127.0.0.1:44377/";
    let identifier = "emu-cfgport-modified";
    let row = PortCase {
        case: "a cluster whose port a modification moves",
        endpoint,
        host: "127.0.0.1",
        requested: Some(NEPTUNE_DEFAULT_PORT),
        expected: NEPTUNE_DEFAULT_PORT,
    };
    let case = row.case;
    expect_ok(case, create_cluster(&router, &row, identifier).await);
    available_cluster(&router, case, identifier).await;

    // Each step of the walk: what the modification carries, and the port every later read
    // must answer. The rows span a move to another port, a refused move, an unrelated
    // modification and a move back.
    let floor = *PORT_RANGE.start();
    let steps: [ModifyStep; 4] = [
        (
            "a move to the floor of the published range",
            vec![("Port", floor.to_string())],
            floor,
        ),
        (
            "a move one below that floor, which is refused",
            vec![("Port", (floor - 1).to_string())],
            floor,
        ),
        (
            "an unrelated modification",
            vec![(
                "PreferredMaintenanceWindow",
                "sun:05:00-sun:05:30".to_string(),
            )],
            floor,
        ),
        (
            "a move back to the engine default",
            vec![("Port", NEPTUNE_DEFAULT_PORT.to_string())],
            NEPTUNE_DEFAULT_PORT,
        ),
    ];

    for (step, modification, expected) in steps {
        let mut params: Vec<(&str, String)> =
            vec![("DBClusterIdentifier", identifier.to_string())];
        params.extend(modification.iter().cloned());
        let refused = modification
            .iter()
            .any(|(name, value)| *name == "Port" && value.parse::<i32>().is_ok_and(|port| !PORT_RANGE.contains(&port)));
        let answered = call_addressed_to(
            &router,
            endpoint,
            REGION,
            "ModifyDBCluster",
            &params,
        )
        .await;
        if refused {
            expect_error(step, answered, 400, INVALID_PARAMETER_VALUE);
        } else {
            let body = expect_ok(step, answered);
            let modified = the_cluster(step, &body);
            assert_eq!(
                reported_port(step, &modified),
                expected,
                "case {step:?}: expected the modification to answer <Port> {expected}, got {} \
                 in {modified}",
                reported_port(step, &modified)
            );
        }
        let settled = available_cluster(&router, step, identifier).await;
        assert_eq!(
            reported_port(step, &settled),
            expected,
            "case {step:?}: expected the cluster to report <Port> {expected} after it, got {} \
             in {settled}",
            reported_port(step, &settled)
        );
    }
}

#[tokio::test]
async fn a_member_instance_is_reached_at_its_clusters_port() {
    let router = emulator();
    let endpoint = "https://127.0.0.1:41587/";
    let cluster_port = NEPTUNE_DEFAULT_PORT + 1;
    let row = PortCase {
        case: "a cluster whose members inherit its custom port",
        endpoint,
        host: "127.0.0.1",
        requested: Some(cluster_port),
        expected: cluster_port,
    };
    let cluster = "emu-cfgport-members";
    expect_ok(row.case, create_cluster(&router, &row, cluster).await);
    available_cluster(&router, row.case, cluster).await;

    // Whether the member's own create named a port or not, the port it is reached at is its
    // CLUSTER's: `Port` "doesn't apply" to such a member.
    let members: [(&str, Option<i32>); 2] = [
        ("a member whose create names no port of its own", None),
        ("a member whose create names a port of its own", Some(7777)),
    ];

    for (index, (case, requested)) in members.into_iter().enumerate() {
        let identifier = format!("emu-cfgport-member-{index}");
        let mut params: Vec<(&str, String)> = vec![
            ("DBInstanceIdentifier", identifier.clone()),
            ("Engine", GRAPH_ENGINE.to_string()),
            ("DBInstanceClass", MEMBER_CLASS.to_string()),
            ("DBClusterIdentifier", cluster.to_string()),
        ];
        if let Some(port) = requested {
            params.push(("Port", port.to_string()));
        }
        expect_ok(
            case,
            call_addressed_to(&router, endpoint, REGION, "CreateDBInstance", &params).await,
        );

        let instance = available_instance(&router, case, &identifier).await;
        let endpoint_member = subtrees(&instance, "Endpoint")
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: the instance must report an Endpoint"));
        let port = text(&endpoint_member, "Port");
        assert_eq!(
            port,
            cluster_port.to_string(),
            "case {case:?}: expected the member's Endpoint.Port to be its cluster's \
             {cluster_port}, got {port:?} in {endpoint_member}"
        );
        let address = text(&endpoint_member, "Address");
        assert_eq!(
            address, row.host,
            "case {case:?}: expected the member's Endpoint.Address {:?}, got {address:?}",
            row.host
        );
        // And the cluster still answers the port it was created with: a member joining it
        // moves nothing.
        let settled = available_cluster(&router, case, cluster).await;
        assert_eq!(
            reported_port(case, &settled),
            cluster_port,
            "case {case:?}: expected the cluster to still report <Port> {cluster_port}, got {} \
             in {settled}",
            reported_port(case, &settled)
        );
    }
}
