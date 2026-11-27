// SPDX-License-Identifier: CC0-1.0

//! The ORDER an IAM role association is accepted in: the role before the feature.
//!
//! The rule this suite pins: **`AddRoleToDBInstance` and `AddRoleToDBCluster` decide the ROLE
//! first — a `RoleArn` that names no role this account's IAM holds, or one whose trust policy
//! does not let `rds.amazonaws.com` assume it, is refused with `InvalidParameterValue` and the
//! one fixed sentence Amazon RDS words a role integration it cannot perform in — and only a
//! role the service can take on is then held to the `SupportedFeatureNames` its engine
//! version publishes.** So a request carrying BOTH defects is refused for the role, and
//! neither refusal records an association.
//!
//! WHICH sentence the unpublished `FeatureName` itself carries is one sentence for both: it
//! names the value the request carried and the ENGINE it was rejected for — the engine is what
//! decides whether the feature exists — under the DESCRIPTION that engine is published with,
//! whichever resource the association addressed.
//!
//! IAM is mounted beside this service on ONE engine, exactly as the assembled router mounts
//! them, so the role each row names is a role that service really holds (or really does not)
//! and the trust policy read is the one it stored.
//!
//! The rows span what the rule generalises over: which half of the request is unusable (the
//! role's existence, the role's trust, the feature name, both), and they are asserted for BOTH
//! resource kinds — a DB instance and a DB cluster — because the rule is one rule and the two
//! operations are accepted against it.

use service_com_amazonaws_rds_amazonrdsv19::state as st;

use crate::db_instances::{call, expect_error, expect_ok, text};
use crate::rds_s3_integration::{
    associated_roles, available_cluster, available_instance, emulator, role,
};

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// A principal of another service, which a role Amazon RDS acts under must not accept in
/// place of its own.
const OTHER_PRINCIPAL: &str = "ec2.amazonaws.com";

/// An ARN of the modelled form naming a role IAM holds nothing under.
fn absent_role_arn(name: &str) -> String {
    format!("arn:aws:iam::000000000000:role/{name}")
}

/// Which half of the request a row makes unusable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Defect {
    /// The value names no role this account's IAM holds.
    NoSuchRole,
    /// IAM holds the role, and its trust policy names another service's principal.
    MistrustedRole,
    /// The role is one the service can take on; the feature name is outside what the
    /// resource's engine version publishes.
    UnpublishedFeature,
    /// Both: no such role, and a feature name the engine does not publish.
    NoSuchRoleAndUnpublishedFeature,
    /// Nothing: the association is the one that holds.
    None,
}

/// One row of the table: the case, the defect it carries, and the feature name it names.
struct Row {
    case: &'static str,
    suffix: &'static str,
    defect: Defect,
}

const ROWS: &[Row] = &[
    Row {
        case: "a role ARN IAM holds nothing under",
        suffix: "absent",
        defect: Defect::NoSuchRole,
    },
    Row {
        case: "a role whose trust policy names another service's principal",
        suffix: "mistrusted",
        defect: Defect::MistrustedRole,
    },
    Row {
        case: "a feature the engine version does not publish",
        suffix: "feature",
        defect: Defect::UnpublishedFeature,
    },
    Row {
        case: "no such role AND a feature the engine version does not publish",
        suffix: "both",
        defect: Defect::NoSuchRoleAndUnpublishedFeature,
    },
    Row {
        case: "a role the service can assume, for a feature the engine publishes",
        suffix: "held",
        defect: Defect::None,
    },
];

/// One resource kind the rule holds for: how the association names the resource, which engine
/// it runs, the feature name that engine publishes and one it does not.
struct Kind {
    kind: &'static str,
    engine: &'static str,
    identifier_member: &'static str,
    associate: &'static str,
    describe: &'static str,
    /// A `FeatureName` the engine version's `SupportedFeatureNames` carries — the list
    /// `DescribeDBEngineVersions` publishes, which is where a client reads it.
    published_feature: &'static str,
    /// A `FeatureName` it does not: `S3_INTEGRATION` is the Oracle and Microsoft SQL Server
    /// name, which no PostgreSQL version publishes.
    unpublished_feature: &'static str,
    /// The `DBEngineDescription` the engine catalogue publishes for this kind's engine, which
    /// is the name the unpublished-feature refusal calls the engine by — spelled out here
    /// rather than taken from the code under test, so this suite pins the wording too.
    engine_description: &'static str,
}

