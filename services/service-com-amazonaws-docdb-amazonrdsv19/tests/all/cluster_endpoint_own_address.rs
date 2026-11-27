// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS a cluster publishes for its OWN endpoints: `DBCluster$Endpoint`,
//! `DBCluster$ReaderEndpoint` and the one `DBCluster$Port` the pair of them shares.
//!
//! THE RULE: an endpoint a service advertises is an address the service ANSWERS on, so the
//! host every `DBCluster`-returning operation publishes is one of THIS emulator's own hosts —
//! the host the core bound the cluster's listener on — and never a name in Amazon DocumentDB's
//! public DNS namespace, which resolves to nothing on the machine this emulator runs on. The
//! `Port` beside it is the port the cluster's engine was configured to listen on, which is the
//! one port the model gives both endpoint members, and the address a client composes from the
//! two is one this emulator serves the cluster's document wire at.
//!
//! The tables below span what the rule generalises over: whether the create asked for a port
//! or took the engine's default, whether the endpoint is encrypted (the cluster's `tls`
//! parameter), whether the cluster authenticates a master user, how many members it has, and
//! which operation the pair is read back through. The recorded case — a cluster created with
//! none of those named — is one row of it.
//!
//! Every reachability assertion goes through the register of the planes this emulator serves
//! in process (`service_common::dialable_data_planes`), which is where a client that reads a
//! cluster endpoint out of the control plane and dials it — the code of a function attached to
//! the cluster's virtual private cloud, the one vantage point a DocumentDB cluster endpoint is
//! reachable from — opens its connection. Nothing here reaches into the service's state.

use ::bson::doc;
use ::service_common::data_plane_addressing::host_port_authority;
use ::service_common::dialable_data_planes::DialablePlanes;

use crate::cluster_lifecycle::{count, let_the_clock_catch_up, text, Docdb};
use crate::instance_data_plane::{code, ok, Session};

/// The DNS namespace Amazon DocumentDB's own endpoints live in, and the one thing the
/// published host must NOT be: nothing on the machine this emulator runs on answers there.
const SERVICE_DNS_SUFFIX: &str = "docdb.amazonaws.com";

/// `CreateDBClusterMessage$Port`: "The port number on which the instances in the cluster
/// accept connections. Default: 27017" — the port a create that names none publishes.
const DEFAULT_PORT: &str = "27017";

/// The signing name of the front door this service is a view of, which is what the register of
/// served planes records as answering at an address this service opened.
const SERVED_BY: &str = "rds";

/// The master user a cluster under test is created with, where it has one.
const MASTER_USERNAME: &str = "clusteradmin";

/// Its password, which only this test and the credential derived from it ever see.
const MASTER_PASSWORD: &str = "a-Secret-27";

/// One row of the table: a cluster to create, and what it is created with.
struct Row {
    /// What a failing assertion names.
    case: &'static str,
    /// The cluster's identifier.
    identifier: &'static str,
    /// `Port`, where the create names one; `None` takes the engine's default.
    port: Option<&'static str>,
    /// Whether the create carries a master user.
    master_user: bool,
    /// Whether the cluster's parameter group turns `tls` off, which serves its endpoints in
    /// the clear instead of TLS-from-the-first-byte.
    plaintext: bool,
    /// How many member instances the cluster is given.
    members: usize,
}

/// Create the cluster `row` describes, with its parameter group where it needs one, and let the
/// clock settle everything it owes.
async fn create(docdb: &Docdb, row: &Row) {
    let group = format!("{}-group", row.identifier);
    if row.plaintext {
        docdb
            .ok(
                row.case,
                "CreateDBClusterParameterGroup",
                &[
                    ("DBClusterParameterGroupName", group.as_str()),
                    ("DBParameterGroupFamily", "docdb5.0"),
                    ("Description", "tls off"),
                ],
            )
            .await;
        docdb
            .ok(
                row.case,
                "ModifyDBClusterParameterGroup",
                &[
                    ("DBClusterParameterGroupName", group.as_str()),
                    ("Parameters.Parameter.1.ParameterName", "tls"),
                    ("Parameters.Parameter.1.ParameterValue", "disabled"),
                    ("Parameters.Parameter.1.ApplyMethod", "immediate"),
                ],
            )
            .await;
    }
    let mut params: Vec<(&str, &str)> = vec![
        ("DBClusterIdentifier", row.identifier),
        ("Engine", "docdb"),
    ];
    if let Some(port) = row.port {
        params.push(("Port", port));
    }
    if row.master_user {
        params.push(("MasterUsername", MASTER_USERNAME));
        params.push(("MasterUserPassword", MASTER_PASSWORD));
    }
    if row.plaintext {
        params.push(("DBClusterParameterGroupName", group.as_str()));
    }
    let created = docdb.ok(row.case, "CreateDBCluster", &params).await;
    // The create's own answer carries the endpoints, so it is checked here too: the rule holds
    // of the operation that MINTS the cluster, not only of the read that follows it.
    assert_published(docdb, row.case, "CreateDBCluster", &created, row.port);
    for member in 1..=row.members {
        let instance = format!("{}-{member}", row.identifier);
        docdb
            .ok(
                row.case,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", instance.as_str()),
                    ("DBInstanceClass", "db.r5.large"),
                    ("Engine", "docdb"),
                    ("DBClusterIdentifier", row.identifier),
                ],
            )
            .await;
    }
    let_the_clock_catch_up();
}

