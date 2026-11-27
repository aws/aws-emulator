// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the port a DB CLUSTER's member instances accept connections on.
//!
//! The rule under test: for a cluster that manages its members' settings, the port is a
//! CLUSTER-level property every member takes on. `CreateDBCluster$Port` is "the port number
//! on which the instances in the DB cluster accept connections" and `CreateDBInstance$Port`
//! "doesn't apply to Aurora DB instances. The port number is managed by the cluster", so a
//! member's `Endpoint.Port` is the port its cluster is on — whatever its own create asked
//! for — and a `ModifyDBCluster` that moves the cluster's port moves its members with it.
//! `Endpoint.Address` + `Endpoint.Port` is the one pair a client dials, so the cluster record
//! and the member record can never contradict each other about it.
//!
//! The table below spans what the rule generalises over: the cluster's port (the engine
//! default the create names none for, a port the create chooses, and each published bound of
//! the range a port may be in plus one just inside it), whether the member's own create names
//! a `Port` at all, and whether that port differs from or equals its cluster's. The second
//! test spans TIME instead: the port the member reports after the cluster's port is changed,
//! and after modifications that name no port at all.
//!
//! Every assertion reads the XML the emulator answered, through the one wire harness this
//! test target shares (`crate::db_instances`).

use crate::db_clusters::{describe_one as describe_cluster, the_cluster};
use crate::db_instance_engine_endpoint::endpoint_of;
use crate::db_instance_read_replicas_of_clusters::poll_cluster_until_available;
use crate::db_instances::{call, emulator, expect_ok, text, the_instance, wait_for_settle};
use smithy_server_runtime::MultiServiceRouter;

/// The engine whose cluster members publish the cluster's own port beside the DNS name they
/// keep: the graph engine of the Amazon Neptune control plane this front door serves, for
/// which this emulator opens no engine listener of its own.
const ENGINE: &str = "neptune";

/// The instance class the orderable-options report offers for a member of such a cluster.
const MEMBER_CLASS: &str = "db.r5.large";

/// The port the graph engine accepts connections on when neither create names one.
/// Source: `com.amazonaws.neptune#CreateDBClusterMessage$Port` — "Default: 8182".
const ENGINE_DEFAULT_PORT: &str = "8182";

/// One row of the inherited-port table.
struct MemberPortCase {
    case: &'static str,
    /// The `Port` the cluster's create names, or `None` when it names none.
    cluster_port: Option<&'static str>,
    /// The `Port` the member's create names, or `None` when it names none.
    member_port: Option<&'static str>,
    /// The port both records must publish.
    expected: &'static str,
}

/// The rows span the cluster's port and the member's own `Port` member.
///
/// The bounds are the ones `CreateDBInstance$Port` publishes for a port at all ("Valid
/// Values: `1150-65535`"), which is the range the cluster's port is refused outside of; the
/// row just inside the floor is there so a passing floor row cannot be an off-by-one.
const MEMBER_PORT_CASES: &[MemberPortCase] = &[
    MemberPortCase {
        case: "neither create names a port, so the engine default",
        cluster_port: None,
        member_port: None,
        expected: ENGINE_DEFAULT_PORT,
    },
    MemberPortCase {
        case: "a port the cluster's create chose",
        cluster_port: Some("8183"),
        member_port: None,
        expected: "8183",
    },
    MemberPortCase {
        case: "the member names a port of its own, which does not apply",
        cluster_port: Some("8191"),
        member_port: Some("7777"),
        expected: "8191",
    },
    MemberPortCase {
        case: "the member names the same port its cluster is on",
        cluster_port: Some("9999"),
        member_port: Some("9999"),
        expected: "9999",
    },
    MemberPortCase {
        case: "the lowest port the published range admits",
        cluster_port: Some("1150"),
        member_port: None,
        expected: "1150",
    },
    MemberPortCase {
        case: "one port inside that floor",
        cluster_port: Some("1151"),
        member_port: Some("1150"),
        expected: "1151",
    },
    MemberPortCase {
        case: "the highest port the published range admits",
        cluster_port: Some("65535"),
        member_port: None,
        expected: "65535",
    },
];

/// `CreateDBCluster` for `identifier`, on `port` when the row names one.
async fn create_cluster(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    port: Option<&str>,
) -> String {
    let mut params: Vec<(&str, String)> = vec![
        ("DBClusterIdentifier", identifier.to_string()),
        ("Engine", ENGINE.to_string()),
    ];
    if let Some(port) = port {
        params.push(("Port", port.to_string()));
    }
    let body = expect_ok(case, call(router, "CreateDBCluster", &params).await);
    the_cluster(case, &body)
}

/// `CreateDBInstance` joining `cluster`, on `port` when the row names one.
async fn create_member(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    cluster: &str,
    port: Option<&str>,
) -> String {
    let mut params: Vec<(&str, String)> = vec![
        ("DBInstanceIdentifier", identifier.to_string()),
        ("Engine", ENGINE.to_string()),
        ("DBInstanceClass", MEMBER_CLASS.to_string()),
        ("DBClusterIdentifier", cluster.to_string()),
    ];
    if let Some(port) = port {
        params.push(("Port", port.to_string()));
    }
    let body = expect_ok(case, call(router, "CreateDBInstance", &params).await);
    the_instance(case, &body)
}

/// `DescribeDBInstances` for one member, as the one `<DBInstance>` it reports.
async fn describe_member(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", identifier)],
        )
        .await,
    );
    the_instance(case, &body)
}

