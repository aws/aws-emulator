// SPDX-License-Identifier: CC0-1.0

//! The `FeatureName` of a DB CLUSTER's IAM role association, at EVERY call that records one.
//!
//! The rule under test: **a role association on a DB cluster may name only a `FeatureName` the
//! cluster's engine version publishes in `DBEngineVersion$SupportedFeatureNames`, and any other
//! value is refused with `InvalidParameterValue` and a sentence naming the value and that
//! engine — whichever call records the association.** `FeatureName` is "the name of the feature
//! for the DB cluster that the IAM role is to be associated with. For information about
//! supported feature names, see DBEngineVersion"
//! (`smithy_query(query="operation",operation="AddRoleToDBCluster")`), so the catalogue
//! `DescribeDBEngineVersions` publishes is the domain of the member — and the `AssociatedRoles`
//! a cluster is CREATED or RESTORED with are the same associations that call makes, entry by
//! entry, so the same domain holds for them. An entry naming NO feature associates the role
//! with the cluster itself, which no catalogue entry gates.
//!
//! A refusal leaves nothing behind: a create or restore AWS would never have accepted leaves no
//! cluster for a later call to find, which is what the read-back here asserts.
//!
//! The rows span what the rule generalises over: the engine (Aurora MySQL, which publishes the
//! machine-learning features and NO S3 feature, and Aurora PostgreSQL, which publishes the S3
//! ones too), the feature name (one the engine publishes, one it does not, one published for
//! ANOTHER engine family, one published name written in another case, and none at all), and the
//! ENTRY the association is recorded through: `CreateDBCluster`'s inline `AssociatedRoles`,
//! `RestoreDBClusterFromSnapshot`'s, `RestoreDBClusterToPointInTime`'s and the
//! `AddRoleToDBCluster` call each of them carries an entry of.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, expect_error, expect_ok, text};
use crate::lifecycle_poll_loops::poll_until;
use crate::rds_s3_integration::{associated_roles, emulator, role};

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// The master user every cluster below is created with — test credentials, and the only place
/// in this suite they appear.
const MASTER_USER: &str = "emuadmin";
const MASTER_PASSWORD: &str = "emu-Secret-77";

/// The service principal a role Amazon RDS acts under for a DB cluster feature must trust.
const RDS_PRINCIPAL: &str = "rds.amazonaws.com";

/// The `Status` an association that holds reports.
const ACTIVE: &str = "ACTIVE";

/// One engine of the table, with the name a refusal calls it by.
struct Engine {
    /// `Engine`: the family the cluster runs.
    engine: &'static str,
    /// The product name the published engine catalogue reports for that value
    /// (`DBEngineVersion$DBEngineDescription`'s sibling, which `DescribeDBEngineVersions`
    /// answers with), which is the name the cluster's refusal puts in front of "engine".
    product: &'static str,
    /// The identifier fragment resources of this engine are named under, since the engine's
    /// own value carries hyphens.
    tag: &'static str,
}

const ENGINES: &[Engine] = &[
    Engine {
        engine: "aurora-mysql",
        product: "Aurora MySQL",
        tag: "my",
    },
    Engine {
        engine: "aurora-postgresql",
        product: "Aurora PostgreSQL",
        tag: "pg",
    },
];

/// One feature-name row: the value an association names, and whether the engine of the row it
/// is run for publishes it.
struct Feature {
    case: &'static str,
    /// The `FeatureName` the association names — `None` for the cluster-wide association, which
    /// names none.
    feature: Option<&'static str>,
    /// Which engines of [`ENGINES`] publish it, by `Engine::engine`.
    published_by: &'static [&'static str],
}

/// The feature-name table. Source for every value: the `SupportedFeatureNames` this service
/// publishes through `DescribeDBEngineVersions` — the Aurora MySQL versions publish the three
/// Aurora machine-learning integrations and no S3 feature, the Aurora PostgreSQL versions those
/// three plus the `aws_s3` extension's import and export and `aws_lambda`'s invoke, and
/// `S3_INTEGRATION` is published by the Oracle and Microsoft SQL Server engines alone.
const FEATURES: &[Feature] = &[
    Feature {
        case: "the S3 import feature",
        feature: Some("s3Import"),
        published_by: &["aurora-postgresql"],
    },
    Feature {
        case: "the S3 export feature",
        feature: Some("s3Export"),
        published_by: &["aurora-postgresql"],
    },
    Feature {
        case: "a machine-learning integration both families publish",
        feature: Some("SageMaker"),
        published_by: &["aurora-mysql", "aurora-postgresql"],
    },
    Feature {
        case: "a feature name published for the Oracle engines alone",
        feature: Some("S3_INTEGRATION"),
        published_by: &[],
    },
    Feature {
        case: "a published name written in another case",
        feature: Some("s3import"),
        published_by: &[],
    },
    Feature {
        case: "no feature name at all, the cluster-wide association",
        feature: None,
        published_by: &["aurora-mysql", "aurora-postgresql"],
    },
];