/// Assert of one response that both endpoint members name a host this emulator answers on, that
/// the `Port` beside them is the one the cluster was created with, and that the pair a client
/// composes from the two is an address this front door serves the cluster at.
fn assert_published(
    docdb: &Docdb,
    case: &str,
    operation: &str,
    response: &str,
    requested_port: Option<&str>,
) {
    let writer = text(response, "Endpoint");
    let reader = text(response, "ReaderEndpoint");
    let port = text(response, "Port");
    assert_eq!(
        count(response, "Endpoint"),
        1,
        "case {case:?} ({operation}): a cluster publishes exactly one writer endpoint, got \
         {response}"
    );
    assert_eq!(
        port,
        requested_port.unwrap_or(DEFAULT_PORT),
        "case {case:?} ({operation}): expected the port the cluster's engine listens on, got \
         {port:?} in {response}"
    );
    for (member, value) in [("Endpoint", &writer), ("ReaderEndpoint", &reader)] {
        assert!(
            !value.ends_with(SERVICE_DNS_SUFFIX),
            "case {case:?} ({operation}): {member} must be an address this emulator answers on, \
             not a name in Amazon DocumentDB's own DNS zone, got {value:?}"
        );
        let host: std::net::IpAddr = value.parse().unwrap_or_else(|_| {
            panic!(
                "case {case:?} ({operation}): {member} must name the address the cluster's \
                 listener is bound on, got {value:?}"
            )
        });
        assert!(
            host.is_loopback(),
            "case {case:?} ({operation}): {member} must be this machine's loopback address, \
             which is where the core binds its listeners, got {value:?}"
        );
        let served = served_at(docdb, value, &port);
        assert_eq!(
            served.as_deref(),
            Some(SERVED_BY),
            "case {case:?} ({operation}): the pair {value}:{port} the control plane published \
             for {member} must be an address this front door serves the cluster at, got \
             {served:?}"
        );
    }
}

/// What this emulator serves at the `host:port` a client composes from the published members,
/// as the register of in-process planes holds it.
fn served_at(docdb: &Docdb, host: &str, port: &str) -> Option<String> {
    let port: u16 = port
        .parse()
        .unwrap_or_else(|_| panic!("the published port must be a port, got {port:?}"));
    let planes = DialablePlanes::of(&**docdb.engine())
        .expect("the engine vends the register of served data planes");
    planes.served_by(&host_port_authority(host, port))
}

/// The endpoints the cluster `identifier` publishes, read through `DescribeDBClusters` exactly
/// as a client reads them: `(writer, reader, port)`.
async fn published(docdb: &Docdb, case: &str, identifier: &str) -> (String, String, String) {
    let described = docdb
        .ok(
            case,
            "DescribeDBClusters",
            &[("DBClusterIdentifier", identifier)],
        )
        .await;
    (
        text(&described, "Endpoint"),
        text(&described, "ReaderEndpoint"),
        text(&described, "Port"),
    )
}

/// Open a session on the address the control plane published, the way a client inside the
/// cluster's virtual private cloud does: through the register of the planes this emulator
/// serves in process, at exactly the `host:port` it was handed.
async fn session_on(docdb: &Docdb, case: &str, host: &str, port: &str) -> Session {
    let port: u16 = port
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: the published port must be a port, got {port:?}"));
    let planes = DialablePlanes::of(&**docdb.engine())
        .expect("the engine vends the register of served data planes");
    let dialed = planes.dialable(host, port).unwrap_or_else(|| {
        panic!("case {case:?}: nothing is served at the published pair {host}:{port}")
    });
    let connection = dialed.plane.dial(None).await.unwrap_or_else(|error| {
        panic!("case {case:?}: {host}:{port} must accept a connection, got {error}")
    });
    Session::over(Box::new(connection))
}