/// Assert the member publishes `expected` and that its cluster publishes the same value, so
/// the pair a client dials is one answer.
fn assert_one_port(case: &str, label: &str, cluster: &str, member: &str, expected: &str) {
    let case = format!("{case} ({label})");
    let cluster_port = text(cluster, "Port");
    assert_eq!(
        cluster_port, expected,
        "case {case:?}: expected the cluster's <Port> {expected:?}, got {cluster_port:?} in \
         {cluster}"
    );
    let (address, port) = endpoint_of(member);
    assert_eq!(
        port.to_string(),
        expected,
        "case {case:?}: expected the member's Endpoint.Port {expected:?} — the port its \
         cluster accepts connections on — got {port} beside address {address:?}"
    );
}

#[tokio::test]
async fn a_cluster_member_accepts_connections_on_its_clusters_port() {
    let router = emulator();

    for (index, row) in MEMBER_PORT_CASES.iter().enumerate() {
        let cluster = format!("emu-member-port-cluster-{index}");
        let member = format!("emu-member-port-{index}");
        let created_cluster =
            create_cluster(&router, row.case, &cluster, row.cluster_port).await;
        let created_member =
            create_member(&router, row.case, &member, &cluster, row.member_port).await;
        assert_one_port(
            row.case,
            "the creates",
            &created_cluster,
            &created_member,
            row.expected,
        );

        // ROUND TRIP: the value is resolved from the stored cluster, so the describes report
        // the same one once the transitions the clock owes have come due.
        wait_for_settle();
        let read_cluster = describe_cluster(&router, row.case, &cluster).await;
        let read_member = describe_member(&router, row.case, &member).await;
        assert_one_port(
            row.case,
            "the describes",
            &read_cluster,
            &read_member,
            row.expected,
        );
    }
}

#[tokio::test]
async fn a_members_port_follows_its_clusters_and_nothing_else() {
    let router = emulator();
    let cluster = "emu-member-port-modified";
    let member = "emu-member-port-modified-1";
    create_cluster(&router, "the cluster is created", cluster, None).await;
    create_member(&router, "the member joins", member, cluster, Some("7777")).await;

    // The ports a change to the CLUSTER moves its member onto: a port the member's own
    // create asked for, the floor of the published range, and the engine default again.
    for port in ["7777", "1150", ENGINE_DEFAULT_PORT] {
        let case = format!("the cluster is moved to port {port}");
        // A port change takes the cluster out of service, so the next one waits for the
        // CONDITION that it is modifiable again rather than for a span of time.
        poll_cluster_until_available(&router, &case, cluster).await;
        let modified = the_cluster(
            &case,
            &expect_ok(
                &case,
                call(
                    &router,
                    "ModifyDBCluster",
                    &[
                        ("DBClusterIdentifier", cluster),
                        ("Port", port),
                        ("ApplyImmediately", "true"),
                    ],
                )
                .await,
            ),
        );
        let read_member = describe_member(&router, &case, member).await;
        assert_one_port(&case, "the modify", &modified, &read_member, port);
    }

    // A modification that names NO port leaves the port where it is — on the cluster and on
    // its member alike.
    let case = "an unrelated ModifyDBCluster";
    poll_cluster_until_available(&router, case, cluster).await;
    let modified = the_cluster(
        case,
        &expect_ok(
            case,
            call(
                &router,
                "ModifyDBCluster",
                &[
                    ("DBClusterIdentifier", cluster),
                    ("PreferredMaintenanceWindow", "sun:05:00-sun:05:30"),
                    ("ApplyImmediately", "true"),
                ],
            )
            .await,
        ),
    );
    let read_member = describe_member(&router, case, member).await;
    assert_one_port(case, "the modify", &modified, &read_member, ENGINE_DEFAULT_PORT);

    let case = "an unrelated ModifyDBInstance";
    let modified_member = the_instance(
        case,
        &expect_ok(
            case,
            call(
                &router,
                "ModifyDBInstance",
                &[
                    ("DBInstanceIdentifier", member),
                    ("PreferredMaintenanceWindow", "mon:05:00-mon:05:30"),
                    ("ApplyImmediately", "true"),
                ],
            )
            .await,
        ),
    );
    let read_cluster = describe_cluster(&router, case, cluster).await;
    assert_one_port(
        case,
        "the modify",
        &read_cluster,
        &modified_member,
        ENGINE_DEFAULT_PORT,
    );

    // The rule from the other direction: a modification of the MEMBER that names a port of
    // its own does not move the port the member accepts connections on, because that port is
    // the cluster's — `CreateDBInstance$Port` "doesn't apply to Aurora DB instances. The port
    // number is managed by the cluster", and `ModifyDBInstance$DBPortNumber` names the same
    // setting on the same member.
    let case = "a ModifyDBInstance naming a port of the member's own";
    let modified_member = the_instance(
        case,
        &expect_ok(
            case,
            call(
                &router,
                "ModifyDBInstance",
                &[
                    ("DBInstanceIdentifier", member),
                    ("DBPortNumber", "1235"),
                    ("ApplyImmediately", "true"),
                ],
            )
            .await,
        ),
    );
    let read_cluster = describe_cluster(&router, case, cluster).await;
    assert_one_port(
        case,
        "the modify",
        &read_cluster,
        &modified_member,
        ENGINE_DEFAULT_PORT,
    );
    let read_member = describe_member(&router, case, member).await;
    assert_one_port(
        case,
        "the describes",
        &read_cluster,
        &read_member,
        ENGINE_DEFAULT_PORT,
    );
}