/// Whether `engine` publishes the row's feature name.
fn publishes(row: &Feature, engine: &str) -> bool {
    row.feature.is_none() || row.published_by.contains(&engine)
}

/// The sentence a DB cluster's association naming a feature its engine does not publish is
/// refused with: the fixed text, the value the request carried and the engine's product name.
fn refusal(product: &str, feature: &str) -> String {
    format!("The feature name {feature} is not valid for the {product} engine.")
}

/// Create the source cluster `identifier` of `engine`, answering once it reports `available` —
/// the state a snapshot is taken in, a point-in-time restore is made from, and an association
/// is added to.
///
/// The CONDITION is polled for, never a span: the transition is owed on this service's own
/// clock, and the loop this test target shares has a deadline generous beside it.
async fn available_cluster(router: &MultiServiceRouter, case: &str, identifier: &str, engine: &str) {
    expect_ok(
        case,
        call(
            router,
            "CreateDBCluster",
            &params![
                "DBClusterIdentifier" => identifier,
                "Engine" => engine,
                "MasterUsername" => MASTER_USER,
                "MasterUserPassword" => MASTER_PASSWORD,
            ],
        )
        .await,
    );
    let read = [("DBClusterIdentifier", identifier)];
    poll_until(
        case,
        "the source cluster",
        || call(router, "DescribeDBClusters", &read),
        |status, body| status == 200 && text(body, "Status") == "available",
    )
    .await;
}

/// Take the cluster snapshot `snapshot` of `cluster`, answering once it reports `available` —
/// the state a restore reads it in. Polled for the same reason as above.
async fn available_snapshot(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    snapshot: &str,
) {
    expect_ok(
        case,
        call(
            router,
            "CreateDBClusterSnapshot",
            &params![
                "DBClusterIdentifier" => cluster,
                "DBClusterSnapshotIdentifier" => snapshot,
            ],
        )
        .await,
    );
    let read = [("DBClusterSnapshotIdentifier", snapshot)];
    poll_until(
        case,
        "the cluster snapshot",
        || call(router, "DescribeDBClusterSnapshots", &read),
        |status, body| status == 200 && text(body, "Status") == "available",
    )
    .await;
}

/// WHICH call records the association, as the members it is asked for with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    /// `CreateDBCluster`'s inline `AssociatedRoles`: "a list of Amazon Web Services Identity
    /// and Access Management (IAM) roles to associate with the DB cluster".
    Create,
    /// `RestoreDBClusterFromSnapshot`'s list of the same shape.
    RestoreFromSnapshot,
    /// `RestoreDBClusterToPointInTime`'s.
    RestoreToPointInTime,
    /// `AddRoleToDBCluster`, the call one entry of such a list is made by.
    AddRole,
}

impl Entry {
    /// The action this entry calls.
    fn action(self) -> &'static str {
        match self {
            Self::Create => "CreateDBCluster",
            Self::RestoreFromSnapshot => "RestoreDBClusterFromSnapshot",
            Self::RestoreToPointInTime => "RestoreDBClusterToPointInTime",
            Self::AddRole => "AddRoleToDBCluster",
        }
    }

    /// Whether this entry CREATES the cluster the association is recorded on, so a refusal of
    /// it must leave no cluster behind.
    fn creates_the_cluster(self) -> bool {
        self != Self::AddRole
    }

    /// The identifier fragment resources of this entry are named under.
    fn tag(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::RestoreFromSnapshot => "snap",
            Self::RestoreToPointInTime => "pitr",
            Self::AddRole => "addrole",
        }
    }
}

/// Every entry, in the order this suite drives them.
const ENTRIES: &[Entry] = &[
    Entry::Create,
    Entry::RestoreFromSnapshot,
    Entry::RestoreToPointInTime,
    Entry::AddRole,
];

/// The source estate one engine's rows are run against: the cluster a point-in-time restore
/// reads and an association is added to, and the snapshot a restore reads.
struct Source {
    cluster: String,
    snapshot: String,
}