#[tokio::test]
async fn every_cluster_publishes_endpoints_at_an_address_this_emulator_serves() {
    // RULE: the published host is one this emulator answers the cluster's wire on, over every
    // input the rule generalises over — the port asked for, the encryption of the wire, the
    // master user, the membership — and over both operations that report a cluster.
    let docdb = Docdb::new();
    let rows = [
        Row {
            case: "a cluster created with nothing but its engine",
            identifier: "plain-cluster",
            port: None,
            master_user: false,
            plaintext: false,
            members: 0,
        },
        Row {
            case: "a cluster with a master user and a member",
            identifier: "member-cluster",
            port: None,
            master_user: true,
            plaintext: false,
            members: 1,
        },
        Row {
            case: "a cluster that asked for a port of its own",
            identifier: "ported-cluster",
            port: Some("28017"),
            master_user: true,
            plaintext: false,
            members: 2,
        },
        Row {
            case: "a cluster whose endpoints are served in the clear",
            identifier: "plaintext-cluster",
            port: Some("27117"),
            master_user: true,
            plaintext: true,
            members: 1,
        },
    ];
    for row in &rows {
        create(&docdb, row).await;
        let described = docdb
            .ok(
                row.case,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", row.identifier)],
            )
            .await;
        assert_published(&docdb, row.case, "DescribeDBClusters", &described, row.port);
        // The address is the cluster's, so a second read reports the same one: a client may
        // keep what it was handed.
        let again = docdb
            .ok(
                row.case,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", row.identifier)],
            )
            .await;
        assert_eq!(
            (
                text(&again, "Endpoint"),
                text(&again, "ReaderEndpoint"),
                text(&again, "Port")
            ),
            (
                text(&described, "Endpoint"),
                text(&described, "ReaderEndpoint"),
                text(&described, "Port")
            ),
            "case {:?}: the published endpoints must be stable across reads, got {again}",
            row.case
        );
    }
}

#[tokio::test]
async fn the_published_cluster_endpoint_serves_the_cluster_s_documents() {
    // RULE: the pair the control plane published is an address this emulator answers the
    // engine's own wire at, and what answers there is the CLUSTER's primary over the CLUSTER's
    // documents — so a write sent to the cluster endpoint is read back through the member's
    // own endpoint, both being addresses onto one volume.
    let docdb = Docdb::new();
    let case = "a document written through the cluster endpoint";
    let row = Row {
        case,
        identifier: "wire-cluster",
        port: Some("27217"),
        master_user: true,
        plaintext: true,
        members: 1,
    };
    create(&docdb, &row).await;
    let (writer, reader, port) = published(&docdb, case, row.identifier).await;
    let mut session = session_on(&docdb, case, &writer, &port).await;

    // The handshake a driver opens with. The cluster endpoint is "the connection endpoint for
    // the primary instance of the cluster", so what answers it answers as that primary.
    let hello = session
        .legacy_command(case, "admin", doc! { "isMaster": 1_i32, "helloOk": true })
        .await;
    assert_eq!(
        ok(&hello),
        1.0,
        "case {case:?}: the published pair must answer the handshake, got {hello:?}"
    );
    assert_eq!(
        hello.get_bool("isWritablePrimary").ok(),
        Some(true),
        "case {case:?}: the cluster endpoint leads to the primary, so it serves writes, got \
         {hello:?}"
    );

    // Before the exchange the session is nobody, so a command that writes is refused.
    let unauthenticated = session
        .command(
            case,
            "shop",
            doc! { "insert": "orders", "documents": [{ "_id": 1_i32 }] },
        )
        .await;
    assert_eq!(
        (ok(&unauthenticated), code(&unauthenticated)),
        (0.0, 13),
        "case {case:?}: an unauthenticated write must be refused, got {unauthenticated:?}"
    );
    let authenticated = session
        .authenticate(case, MASTER_USERNAME, MASTER_PASSWORD)
        .await;
    assert_eq!(
        ok(&authenticated),
        1.0,
        "case {case:?}: the cluster's master user must authenticate a session on its own \
         endpoint, got {authenticated:?}"
    );

    let inserted = session
        .command(
            case,
            "shop",
            doc! {
                "insert": "orders",
                "documents": [{ "_id": 7_i32, "item": "widget", "quantity": 3_i32 }],
            },
        )
        .await;
    assert_eq!(
        (ok(&inserted), inserted.get_i32("n").ok()),
        (1.0, Some(1)),
        "case {case:?}: the write must be served, got {inserted:?}"
    );

    // The READER endpoint fronts the same plane, so a session on it finds what the writer
    // endpoint wrote.
    let mut through_reader = session_on(&docdb, case, &reader, &port).await;
    through_reader
        .authenticate(case, MASTER_USERNAME, MASTER_PASSWORD)
        .await;
    let found = through_reader
        .command(case, "shop", doc! { "find": "orders", "filter": { "_id": 7_i32 } })
        .await;
    assert_eq!(
        ok(&found),
        1.0,
        "case {case:?}: the reader endpoint must serve the read, got {found:?}"
    );
    let batch = found
        .get_document("cursor")
        .and_then(|cursor| cursor.get_array("firstBatch"))
        .unwrap_or_else(|_| panic!("case {case:?}: the read answers a cursor, got {found:?}"));
    assert_eq!(
        batch.len(),
        1,
        "case {case:?}: expected the document the cluster endpoint wrote, got {batch:?}"
    );

    // The member's OWN endpoint is a second address onto the same documents, which is what
    // makes the cluster endpoint the cluster's rather than one socket's.
    let member = docdb
        .ok(
            case,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "wire-cluster-1")],
        )
        .await;
    let mut through_member = session_on(
        &docdb,
        case,
        &text(&member, "Address"),
        &text(&member, "Port"),
    )
    .await;
    through_member
        .authenticate(case, MASTER_USERNAME, MASTER_PASSWORD)
        .await;
    let from_member = through_member
        .command(case, "shop", doc! { "find": "orders", "filter": { "_id": 7_i32 } })
        .await;
    let member_batch = from_member
        .get_document("cursor")
        .and_then(|cursor| cursor.get_array("firstBatch"))
        .unwrap_or_else(|_| {
            panic!("case {case:?}: the member's endpoint answers a cursor, got {from_member:?}")
        });
    assert_eq!(
        member_batch.len(),
        1,
        "case {case:?}: the cluster's documents are one volume, so the member's endpoint must \
         find what the cluster endpoint wrote, got {from_member:?}"
    );
}

