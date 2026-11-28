// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the global-database membership a `DBCluster` envelope reports: the
//! IDENTIFIER of the global database a cluster belongs to, beside the immutable RESOURCE ID
//! that same global database publishes for itself.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper
//! this test target has, as a real awsQuery form POST carrying the SigV4 credential scope
//! the router keys on, so every assertion reads the XML the emulator itself produced.
//!
//! The rule under test: a cluster that belongs to a global database names that global
//! database wherever a cluster envelope is reported — the create that minted the cluster and
//! every later read — and a cluster that belongs to none names none at all. The membership
//! is read from the global database's own record, so it does not move between reads and it
//! appears and disappears exactly as the membership itself does, while the global database
//! keeps the immutable resource id it publishes for itself throughout.
//!
//! The table spans what the rule generalises over: both ways a cluster becomes a member (a
//! create that names the global database, and a later `CreateGlobalCluster` that adopts a
//! stored cluster), the identifier spelled in another case, both engines a global database
//! publishes, and a cluster that joins nothing.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_clusters::{describe_one, the_cluster};
use crate::db_instances::{call, emulator, expect_ok, optional_text, text, REGION};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// How the member cluster of one row comes to belong to its global database.
#[derive(Debug, Clone, Copy)]
enum Joined {
    /// `CreateDBCluster` names the global database the new cluster is created into.
    ByCreate,
    /// `CreateDBCluster` names none and a later `CreateGlobalCluster` adopts the cluster.
    ByAdoption,
    /// The cluster joins nothing: it belongs to no global database at all.
    Never,
}

/// One row of the membership table.
struct MembershipCase {
    case: &'static str,
    /// The DB cluster the row creates.
    cluster: &'static str,
    /// The global database the row creates, where it creates one.
    global: Option<&'static str>,
    engine: &'static str,
    version: &'static str,
    joined: Joined,
    /// How the row SPELLS the global database's identifier when it names it, so the
    /// case-insensitive resolution of that name is covered too.
    spelled: fn(&str) -> String,
}

/// The identifier as the caller wrote it.
fn verbatim(identifier: &str) -> String {
    identifier.to_string()
}

/// The identifier in another case: a global database is named case-insensitively, so this
/// names the same one.
fn shouted(identifier: &str) -> String {
    identifier.to_ascii_uppercase()
}

/// The rows the rule is asserted over.
const MEMBERSHIP_CASES: &[MembershipCase] = &[
    MembershipCase {
        case: "aurora-mysql created into its global database",
        cluster: "emu-gcrid-mysql",
        global: Some("emu-gcrid-global-mysql"),
        engine: "aurora-mysql",
        version: "8.0.mysql_aurora.3.05.2",
        joined: Joined::ByCreate,
        spelled: verbatim,
    },
    MembershipCase {
        case: "aurora-postgresql created into its global database, named in another case",
        cluster: "emu-gcrid-postgres",
        global: Some("emu-gcrid-global-postgres"),
        engine: "aurora-postgresql",
        version: "15.8",
        joined: Joined::ByCreate,
        spelled: shouted,
    },
    MembershipCase {
        case: "aurora-postgresql adopted by a later CreateGlobalCluster",
        cluster: "emu-gcrid-adopted",
        global: Some("emu-gcrid-global-adopted"),
        engine: "aurora-postgresql",
        version: "15.8",
        joined: Joined::ByAdoption,
        spelled: verbatim,
    },
    MembershipCase {
        case: "aurora-mysql that belongs to no global database",
        cluster: "emu-gcrid-standalone",
        global: None,
        engine: "aurora-mysql",
        version: "8.0.mysql_aurora.3.05.2",
        joined: Joined::Never,
        spelled: verbatim,
    },
];

/// A DB cluster's ARN, which carries the region the cluster lives in: the form
/// `RemoveFromGlobalCluster$DbClusterIdentifier` documents.
fn db_cluster_arn(identifier: &str) -> String {
    format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{identifier}")
}