/// The parameters one entry's call carries for the association `(role_arn, feature)` on the
/// cluster `target`.
fn request(
    entry: Entry,
    source: &Source,
    engine: &str,
    target: &str,
    role_arn: &str,
    feature: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut members: Vec<(&'static str, String)> = match entry {
        Entry::Create => params![
            "DBClusterIdentifier" => target,
            "Engine" => engine,
            "MasterUsername" => MASTER_USER,
            "MasterUserPassword" => MASTER_PASSWORD,
            "AssociatedRoles.DBClusterAssociatedRole.1.RoleArn" => role_arn,
        ]
        .to_vec(),
        Entry::RestoreFromSnapshot => params![
            "DBClusterIdentifier" => target,
            "SnapshotIdentifier" => source.snapshot,
            "Engine" => engine,
            "AssociatedRoles.DBClusterAssociatedRole.1.RoleArn" => role_arn,
        ]
        .to_vec(),
        Entry::RestoreToPointInTime => params![
            "DBClusterIdentifier" => target,
            "SourceDBClusterIdentifier" => source.cluster,
            "UseLatestRestorableTime" => true,
            "AssociatedRoles.DBClusterAssociatedRole.1.RoleArn" => role_arn,
        ]
        .to_vec(),
        Entry::AddRole => params![
            "DBClusterIdentifier" => source.cluster,
            "RoleArn" => role_arn,
        ]
        .to_vec(),
    };
    if let Some(feature) = feature {
        let member = match entry {
            Entry::AddRole => "FeatureName",
            _ => "AssociatedRoles.DBClusterAssociatedRole.1.FeatureName",
        };
        members.push((member, feature.to_string()));
    }
    members
}

/// The `(RoleArn, FeatureName, Status)` triples the cluster `identifier` reports now.
async fn reported_roles(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> Vec<(String, String, String)> {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBClusters",
            &[("DBClusterIdentifier", identifier)],
        )
        .await,
    );
    associated_roles(&body)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_call_that_records_a_cluster_role_holds_the_feature_name_to_the_engine() {
    let (router, _engine) = emulator();
    // The estate every row of one engine is run against, stood up once: the creates are made
    // for both engines before either is waited for, so the transitions they owe run side by
    // side.
    let mut sources: Vec<(&Engine, Source)> = Vec::new();
    for engine in ENGINES {
        let source = Source {
            cluster: format!("emu-crfn-src-{}", engine.tag),
            snapshot: format!("emu-crfn-snap-{}", engine.tag),
        };
        available_cluster(&router, engine.engine, &source.cluster, engine.engine).await;
        available_snapshot(
            &router,
            engine.engine,
            &source.cluster,
            &source.snapshot,
        )
        .await;
        sources.push((engine, source));
    }

    for (engine, source) in &sources {
        for (index, row) in FEATURES.iter().enumerate() {
            let published = publishes(row, engine.engine);
            for entry in ENTRIES {
                let case = format!(
                    "{}, {} ({})",
                    engine.product,
                    row.case,
                    entry.action()
                );
                // One role per case: a DB cluster takes one association per ROLE, so the rows
                // an `AddRoleToDBCluster` accepts need a role of their own.
                let role_name = format!("emu-crfn-{}-{index}-{}", engine.tag, entry.tag());
                let role_arn = role(&router, &role_name, RDS_PRINCIPAL, None).await;
                let target = format!("emu-crfn-{}-{index}-{}", engine.tag, entry.tag());
                let members = request(
                    *entry,
                    source,
                    engine.engine,
                    &target,
                    &role_arn,
                    row.feature,
                );
                let answered = call(&router, entry.action(), &members).await;

                if !published {
                    let body = expect_error(&case, answered, 400, "InvalidParameterValue");
                    let message = text(&body, "Message");
                    let expected = refusal(
                        engine.product,
                        row.feature.expect("an unpublished row names a feature"),
                    );
                    assert_eq!(
                        message, expected,
                        "case {case:?}: expected the sentence {expected:?}, got {message:?}"
                    );
                    if entry.creates_the_cluster() {
                        // The cluster is never created, so the identifier the refused call named
                        // resolves to nothing.
                        let (status, body) = call(
                            &router,
                            "DescribeDBClusters",
                            &[("DBClusterIdentifier", target.as_str())],
                        )
                        .await;
                        assert_eq!(
                            status, 404,
                            "case {case:?}: a refused call leaves no cluster behind, got \
                             {status} with {body}"
                        );
                        assert_eq!(
                            text(&body, "Message"),
                            format!("DBCluster {target} not found."),
                            "case {case:?}: expected the cluster not-found sentence, got {body}"
                        );
                    } else {
                        // And an association the cluster already holds is untouched: the role
                        // this case named is not among them.
                        let held = reported_roles(&router, &case, &source.cluster).await;
                        assert!(
                            !held.iter().any(|(arn, _, _)| arn == &role_arn),
                            "case {case:?}: a refused association records nothing, got {held:?}"
                        );
                    }
                    continue;
                }

                expect_ok(&case, answered);
                // The accepted association is REPORTED on the cluster that holds it, read back
                // through the describe: in place from the call onwards, which is the `ACTIVE`
                // status an association that holds carries.
                let holder = match entry.creates_the_cluster() {
                    true => target.as_str(),
                    false => source.cluster.as_str(),
                };
                let held = reported_roles(&router, &case, holder).await;
                let wanted = (
                    role_arn.clone(),
                    row.feature.unwrap_or_default().to_string(),
                    ACTIVE.to_string(),
                );
                assert!(
                    held.contains(&wanted),
                    "case {case:?}: expected the association {wanted:?} on {holder}, got {held:?}"
                );
            }
        }
    }
}
