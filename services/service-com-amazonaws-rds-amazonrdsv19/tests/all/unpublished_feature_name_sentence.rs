// SPDX-License-Identifier: CC0-1.0

//! The SENTENCE an IAM role association naming an unpublished `FeatureName` is refused with.
//!
//! The rule under test: **`AddRoleToDBInstance` and `AddRoleToDBCluster` refuse a
//! `FeatureName` outside the `SupportedFeatureNames` of the addressed resource's engine
//! version with HTTP 400 `InvalidParameterValue` and ONE sentence — "The feature name
//! &lt;FeatureName&gt; is not valid for the engine &lt;engine&gt;." — whose fixed text ends in
//! the clause "for the engine", so the engine comes LAST, and whose variable parts are the
//! value the request carried, verbatim, and the `DBEngineDescription`
//! `DescribeDBEngineVersions` publishes for that engine; neither part depends on which
//! resource kind the association addressed, and the refusal records no association.**
//!
//! `FeatureName` is "the name of the feature for the DB cluster that the IAM role is to be
//! associated with. For information about supported feature names, see DBEngineVersion"
//! (`smithy_query(query="operation",operation="AddRoleToDBCluster")`), and the model gives the
//! member no enum and this refusal no modelled error, so the code is the Query API's common
//! `InvalidParameterValue` and the engine is named by the catalogue's own description — which
//! is why every row below reads that description back from `DescribeDBEngineVersions` instead
//! of trusting the name spelled beside it.
//!
//! The rows span what the rule generalises over: the engine (one whose published description
//! parenthesises the dialect while its product name does not, one whose description is its
//! product name, and three provisioned families whose descriptions carry an edition), the
//! RESOURCE the association is made on (a DB cluster and a DB instance), and the form of the
//! rejected value (a name no engine publishes, a name published for ANOTHER engine family, a
//! name published for this very engine written in another case, and a name published for a
//! sibling family).

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, expect_error, expect_ok, text, texts};
use crate::rds_s3_integration::{
    associated_roles, available_cluster, available_instance_of, emulator, role,
};

/// The service principal a role Amazon RDS acts under for a DB instance or DB cluster feature
/// must trust, so every row's role is one the service can take on and the graded refusal is
/// the FEATURE's rather than the role's.
const RDS_PRINCIPAL: &str = "rds.amazonaws.com";

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// Which resource an association is made on, and what creating one of that kind takes.
#[derive(Debug, Clone, Copy)]
enum Resource {
    /// A DB cluster, created from its engine alone.
    Cluster,
    /// A DB instance, created with the class and allocated storage its engine is offered
    /// with.
    Instance {
        class: &'static str,
        storage: u32,
    },
}

/// One row: the engine, the resource the association is made on, the `FeatureName` that
/// engine's versions do not publish, and the description the catalogue publishes for the
/// engine.
struct Row {
    case: &'static str,
    engine: &'static str,
    resource: Resource,
    feature: &'static str,
    /// The `DBEngineDescription` of the engine, spelled out here rather than taken from the
    /// code under test — and asserted against what `DescribeDBEngineVersions` publishes, so
    /// the sentence and the catalogue cannot drift apart.
    description: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "an Aurora PostgreSQL cluster, a feature name no engine publishes",
        engine: "aurora-postgresql",
        resource: Resource::Cluster,
        feature: "NotARealFeatureXYZ",
        description: "Aurora (PostgreSQL)",
    },
    Row {
        case: "an Aurora PostgreSQL cluster, a name it publishes written in another case",
        engine: "aurora-postgresql",
        resource: Resource::Cluster,
        feature: "s3import",
        description: "Aurora (PostgreSQL)",
    },
    Row {
        case: "an Aurora MySQL cluster, the S3 import feature of the other Aurora family",
        engine: "aurora-mysql",
        resource: Resource::Cluster,
        feature: "s3Import",
        description: "Aurora MySQL",
    },
    Row {
        case: "an RDS for PostgreSQL instance, the S3 integration of the Oracle engines",
        engine: "postgres",
        resource: Resource::Instance {
            class: "db.t3.micro",
            storage: 20,
        },
        feature: "S3_INTEGRATION",
        description: "PostgreSQL",
    },
    Row {
        case: "an RDS for MySQL instance, which publishes no role feature at all",
        engine: "mysql",
        resource: Resource::Instance {
            class: "db.t3.micro",
            storage: 20,
        },
        feature: "s3Import",
        description: "MySQL Community Edition",
    },
    Row {
        case: "an RDS for Oracle instance, the S3 import of the PostgreSQL engines",
        engine: "oracle-se2",
        resource: Resource::Instance {
            class: "db.m5.xlarge",
            storage: 40,
        },
        feature: "s3Import",
        description: "Oracle Database Standard Edition Two",
    },
    Row {
        case: "an RDS for SQL Server instance, the Lambda invoke of the PostgreSQL engines",
        engine: "sqlserver-ex",
        resource: Resource::Instance {
            class: "db.t3.small",
            storage: 30,
        },
        feature: "Lambda",
        description: "Microsoft SQL Server Express Edition",
    },
];