/// The sentence an unpublished `FeatureName` is refused with: one sentence for both resource
/// kinds, naming the value the request carried and then the engine, last, after "for the
/// engine".
fn feature_refusal(description: &str, feature: &str) -> String {
    format!("The feature name {feature} is not valid for the engine {description}.")
}

const KINDS: &[Kind] = &[
    Kind {
        kind: "DB instance",
        engine: "postgres",
        identifier_member: "DBInstanceIdentifier",
        associate: "AddRoleToDBInstance",
        describe: "DescribeDBInstances",
        published_feature: "s3Import",
        unpublished_feature: "S3_INTEGRATION",
        engine_description: "PostgreSQL",
    },
    Kind {
        kind: "DB cluster",
        engine: "aurora-postgresql",
        identifier_member: "DBClusterIdentifier",
        associate: "AddRoleToDBCluster",
        describe: "DescribeDBClusters",
        published_feature: "s3Export",
        unpublished_feature: "S3_INTEGRATION",
        engine_description: "Aurora (PostgreSQL)",
    },
];

/// Every row, for both resource kinds: the refusal names the ROLE wherever the role is
/// unusable, the feature only where the role is not, and no refusal records an association.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_role_the_service_cannot_assume_is_refused_before_the_feature_name() {
    for kind in KINDS {
        for row in ROWS {
            let case = format!("{}, {}", kind.kind, row.case);
            let (router, _engine) = emulator();
            let identifier = format!("emu-role-order-{}-{}", kind.associate.len(), row.suffix);
            match kind.kind {
                "DB instance" => available_instance(&router, &identifier, kind.engine).await,
                _ => available_cluster(&router, &identifier, kind.engine).await,
            }

            let role_arn = match row.defect {
                Defect::NoSuchRole | Defect::NoSuchRoleAndUnpublishedFeature => {
                    absent_role_arn(&format!("{identifier}-nowhere"))
                }
                Defect::MistrustedRole => {
                    role(&router, &format!("{identifier}-role"), OTHER_PRINCIPAL, None).await
                }
                Defect::UnpublishedFeature | Defect::None => {
                    role(
                        &router,
                        &format!("{identifier}-role"),
                        st::RDS_SERVICE_PRINCIPAL,
                        None,
                    )
                    .await
                }
            };
            let feature_name = match row.defect {
                Defect::UnpublishedFeature | Defect::NoSuchRoleAndUnpublishedFeature => {
                    kind.unpublished_feature
                }
                _ => kind.published_feature,
            };
            let request = params![
                kind.identifier_member => identifier,
                "RoleArn" => role_arn,
                "FeatureName" => feature_name,
            ]
            .to_vec();
            let answered = call(&router, kind.associate, &request).await;

            // The sentence each defect carries: the ROLE's wherever the role is unusable —
            // including the row that carries both defects, which is what pins the order — and
            // the FEATURE's only where the role itself is one the service can take on.
            let expected_sentence = match row.defect {
                Defect::NoSuchRole
                | Defect::MistrustedRole
                | Defect::NoSuchRoleAndUnpublishedFeature => {
                    Some(st::ROLE_NOT_TRUSTED_MESSAGE.to_string())
                }
                Defect::UnpublishedFeature => {
                    Some(feature_refusal(kind.engine_description, feature_name))
                }
                Defect::None => None,
            };
            match expected_sentence {
                Some(expected) => {
                    let body = expect_error(&case, answered, 400, "InvalidParameterValue");
                    let message = text(&body, "Message");
                    assert_eq!(
                        message, expected,
                        "case {case:?}: expected the sentence {expected:?}, got {message:?}"
                    );
                }
                None => {
                    expect_ok(&case, answered);
                }
            }

            // What the resource reports afterwards: the association the one accepted row made,
            // and nothing at all for every refused one — a refusal records no association.
            let described = expect_ok(
                &case,
                call(
                    &router,
                    kind.describe,
                    &params![kind.identifier_member => identifier],
                )
                .await,
            );
            let reported = associated_roles(&described);
            let wanted: Vec<(String, String, String)> = match row.defect {
                Defect::None => vec![(
                    role_arn.clone(),
                    feature_name.to_string(),
                    st::DB_CLUSTER_ROLE_STATUS_ACTIVE.to_string(),
                )],
                _ => Vec::new(),
            };
            assert_eq!(
                reported, wanted,
                "case {case:?}: expected the associations {wanted:?}, got {reported:?}"
            );
        }
    }
}