/// The `GlobalClusterResourceId` of the one `<GlobalCluster>` `DescribeGlobalClusters`
/// reports for `identifier`.
async fn global_resource_id(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> Option<String> {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeGlobalClusters",
            &[("GlobalClusterIdentifier", identifier)],
        )
        .await,
    );
    optional_text(&body, "GlobalClusterResourceId")
}

/// The `GlobalClusterIdentifier` one cluster envelope carries.
fn membership(cluster: &str) -> Option<String> {
    optional_text(cluster, "GlobalClusterIdentifier")
}

/// The cluster of one row, created and joined as the row says, with the envelope the create
/// answered and the resource id the global database reports for itself.
async fn provision(
    router: &MultiServiceRouter,
    row: &MembershipCase,
) -> (String, Option<String>) {
    let case = row.case;
    // A global database created EMPTY, so the cluster's own create can join it.
    if let (Some(global), Joined::ByCreate) = (row.global, row.joined) {
        expect_ok(
            case,
            call(
                router,
                "CreateGlobalCluster",
                &[
                    ("GlobalClusterIdentifier", global.to_string()),
                    ("Engine", row.engine.to_string()),
                    ("EngineVersion", row.version.to_string()),
                ],
            )
            .await,
        );
    }

    let mut parameters = vec![
        ("DBClusterIdentifier", row.cluster.to_string()),
        ("Engine", row.engine.to_string()),
        ("EngineVersion", row.version.to_string()),
    ];
    if let (Some(global), Joined::ByCreate) = (row.global, row.joined) {
        parameters.push(("GlobalClusterIdentifier", (row.spelled)(global)));
    }
    let created = the_cluster(
        case,
        &expect_ok(case, call(router, "CreateDBCluster", &parameters).await),
    );

    // A global database created AROUND the stored cluster adopts it as its primary, which is
    // the second way a cluster becomes a member.
    if let (Some(global), Joined::ByAdoption) = (row.global, row.joined) {
        expect_ok(
            case,
            call(
                router,
                "CreateGlobalCluster",
                &[
                    ("GlobalClusterIdentifier", global.to_string()),
                    ("SourceDBClusterIdentifier", row.cluster.to_string()),
                ],
            )
            .await,
        );
    }

    let expected = match row.global {
        Some(global) => global_resource_id(router, case, global).await,
        None => None,
    };
    (created, expected)
}

/// A cluster names the global database it belongs to, and a cluster that belongs to none
/// names nothing. The create's envelope and every later read answer the same, while the
/// global database publishes an immutable resource id of its own.
#[tokio::test]
async fn a_member_cluster_reports_its_global_databases_resource_id() {
    let router = emulator();

    for row in MEMBERSHIP_CASES {
        let case = row.case;
        let (created, published) = provision(&router, row).await;
        let member = matches!(row.joined, Joined::ByCreate | Joined::ByAdoption);

        if member {
            assert!(
                published.as_ref().is_some_and(|id| !id.is_empty()),
                "case {case:?}: the global database a cluster joined must publish a resource \
                 id of its own, got {published:?}"
            );
        }

        // A cluster the create joined reports the membership at once; one adopted later
        // reports it from the adoption onwards, so the create's own envelope is asserted
        // only where the create itself carried the membership.
        if matches!(row.joined, Joined::ByCreate) {
            let identifier = membership(&created);
            assert_eq!(
                identifier.as_deref(),
                row.global,
                "case {case:?}: the create's envelope must report the global database it \
                 joined, got {identifier:?}"
            );
        }

        // And every read of the cluster reports the same membership, twice over: it is a
        // fact about the global database's record, not something a read mints.
        for attempt in 1..=2 {
            let read = describe_one(&router, case, row.cluster).await;
            let identifier = membership(&read);
            assert_eq!(
                identifier.as_deref(),
                row.global.filter(|_| member),
                "case {case:?} read {attempt}: expected the global database identifier \
                 {:?}, got {identifier:?}",
                row.global.filter(|_| member)
            );
        }
    }
}