/// The `DBEngineDescription` `DescribeDBEngineVersions` publishes for `engine` — the one name
/// of the engine a client has already read, and the one the refusal has to carry.
///
/// Every version of an engine reports the same description, so the distinct values are
/// asserted to be exactly one and that one is answered.
async fn published_description(router: &MultiServiceRouter, case: &str, engine: &str) -> String {
    let described = expect_ok(
        case,
        call(
            router,
            "DescribeDBEngineVersions",
            &params!["Engine" => engine],
        )
        .await,
    );
    let mut published = texts(&described, "DBEngineDescription");
    published.sort();
    published.dedup();
    assert_eq!(
        published.len(),
        1,
        "case {case:?}: the engine {engine} is published under one description, got \
         {published:?}"
    );
    published.into_iter().next().expect("checked just above")
}

/// The sentence the refusal carries, spelled out: the fixed text, the value the request
/// carried and the engine's published description, which the clause "for the engine"
/// introduces LAST.
fn sentence(feature: &str, description: &str) -> String {
    format!("The feature name {feature} is not valid for the engine {description}.")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unpublished_feature_name_is_refused_naming_the_engine_last() {
    let (router, _engine) = emulator();

    for (index, row) in ROWS.iter().enumerate() {
        let identifier = format!("emu-ufn-{index}");
        let (action, identifier_member, describe) = match row.resource {
            Resource::Cluster => {
                available_cluster(&router, &identifier, row.engine).await;
                (
                    "AddRoleToDBCluster",
                    "DBClusterIdentifier",
                    "DescribeDBClusters",
                )
            }
            Resource::Instance { class, storage } => {
                available_instance_of(&router, &identifier, row.engine, class, storage, None).await;
                (
                    "AddRoleToDBInstance",
                    "DBInstanceIdentifier",
                    "DescribeDBInstances",
                )
            }
        };
        let role_arn = role(&router, &format!("emu-ufn-role-{index}"), RDS_PRINCIPAL, None).await;

        // The engine's published description is read back from the catalogue the member's own
        // documentation points at, so the sentence names the engine by the name a client that
        // read that catalogue has already seen.
        let published = published_description(&router, row.case, row.engine).await;
        assert_eq!(
            published, row.description,
            "case {:?}: expected the published description {:?}, got {published:?}",
            row.case, row.description
        );

        let body = expect_error(
            row.case,
            call(
                &router,
                action,
                &params![
                    identifier_member => identifier,
                    "RoleArn" => role_arn,
                    "FeatureName" => row.feature,
                ],
            )
            .await,
            400,
            "InvalidParameterValue",
        );
        let message = text(&body, "Message");
        let expected = sentence(row.feature, &published);
        assert_eq!(
            message, expected,
            "case {:?}: expected the sentence {expected:?}, got {message:?}",
            row.case
        );

        // And the refusal records nothing: the resource reports no association at all.
        let described = expect_ok(
            row.case,
            call(
                &router,
                describe,
                &params![identifier_member => identifier],
            )
            .await,
        );
        let reported = associated_roles(&described);
        assert!(
            reported.is_empty(),
            "case {:?}: a refused association records nothing, got {reported:?}",
            row.case
        );
    }
}