#[tokio::test]
async fn a_cluster_endpoint_with_no_primary_serves_no_command() {
    // RULE: the cluster endpoint leads to the member the cluster writes through, so a cluster
    // that has none has no engine behind its endpoint: the handshake reaches it and nothing
    // else does. The rows span the two ways a cluster is left without one — one that has had
    // no member yet, and one whose only member has been deleted.
    let docdb = Docdb::new();
    let rows = [
        ("a cluster with no member yet", "empty-cluster", "27417", false),
        (
            "a cluster whose only member is gone",
            "emptied-cluster",
            "27517",
            true,
        ),
    ];
    for (case, identifier, port, with_member) in rows {
        let row = Row {
            case,
            identifier,
            port: Some(port),
            master_user: true,
            plaintext: true,
            members: usize::from(with_member),
        };
        create(&docdb, &row).await;
        if with_member {
            let member = format!("{identifier}-1");
            docdb
                .ok(
                    case,
                    "DeleteDBInstance",
                    &[("DBInstanceIdentifier", member.as_str())],
                )
                .await;
        }
        let (writer, _reader, port) = published(&docdb, case, identifier).await;
        let mut session = session_on(&docdb, case, &writer, &port).await;
        let hello = session
            .legacy_command(case, "admin", doc! { "isMaster": 1_i32, "helloOk": true })
            .await;
        assert_eq!(
            ok(&hello),
            1.0,
            "case {case:?}: the handshake reaches an endpoint with no engine behind it, got \
             {hello:?}"
        );
        session
            .authenticate(case, MASTER_USERNAME, MASTER_PASSWORD)
            .await;
        let read = session
            .command(case, "shop", doc! { "find": "orders", "filter": {} })
            .await;
        assert_eq!(
            ok(&read),
            0.0,
            "case {case:?}: a cluster with no primary serves no read, got {read:?}"
        );
    }
}

#[tokio::test]
async fn the_cluster_s_socket_closes_with_the_cluster() {
    // RULE: the address is the CLUSTER's, so it is open for exactly as long as the cluster is:
    // a port onto a cluster that is gone would answer out of nothing.
    let docdb = Docdb::new();
    let case = "a deleted cluster";
    let row = Row {
        case,
        identifier: "closing-cluster",
        port: Some("27317"),
        master_user: true,
        plaintext: true,
        members: 0,
    };
    create(&docdb, &row).await;
    let (writer, _reader, port) = published(&docdb, case, row.identifier).await;
    assert_eq!(
        served_at(&docdb, &writer, &port).as_deref(),
        Some(SERVED_BY),
        "case {case:?}: the published pair is served while the cluster exists"
    );
    docdb
        .ok(
            case,
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", row.identifier),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(
        served_at(&docdb, &writer, &port),
        None,
        "case {case:?}: the address must be withdrawn with the cluster it led to"
    );
}
