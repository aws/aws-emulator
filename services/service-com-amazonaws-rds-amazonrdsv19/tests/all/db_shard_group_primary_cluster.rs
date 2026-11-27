// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the PRIMARY DB CLUSTER a `CreateDBShardGroup` names: how the
//! create resolves it, and which of that cluster's states the create can act on.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper
//! this test target has, as a real awsQuery form POST carrying the SigV4 credential scope
//! the router keys on — so a handler that computed nothing could not pass these
//! assertions.
//!
//! The rule under test: `DBClusterIdentifier` is "the name of the primary DB cluster for
//! the DB shard group", resolved in the region the request is signed for in either form
//! that member accepts and without regard to case, and a create is served for a cluster in
//! any state it can act on — a provisioning status included — while a cluster whose delete
//! has begun is the modelled state fault and a cluster the delete has already taken out is
//! the modelled cluster not-found, neither of which leaves a DB shard group behind.
//!
//! The table below spans what the rule generalises over: the state the cluster is in when
//! the create arrives (still provisioning, settled, its delete running, and gone), and each
//! form the create may name it by (its own spelling, another case, and its ARN).

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_error, expect_ok, text, wait_for_settle, REGION};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// The engine the primary cluster runs: Aurora Limitless Database is an Aurora PostgreSQL
/// feature, and `CreateDBShardGroup` is "valid for: Aurora DB clusters only".
const CLUSTER_ENGINE: &str = "aurora-postgresql";

/// The `MaxACU` every create below names — "the maximum capacity of the DB shard group in
/// Aurora capacity units (ACUs)", which the operation requires.
const MAX_ACU: &str = "100";

/// The status a DB shard group reports from an accepted create until the transition this
/// service owes it comes due.
const CREATING: &str = "creating";

/// The status it reports once that transition has settled.
const AVAILABLE: &str = "available";

/// What the primary cluster is doing when the create arrives, and how the test gets it
/// there.
#[derive(Debug, Clone, Copy)]
enum ClusterState {
    /// Straight after `CreateDBCluster`, before the clock has settled the cluster.
    Creating,
    /// After the create has settled.
    Available,
    /// After a delete has been accepted for it, inside the span the clock owes.
    Deleting,
    /// After that delete has run, so the identifier names nothing.
    Gone,
}

/// How the create NAMES the primary cluster: `DBClusterIdentifier` is "the user-supplied
/// DB cluster identifier or the Amazon Resource Name (ARN) of the DB cluster", and "this
/// parameter isn't case-sensitive".
#[derive(Debug, Clone, Copy)]
enum NamedBy {
    /// The identifier exactly as the create of the cluster spelled it.
    AsCreated,
    /// The identifier in another case.
    OtherCase,
    /// The cluster's ARN.
    Arn,
}