/// The member a detach removes stops naming the global database: the membership is read from
/// the global database's own record rather than copied onto the cluster when it was created.
#[tokio::test]
async fn a_detached_cluster_reports_neither_half_of_the_membership() {
    let router = emulator();
    let case = "a detached member";
    let cluster = "emu-gcrid-detached";
    let global = "emu-gcrid-global-detached";

    expect_ok(
        case,
        call(
            &router,
            "CreateGlobalCluster",
            &[
                ("GlobalClusterIdentifier", global.to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("EngineVersion", "8.0.mysql_aurora.3.05.2".to_string()),
            ],
        )
        .await,
    );
    expect_ok(
        case,
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", cluster.to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("EngineVersion", "8.0.mysql_aurora.3.05.2".to_string()),
                ("GlobalClusterIdentifier", global.to_string()),
            ],
        )
        .await,
    );

    let published = global_resource_id(&router, case, global).await;
    assert!(
        published.as_ref().is_some_and(|id| !id.is_empty()),
        "case {case:?}: the global database publishes a resource id of its own, got \
         {published:?}"
    );
    let joined = membership(&describe_one(&router, case, cluster).await);
    assert_eq!(
        joined.as_deref(),
        Some(global),
        "case {case:?}: a member names the global database it belongs to, got {joined:?}"
    );

    expect_ok(
        case,
        call(
            &router,
            "RemoveFromGlobalCluster",
            &[
                ("GlobalClusterIdentifier", global.to_string()),
                ("DbClusterIdentifier", db_cluster_arn(cluster)),
            ],
        )
        .await,
    );

    let detached = membership(&describe_one(&router, case, cluster).await);
    assert_eq!(
        detached, None,
        "case {case:?}: a detached cluster belongs to no global database, so it names none, \
         got {detached:?}"
    );

    // The global database itself still carries the id it always had: the detach changed the
    // membership, not the global database's identity.
    let still = global_resource_id(&router, case, global).await;
    assert_eq!(
        still, published,
        "case {case:?}: the global database's own resource id is immutable, got {still:?}"
    );
}

/// Two clusters in the SAME global database name one and the same global database, and a
/// cluster in another one names that other: the resource id each global database publishes
/// identifies the global database, not the cluster.
#[tokio::test]
async fn the_reported_id_identifies_the_global_database_and_not_the_cluster() {
    let router = emulator();
    let case = "two members and a second global database";
    let rows = [
        ("emu-gcrid-shared-a", "emu-gcrid-global-shared"),
        ("emu-gcrid-shared-b", "emu-gcrid-global-shared"),
        ("emu-gcrid-other", "emu-gcrid-global-other"),
    ];

    for global in ["emu-gcrid-global-shared", "emu-gcrid-global-other"] {
        expect_ok(
            case,
            call(
                &router,
                "CreateGlobalCluster",
                &[
                    ("GlobalClusterIdentifier", global.to_string()),
                    ("Engine", "aurora-mysql".to_string()),
                    ("EngineVersion", "8.0.mysql_aurora.3.05.2".to_string()),
                ],
            )
            .await,
        );
    }

    let mut reported = Vec::new();
    for (cluster, global) in rows {
        let created = the_cluster(
            case,
            &expect_ok(
                case,
                call(
                    &router,
                    "CreateDBCluster",
                    &[
                        ("DBClusterIdentifier", cluster.to_string()),
                        ("Engine", "aurora-mysql".to_string()),
                        ("EngineVersion", "8.0.mysql_aurora.3.05.2".to_string()),
                        ("GlobalClusterIdentifier", global.to_string()),
                    ],
                )
                .await,
            ),
        );
        assert_eq!(
            text(&created, "GlobalClusterIdentifier"),
            global,
            "case {case:?}: {cluster} must name the global database it was created into, got \
             {created}"
        );
        let published = global_resource_id(&router, case, global)
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: {global} must publish a resource id of its own")
            });
        reported.push((cluster, global, published));
    }

    assert_eq!(
        reported[0].2, reported[1].2,
        "case {case:?}: two members of one global database read one id, got {:?} and {:?}",
        reported[0], reported[1]
    );
    assert_ne!(
        reported[1].2, reported[2].2,
        "case {case:?}: members of two global databases read two ids, got {:?} and {:?}",
        reported[1], reported[2]
    );
}