/// What the create is answered with.
#[derive(Debug, Clone, Copy)]
enum Expected {
    /// HTTP 200, reporting the DB shard group being provisioned on that cluster.
    Served,
    /// The modelled fault: its HTTP status and its wire error `<Code>`.
    Refused(u16, &'static str),
}

/// One row of the primary-cluster table.
struct PrimaryClusterCase {
    case: &'static str,
    /// The `DBClusterIdentifier` the cluster is created with, which is stored lowercased.
    cluster: &'static str,
    /// The `DBShardGroupIdentifier` the create names.
    shard_group: &'static str,
    state: ClusterState,
    named_by: NamedBy,
    expected: Expected,
}

/// `CreateDBCluster` for the primary cluster of one row.
///
/// `ClusterScalabilityType` is `limitless` and the engine version is the one Aurora Limitless
/// Database is offered on: a DB shard group holds the shards of a Limitless database, so only
/// a cluster created that way can take one.
async fn create_cluster(router: &MultiServiceRouter, identifier: &str) -> (u16, String) {
    let engine_version =
        crate::instance_reference_resolution_refusals::limitless_engine_version(router).await;
    call(
        router,
        "CreateDBCluster",
        &[
            ("DBClusterIdentifier", identifier.to_string()),
            ("Engine", CLUSTER_ENGINE.to_string()),
            ("ClusterScalabilityType", "limitless".to_string()),
            ("EngineVersion", engine_version),
        ],
    )
    .await
}

/// `DeleteDBCluster` for that cluster, skipping the final snapshot.
async fn delete_cluster(router: &MultiServiceRouter, identifier: &str) -> (u16, String) {
    call(
        router,
        "DeleteDBCluster",
        &[
            ("DBClusterIdentifier", identifier),
            ("SkipFinalSnapshot", "true"),
        ],
    )
    .await
}

/// `CreateDBShardGroup` naming the primary cluster as `named_cluster` spells it.
async fn create_shard_group(
    router: &MultiServiceRouter,
    shard_group: &str,
    named_cluster: &str,
) -> (u16, String) {
    call(
        router,
        "CreateDBShardGroup",
        &[
            ("DBShardGroupIdentifier", shard_group),
            ("DBClusterIdentifier", named_cluster),
            ("MaxACU", MAX_ACU),
        ],
    )
    .await
}

/// `DescribeDBShardGroups` for one identifier.
async fn describe_shard_group(router: &MultiServiceRouter, shard_group: &str) -> (u16, String) {
    call(
        router,
        "DescribeDBShardGroups",
        &[("DBShardGroupIdentifier", shard_group)],
    )
    .await
}

#[tokio::test]
async fn a_create_resolves_its_primary_cluster_and_acts_only_on_a_state_it_can() {
    let table: &[PrimaryClusterCase] = &[
        PrimaryClusterCase {
            case: "a cluster still being provisioned, named as created",
            cluster: "emu-sgpc-creating",
            shard_group: "emu-sgpc-on-creating",
            state: ClusterState::Creating,
            named_by: NamedBy::AsCreated,
            expected: Expected::Served,
        },
        PrimaryClusterCase {
            case: "an available cluster, named in another case",
            cluster: "emu-sgpc-available",
            shard_group: "emu-sgpc-on-available",
            state: ClusterState::Available,
            named_by: NamedBy::OtherCase,
            expected: Expected::Served,
        },
        PrimaryClusterCase {
            case: "an available cluster, named by its ARN",
            cluster: "emu-sgpc-by-arn",
            shard_group: "emu-sgpc-on-by-arn",
            state: ClusterState::Available,
            named_by: NamedBy::Arn,
            expected: Expected::Served,
        },
        PrimaryClusterCase {
            case: "a cluster whose delete has begun, named as created",
            cluster: "emu-sgpc-deleting",
            shard_group: "emu-sgpc-on-deleting",
            state: ClusterState::Deleting,
            named_by: NamedBy::AsCreated,
            expected: Expected::Refused(400, "InvalidDBClusterStateFault"),
        },
        PrimaryClusterCase {
            case: "a cluster whose delete has begun, named by its ARN",
            cluster: "emu-sgpc-deleting-arn",
            shard_group: "emu-sgpc-on-deleting-arn",
            state: ClusterState::Deleting,
            named_by: NamedBy::Arn,
            expected: Expected::Refused(400, "InvalidDBClusterStateFault"),
        },
        PrimaryClusterCase {
            case: "a cluster the delete has already taken out",
            cluster: "emu-sgpc-gone",
            shard_group: "emu-sgpc-on-gone",
            state: ClusterState::Gone,
            named_by: NamedBy::AsCreated,
            expected: Expected::Refused(404, "DBClusterNotFoundFault"),
        },
    ];

    for row in table {
        // Each row gets its own emulator, so the timing of one row's transitions cannot
        // carry into the next.
        let router = emulator();
        expect_ok(row.case, create_cluster(&router, row.cluster).await);
        match row.state {
            ClusterState::Creating => {}
            ClusterState::Available => wait_for_settle(),
            ClusterState::Deleting => {
                wait_for_settle();
                expect_ok(row.case, delete_cluster(&router, row.cluster).await);
            }
            ClusterState::Gone => {
                wait_for_settle();
                expect_ok(row.case, delete_cluster(&router, row.cluster).await);
                wait_for_settle();
            }
        }

        let named = match row.named_by {
            NamedBy::AsCreated => row.cluster.to_string(),
            NamedBy::OtherCase => row.cluster.to_ascii_uppercase(),
            NamedBy::Arn => format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{}", row.cluster),
        };
        let answer = create_shard_group(&router, row.shard_group, &named).await;

        match row.expected {
            Expected::Served => {
                let created = expect_ok(row.case, answer);
                // The shard group reports the cluster the create RESOLVED — the stored
                // identifier — whichever form the request named it by.
                assert_eq!(
                    text(&created, "DBClusterIdentifier"),
                    row.cluster,
                    "case {:?}: expected the resolved primary cluster, got {created}",
                    row.case
                );
                assert_eq!(
                    text(&created, "Status"),
                    CREATING,
                    "case {:?}: an accepted create must report {CREATING:?}, got {created}",
                    row.case
                );
                // And the record was PERSISTED: the read after the transition this
                // service owes has come due reports the same primary cluster.
                wait_for_settle();
                let described =
                    expect_ok(row.case, describe_shard_group(&router, row.shard_group).await);
                assert_eq!(
                    text(&described, "DBClusterIdentifier"),
                    row.cluster,
                    "case {:?}: the stored shard group must keep its primary cluster, got \
                     {described}",
                    row.case
                );
                assert_eq!(
                    text(&described, "Status"),
                    AVAILABLE,
                    "case {:?}: expected the status the clock has made due, got {described}",
                    row.case
                );
            }
            Expected::Refused(status, code) => {
                let refused = expect_error(row.case, answer, status, code);
                let message = text(&refused, "Message");
                // The state fault names the cluster whose state refused the create, while
                // the not-found fault for a cluster this call cannot resolve carries the
                // one fixed sentence that names nothing — so each is asserted as the
                // service words it.
                if code == "DBClusterNotFoundFault" {
                    assert_eq!(
                        message,
                        "The DB cluster doesn't exist.",
                        "case {:?}: expected the fixed not-found sentence, got {message:?}",
                        row.case
                    );
                } else {
                    assert!(
                        message.contains(row.cluster),
                        "case {:?}: the refusal must name the cluster, got {message:?}",
                        row.case
                    );
                }
                // A refused create leaves nothing behind: the identifier it named is
                // still one nothing created.
                expect_error(
                    row.case,
                    describe_shard_group(&router, row.shard_group).await,
                    404,
                    "DBShardGroupNotFound",
                );
            }
        }
    }
}
