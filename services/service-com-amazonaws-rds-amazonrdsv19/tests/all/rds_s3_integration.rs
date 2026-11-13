// SPDX-License-Identifier: CC0-1.0

//! The Amazon RDS ↔ Amazon S3 integration surface: the IAM roles a DB instance or DB
//! cluster is associated with, and the snapshot and cluster EXPORT those roles gate.
//!
//! The rules these pin:
//!
//! * `AddRoleToDBInstance` / `AddRoleToDBCluster` record an association on the addressed
//!   resource and `RemoveRoleFromDBInstance` / `RemoveRoleFromDBCluster` drop it, so
//!   `DBInstance$AssociatedRoles` / `DBCluster$AssociatedRoles` report exactly the
//!   associations that were made. A `FeatureName` outside the `SupportedFeatureNames` of
//!   the resource's ENGINE VERSION, and a role whose stored trust policy does not let
//!   `rds.amazonaws.com` assume it, are refused with `InvalidParameterValue`; an
//!   association already in place is refused by the rule that holds for that resource — a
//!   DB instance's engine takes one role ARN per feature name and a DB cluster takes one
//!   association per role (`iam_role_association_refusals` beside this suite pins both
//!   refusals' codes and sentences) — and removing one that was never made is the modelled
//!   not-found fault.
//! * `StartExportTask` EXPORTS the source's data: it writes one Apache Parquet object per
//!   table of the source database plus the JSON file that says what was exported, into the
//!   destination bucket, and records a task that walks STARTING → IN_PROGRESS → COMPLETE.
//!   Its documented refusals are each answered with the code AWS answers them with: a
//!   bucket Amazon S3 holds nothing under is `InvalidS3BucketFault`, a bucket in another
//!   Region is `InvalidParameterValue`, a role that does not trust
//!   `export.rds.amazonaws.com` is `IamRoleNotFoundFault`, a role without the object write
//!   — or one the bucket policy denies it to — is `IamRoleMissingPermissionsFault`, a KMS
//!   key KMS holds nothing under is `KMSKeyNotAccessibleFault`, an identifier already
//!   running is `ExportTaskAlreadyExistsFault`, an `ExportOnly` entry outside the published
//!   forms is `InvalidExportOnlyFault`, and a source this region holds nothing under is
//!   that source's own not-found fault. Each refusal also carries the SENTENCE the service
//!   words that fault as, with the request's own values in its variable parts.
//!
//! Each rule is asserted over a TABLE spanning what it generalises over: the feature names
//! of three engine families plus an engine that publishes none, both resource kinds
//! (instance and cluster), a prefix given and omitted, an `ExportOnly` list of zero, one and
//! two entries, and one row per documented refusal.
//!
//! Amazon S3, IAM and KMS are mounted beside this service on ONE engine, exactly as the
//! assembled router mounts them, because the export resolves its bucket, role and key
//! against them — and the objects the export wrote are read back through Amazon S3's OWN
//! `ListObjectsV2` and `GetObject`, so what this suite asserts exists is what that service
//! really holds. The exported Parquet is read with the Apache Parquet reader, so the bytes
//! are asserted to be a real file in that format carrying the source's own rows.

use std::sync::Arc;

use bytes::Bytes;
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::RowAccessor;
use serde_json::Value;
use service_com_amazonaws_rds_amazonrdsv19::state as st;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, expect_error, expect_ok, optional_text, subtrees, text, texts, wait_for_settle,
    REGION,
};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// A second region, for the row that names a bucket outside the export's own Region.
const OTHER_REGION: &str = "eu-west-1";

/// The master user every DB instance below is created with, and its password — test
/// credentials, and the only place in this suite they appear.
const MASTER_USER: &str = "emuadmin";
const MASTER_PASSWORD: &str = "emu-Secret-99";

/// The database the exported tables live in.
const DATABASE: &str = "appdb";

/// The trust policy of a role Amazon RDS may assume for a DB instance or DB cluster
/// feature: it grants `sts:AssumeRole` to the service principal that acts under it.
fn trust_policy(principal: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"Service":"{principal}"}},"Action":"sts:AssumeRole"}}]}}"#
    )
}

/// An identity policy granting the object write an export performs on `bucket`.
pub(crate) fn put_object_policy(bucket: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"s3:PutObject","Resource":"arn:aws:s3:::{bucket}/*"}}]}}"#
    )
}

/// A bucket policy DENYING `role_arn` the object write, which is the other half of the
/// permissions decision ("an explicit deny in any policy overrides any allow").
fn deny_policy(bucket: &str, role_arn: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Deny","Principal":{{"AWS":"{role_arn}"}},"Action":"s3:PutObject","Resource":"arn:aws:s3:::{bucket}/*"}}]}}"#
    )
}

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// A router carrying this service plus the three its RDS/S3 integration resolves against —
/// Amazon S3 (the destination bucket and the objects an export writes), IAM (the role) and
/// KMS (the key) — all over ONE shared engine.
pub(crate) fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Verifying signatures is the emulator's concern, not this service's; these tests sign
    // nothing, so the check is off and the scope header is read for routing only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_rds_amazonrdsv19::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// An Amazon S3 client over this emulator's own in-process callback — the seam the export
/// writes its objects through, so a read here sees what the export really stored.
///
/// `pub(crate)` so the sibling suite that reads the objects one export wrote reaches them
/// through this one client rather than building a second copy of it.
pub(crate) fn s3(router: &MultiServiceRouter) -> aws_sdk_s3::Client {
    s3_in(router, REGION)
}

/// The same client signed for `region`, for the bucket a row creates outside the export's
/// own Region: Amazon S3 admits a location constraint only on the endpoint of the Region it
/// names.
fn s3_in(router: &MultiServiceRouter, region: &str) -> aws_sdk_s3::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        .force_path_style(true)
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_s3::Client::from_conf(config)
}

/// An IAM client over the same callback, for the roles the associations and the export name.
fn iam(router: &MultiServiceRouter) -> aws_sdk_iam::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_iam::Config::builder()
        .behavior_version(aws_sdk_iam::config::BehaviorVersion::latest())
        .region(aws_sdk_iam::config::Region::new(REGION))
        .endpoint_url("https://iam.amazonaws.com".to_string())
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_iam::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_iam::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_iam::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_iam::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_iam::Client::from_conf(config)
}

/// A KMS client over the same callback, for the key the export encrypts its objects under.
pub(crate) fn kms(router: &MultiServiceRouter) -> aws_sdk_kms::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_kms::Config::builder()
        .behavior_version(aws_sdk_kms::config::BehaviorVersion::latest())
        .region(aws_sdk_kms::config::Region::new(REGION))
        .endpoint_url(format!("https://kms.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_kms::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_kms::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_kms::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_kms::Client::from_conf(config)
}

/// Create a bucket in `region` through Amazon S3's own `CreateBucket`.
///
/// A regional endpoint requires the create to state its own location constraint ("the
/// unspecified location constraint is incompatible for the region specific endpoint this
/// request was sent to"), so every bucket here names the Region it is created in.
pub(crate) async fn create_bucket(router: &MultiServiceRouter, bucket: &str, region: &str) {
    s3_in(router, region)
        .create_bucket()
        .bucket(bucket)
        .create_bucket_configuration(
            aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(aws_sdk_s3::types::BucketLocationConstraint::from(region))
                .build(),
        )
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("the emulated Amazon S3 creates the bucket {bucket} in {region}: {error}")
        });
}

/// Stage one object in `bucket`, through Amazon S3's OWN `PutObject` — so an object this
/// service then reads is an object that service really holds.
///
/// `pub(crate)` so the sibling suite that drives the engine's own S3 import stages its
/// objects the same way rather than growing a second copy of this call.
pub(crate) async fn put_object(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    body: Vec<u8>,
) {
    s3(router)
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(aws_sdk_s3::primitives::ByteStream::from(body))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("the emulated Amazon S3 holds the object {bucket}/{key}: {error}")
        });
}

/// Create a role trusting `principal`, with `policy` attached inline when one is given, and
/// answer with its ARN — a role this emulator's own IAM holds, which is what the
/// association and the export resolve.
pub(crate) async fn role(
    router: &MultiServiceRouter,
    name: &str,
    principal: &str,
    policy: Option<String>,
) -> String {
    let created = iam(router)
        .create_role()
        .role_name(name)
        .assume_role_policy_document(trust_policy(principal))
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {name:?}: IAM creates the role: {error}"));
    if let Some(policy) = policy {
        iam(router)
            .put_role_policy()
            .role_name(name)
            .policy_name(format!("{name}-inline"))
            .policy_document(policy)
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {name:?}: IAM attaches the policy: {error}"));
    }
    created
        .role()
        .map(|role| role.arn().to_string())
        .unwrap_or_else(|| panic!("case {name:?}: the created role reports its ARN"))
}

/// Create a DB instance of `engine` and wait until it reports `available`.
pub(crate) async fn available_instance(router: &MultiServiceRouter, identifier: &str, engine: &str) {
    available_instance_of(
        router,
        identifier,
        engine,
        "db.t3.micro",
        20,
        Some(DATABASE),
    )
    .await;
}

/// The same, with the class and the allocated storage the engine is OFFERED with, and the
/// `DBName` it takes — the orderable options differ by engine family, and the Microsoft SQL
/// Server engines take no `DBName` at all, so an engine outside the default row above names
/// its own.
pub(crate) async fn available_instance_of(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    class: &str,
    storage: u32,
    database: Option<&str>,
) {
    let mut request: Vec<(&str, String)> = params![
        "DBInstanceIdentifier" => identifier,
        "Engine" => engine,
        "DBInstanceClass" => class,
        "AllocatedStorage" => storage,
        "MasterUsername" => MASTER_USER,
        "MasterUserPassword" => MASTER_PASSWORD,
    ]
    .to_vec();
    if let Some(database) = database {
        request.push(("DBName", database.to_string()));
    }
    expect_ok(identifier, call(router, "CreateDBInstance", &request).await);
    wait_for_settle();
}

/// Create a DB cluster of `engine` and wait until it reports `available`.
pub(crate) async fn available_cluster(router: &MultiServiceRouter, identifier: &str, engine: &str) {
    let request: Vec<(&str, String)> = params![
        "DBClusterIdentifier" => identifier,
        "Engine" => engine,
        "MasterUsername" => MASTER_USER,
        "MasterUserPassword" => MASTER_PASSWORD,
    ]
    .to_vec();
    expect_ok(identifier, call(router, "CreateDBCluster", &request).await);
    wait_for_settle();
}

/// The `(RoleArn, FeatureName, Status)` triples an envelope's `AssociatedRoles` reports.
pub(crate) fn associated_roles(envelope: &str) -> Vec<(String, String, String)> {
    subtrees(envelope, "AssociatedRoles")
        .into_iter()
        .flat_map(|roles| {
            subtrees(&roles, "DBInstanceRole")
                .into_iter()
                .chain(subtrees(&roles, "DBClusterRole"))
                .collect::<Vec<String>>()
        })
        .map(|role| {
            (
                text(&role, "RoleArn"),
                optional_text(&role, "FeatureName").unwrap_or_default(),
                text(&role, "Status"),
            )
        })
        .collect()
}

/// One row of the feature-name table: the engine, the `FeatureName` the request names, and
/// whether that engine version publishes it.
struct FeatureRow {
    case: &'static str,
    engine: &'static str,
    feature: &'static str,
    published: bool,
}

/// The feature names each engine family publishes as `SupportedFeatureNames` — the catalogue
/// `DescribeDBEngineVersions` reports and `AddRoleToDBInstance`'s own documentation points
/// at — spanning three families that publish some, one that publishes none, a name published
/// for ANOTHER engine, and a published name in the wrong case.
const FEATURE_ROWS: &[FeatureRow] = &[
    FeatureRow {
        case: "RDS for PostgreSQL imports from S3",
        engine: "postgres",
        feature: "s3Import",
        published: true,
    },
    FeatureRow {
        case: "RDS for PostgreSQL exports to S3",
        engine: "postgres",
        feature: "s3Export",
        published: true,
    },
    FeatureRow {
        case: "RDS for PostgreSQL invokes Lambda",
        engine: "postgres",
        feature: "Lambda",
        published: true,
    },
    FeatureRow {
        case: "RDS for MySQL publishes no role feature at all",
        engine: "mysql",
        feature: "s3Import",
        published: false,
    },
    FeatureRow {
        case: "a feature name published for the Oracle and SQL Server engines",
        engine: "postgres",
        feature: "S3_INTEGRATION",
        published: false,
    },
    FeatureRow {
        case: "a published name in another case",
        engine: "postgres",
        feature: "s3import",
        published: false,
    },
];

/// Rule: an association names a feature the resource's engine version publishes, and the
/// role it names is one `rds.amazonaws.com` may assume; the resource then REPORTS the
/// association, and dropping it removes exactly that entry.
#[tokio::test]
async fn an_instance_role_association_is_recorded_and_reported() {
    let (router, _engine) = emulator();
    let role_arn = role(
        &router,
        "rds-s3-instance-role",
        st::RDS_SERVICE_PRINCIPAL,
        None,
    )
    .await;

    for (index, row) in FEATURE_ROWS.iter().enumerate() {
        let identifier = format!("emu-role-instance-{index}");
        available_instance(&router, &identifier, row.engine).await;
        let request = params![
            "DBInstanceIdentifier" => identifier,
            "RoleArn" => role_arn,
            "FeatureName" => row.feature,
        ];
        let answer = call(&router, "AddRoleToDBInstance", &request).await;
        if !row.published {
            expect_error(row.case, answer, 400, "InvalidParameterValue");
            let described = expect_ok(
                row.case,
                call(
                    &router,
                    "DescribeDBInstances",
                    &params!["DBInstanceIdentifier" => identifier],
                )
                .await,
            );
            assert!(
                associated_roles(&described).is_empty(),
                "case {:?}: a refused association records nothing, got {:?}",
                row.case,
                associated_roles(&described)
            );
            continue;
        }
        expect_ok(row.case, answer);

        // The instance REPORTS the association it was given, as `ACTIVE`.
        let described = expect_ok(
            row.case,
            call(
                &router,
                "DescribeDBInstances",
                &params!["DBInstanceIdentifier" => identifier],
            )
            .await,
        );
        let reported = associated_roles(&described);
        assert_eq!(
            reported,
            vec![(role_arn.clone(), row.feature.to_string(), "ACTIVE".to_string())],
            "case {:?}: expected the association to be reported, got {reported:?}",
            row.case
        );

        // A feature of the instance's engine takes ONE role ARN, so a second association
        // for the same feature is refused rather than recorded twice — with the Query API's
        // parameter code and the engine's own sentence.
        let body = expect_error(
            row.case,
            call(&router, "AddRoleToDBInstance", &request).await,
            400,
            "InvalidParameterValue",
        );
        let message = text(&body, "Message");
        // Spelled out rather than taken from the code under test. Every published row above
        // runs the `postgres` engine, which Amazon RDS names `PostgreSQL` in a sentence.
        let expected = format!(
            "The engine PostgreSQL supports only one ARN associated with feature name {}.",
            row.feature
        );
        assert_eq!(
            message, expected,
            "case {:?}: expected the message {expected:?}, got {message:?}",
            row.case
        );

        // Dropping it removes exactly that entry; dropping it again is the not-found fault.
        expect_ok(
            row.case,
            call(&router, "RemoveRoleFromDBInstance", &request).await,
        );
        let described = expect_ok(
            row.case,
            call(
                &router,
                "DescribeDBInstances",
                &params!["DBInstanceIdentifier" => identifier],
            )
            .await,
        );
        assert!(
            associated_roles(&described).is_empty(),
            "case {:?}: expected no association after the removal, got {:?}",
            row.case,
            associated_roles(&described)
        );
        expect_error(
            row.case,
            call(&router, "RemoveRoleFromDBInstance", &request).await,
            404,
            "DBInstanceRoleNotFound",
        );
    }
}

/// The same rule for a DB CLUSTER, whose `FeatureName` is optional: an association that
/// names no feature is the cluster-wide one, and the published names are the Aurora
/// family's.
#[tokio::test]
async fn a_cluster_role_association_is_recorded_and_reported() {
    let (router, _engine) = emulator();
    let role_arn = role(
        &router,
        "rds-s3-cluster-role",
        st::RDS_SERVICE_PRINCIPAL,
        None,
    )
    .await;
    let identifier = "emu-role-cluster";
    available_cluster(&router, identifier, "aurora-mysql").await;

    // (case, the FeatureName the request names, whether Aurora MySQL publishes it)
    //
    // Aurora MySQL publishes the machine-learning features and NO S3 feature: the S3
    // load/unload integration of that engine is configured through the cluster parameter
    // group's import-role parameters, with the role associated under no feature at all,
    // which is the row that names none.
    let rows: Vec<(&str, Option<&str>, bool)> = vec![
        ("an Aurora machine-learning feature", Some("SageMaker"), true),
        ("no feature name at all", None, true),
        ("the S3 import feature, which Aurora MySQL does not publish", Some("s3Import"), false),
        ("the S3 export feature, likewise", Some("s3Export"), false),
        ("a name published for no Aurora version", Some("S3_INTEGRATION"), false),
    ];
    for (case, feature, published) in rows {
        let mut request: Vec<(&str, String)> = params![
            "DBClusterIdentifier" => identifier,
            "RoleArn" => role_arn,
        ]
        .to_vec();
        if let Some(feature) = feature {
            request.push(("FeatureName", feature.to_string()));
        }
        let answer = call(&router, "AddRoleToDBCluster", &request).await;
        if !published {
            expect_error(case, answer, 400, "InvalidParameterValue");
            continue;
        }
        expect_ok(case, answer);
        let described = expect_ok(
            case,
            call(
                &router,
                "DescribeDBClusters",
                &params!["DBClusterIdentifier" => identifier],
            )
            .await,
        );
        let reported = associated_roles(&described);
        assert!(
            reported.iter().any(|(arn, name, status)| arn == &role_arn
                && name == feature.unwrap_or("")
                && status == "ACTIVE"),
            "case {case:?}: expected the association to be reported, got {reported:?}"
        );
        expect_error(
            case,
            call(&router, "AddRoleToDBCluster", &request).await,
            400,
            "DBClusterRoleAlreadyExists",
        );
        expect_ok(
            case,
            call(&router, "RemoveRoleFromDBCluster", &request).await,
        );
        expect_error(
            case,
            call(&router, "RemoveRoleFromDBCluster", &request).await,
            404,
            "DBClusterRoleNotFound",
        );
    }
}

/// One row of the engine-name table: the resource an association is made on, the
/// `FeatureName` its engine does not publish, and the name that engine is published under.
struct EngineNameRow {
    case: &'static str,
    engine: &'static str,
    /// Whether the row's resource is a DB CLUSTER (the Aurora engines) or a DB instance.
    cluster: bool,
    feature: &'static str,
    /// The engine's `DBEngineDescription`, which is the name a sentence about the engine
    /// uses: the same description `DescribeDBEngineVersions` reports for it.
    display: &'static str,
}

/// The engine-name table: the refusal names the ENGINE, under the description that engine
/// publishes, so the rows span engines whose descriptions differ in form — an Aurora engine
/// named for its product, one whose description parenthesises the dialect, and the community
/// editions of the two provisioned engines — with a feature name published for ANOTHER
/// engine beside one published in another case.
const ENGINE_NAME_ROWS: &[EngineNameRow] = &[
    EngineNameRow {
        case: "Aurora MySQL does not publish the S3 import feature",
        engine: "aurora-mysql",
        cluster: true,
        feature: "s3Import",
        display: "Aurora MySQL",
    },
    EngineNameRow {
        case: "an Aurora PostgreSQL cluster named a feature of the Oracle engines",
        engine: "aurora-postgresql",
        cluster: true,
        feature: "S3_INTEGRATION",
        display: "Aurora (PostgreSQL)",
    },
    EngineNameRow {
        case: "RDS for MySQL publishes no role feature at all",
        engine: "mysql",
        cluster: false,
        feature: "s3Import",
        display: "MySQL Community Edition",
    },
    EngineNameRow {
        case: "a feature RDS for PostgreSQL publishes, named in another case",
        engine: "postgres",
        cluster: false,
        feature: "s3import",
        display: "PostgreSQL",
    },
];

/// Rule: a `FeatureName` the addressed resource's engine does not publish is refused with
/// `InvalidParameterValue` and the sentence AWS words it in — the value the request named and
/// the engine, under the description that engine publishes.
#[tokio::test]
async fn an_unpublished_feature_name_is_refused_naming_the_engine() {
    let (router, _engine) = emulator();
    let role_arn = role(&router, "rds-s3-feature-name", st::RDS_SERVICE_PRINCIPAL, None).await;
    for (index, row) in ENGINE_NAME_ROWS.iter().enumerate() {
        let identifier = format!("emu-feature-name-{index}");
        let (action, resource_member) = if row.cluster {
            available_cluster(&router, &identifier, row.engine).await;
            ("AddRoleToDBCluster", "DBClusterIdentifier")
        } else {
            available_instance(&router, &identifier, row.engine).await;
            ("AddRoleToDBInstance", "DBInstanceIdentifier")
        };
        let body = expect_error(
            row.case,
            call(
                &router,
                action,
                &params![
                    resource_member => identifier,
                    "RoleArn" => role_arn,
                    "FeatureName" => row.feature,
                ],
            )
            .await,
            400,
            "InvalidParameterValue",
        );
        let message = text(&body, "Message");
        let expected = format!(
            "The feature name {} is not valid for the engine {}.",
            row.feature, row.display
        );
        assert_eq!(
            message, expected,
            "case {:?}: expected the refusal to name the engine, got {message:?}",
            row.case
        );
    }
}

/// Rule: a role whose STORED trust policy does not let `rds.amazonaws.com` assume it cannot
/// be associated — `InvalidParameterValue` — whichever resource the request addressed.
#[tokio::test]
async fn a_role_that_does_not_trust_rds_cannot_be_associated() {
    let (router, _engine) = emulator();
    let instance = "emu-trust-instance";
    let cluster = "emu-trust-cluster";
    available_instance(&router, instance, "postgres").await;
    available_cluster(&router, cluster, "aurora-postgresql").await;

    // (case, the principal the role's trust policy names)
    let rows: &[(&str, &str)] = &[
        ("a role trusting the EXPORT principal only", st::EXPORT_SERVICE_PRINCIPAL),
        ("a role trusting another service", "lambda.amazonaws.com"),
        ("a role trusting Amazon S3", "s3.amazonaws.com"),
    ];
    for (index, (case, principal)) in rows.iter().enumerate() {
        let role_arn = role(
            &router,
            &format!("rds-s3-untrusted-{index}"),
            principal,
            None,
        )
        .await;
        expect_error(
            case,
            call(
                &router,
                "AddRoleToDBInstance",
                &params![
                    "DBInstanceIdentifier" => instance,
                    "RoleArn" => role_arn,
                    "FeatureName" => "s3Import",
                ],
            )
            .await,
            400,
            "InvalidParameterValue",
        );
        expect_error(
            case,
            call(
                &router,
                "AddRoleToDBCluster",
                &params![
                    "DBClusterIdentifier" => cluster,
                    "RoleArn" => role_arn,
                    "FeatureName" => "s3Import",
                ],
            )
            .await,
            400,
            "InvalidParameterValue",
        );
    }
}

/// Everything an export needs: a bucket, a KMS key, a role that may write it, and a DB
/// snapshot of an instance whose database holds rows.
struct ExportFixture {
    router: MultiServiceRouter,
    bucket: String,
    role_arn: String,
    key_id: String,
    snapshot_arn: String,
}

/// The tables the exported database holds: one with an integer key, text, real and 64-bit
/// integer columns, one with two text columns, and one that is empty — so the export spans
/// the column types the source engine holds, a value beyond a double's exact range, a null in
/// every nullable kind, and a table with no rows.
///
/// `ledger` is declared `BIGINT` rather than an exact-numeric type: a column declared
/// `DECIMAL`/`NUMERIC` carries that type's own precision and scale through the export, which
/// the sibling suite `snapshot_export_to_s3` pins, while the value below is what an INTEGER
/// column's export has to preserve.
const SEEDED_TABLES: &[(&str, &str, &[&str])] = &[
    (
        "orders",
        "CREATE TABLE orders (id INTEGER, customer TEXT, total REAL, ledger BIGINT)",
        &[
            // 2^53 + 1: a value a double cannot hold exactly, so the exported column has to
            // be a 64-bit integer for the row to survive the export.
            "INSERT INTO orders VALUES (1, 'alpha', 10.5, 9007199254740993)",
            "INSERT INTO orders VALUES (2, 'beta', 0.25, 2)",
            "INSERT INTO orders VALUES (3, NULL, NULL, NULL)",
        ],
    ),
    (
        "regions",
        "CREATE TABLE regions (code TEXT, name TEXT)",
        &["INSERT INTO regions VALUES ('wa', 'Washington')"],
    ),
    ("empty_audit", "CREATE TABLE empty_audit (note TEXT)", &[]),
];

/// Stand up an export's fixture: the instance, its seeded database, the snapshot taken of
/// it, the bucket, the key and the role.
async fn export_fixture(bucket: &str, role_name: &str) -> ExportFixture {
    let (router, engine) = emulator();
    let instance = "emu-export-source";
    available_instance(&router, instance, "mysql").await;

    // The rows the export will write: created through the instance's OWN engine — the same
    // database a client's session runs its statements against — so the snapshot taken below
    // carries real data.
    {
        let state = st::service_state(&*engine).expect("this service's store");
        let database = state
            .lock()
            .engine_endpoint(&(REGION.to_string(), instance.to_string()))
            .map(st::DbEngineEndpoint::database)
            .expect("a MySQL DB instance serves an engine endpoint");
        database
            .create_database(DATABASE)
            .expect("the instance's database is created");
        for (table, create, inserts) in SEEDED_TABLES {
            database
                .execute(DATABASE, create, &[])
                .unwrap_or_else(|error| panic!("seeding {table}: {}", error.message));
            for insert in *inserts {
                database
                    .execute(DATABASE, insert, &[])
                    .unwrap_or_else(|error| panic!("seeding {table}: {}", error.message));
            }
        }
    }

    let snapshot = "emu-export-snapshot";
    expect_ok(
        "snapshot the source",
        call(
            &router,
            "CreateDBSnapshot",
            &params![
                "DBInstanceIdentifier" => instance,
                "DBSnapshotIdentifier" => snapshot,
            ],
        )
        .await,
    );
    wait_for_settle();

    create_bucket(&router, bucket, REGION).await;
    let key_id = kms(&router)
        .create_key()
        .description("the export's encryption key")
        .send()
        .await
        .expect("the emulated KMS creates the export key")
        .key_metadata()
        .map(|metadata| metadata.key_id().to_string())
        .expect("the created key reports its id");
    let role_arn = role(
        &router,
        role_name,
        st::EXPORT_SERVICE_PRINCIPAL,
        Some(put_object_policy(bucket)),
    )
    .await;

    ExportFixture {
        router,
        bucket: bucket.to_string(),
        role_arn,
        key_id,
        snapshot_arn: format!("arn:aws:rds:{REGION}:{ACCOUNT}:snapshot:{snapshot}"),
    }
}

/// The keys the destination bucket holds under `prefix`, read through Amazon S3's own
/// `ListObjectsV2`.
async fn listed_keys(fixture: &ExportFixture, prefix: &str) -> Vec<String> {
    s3(&fixture.router)
        .list_objects_v2()
        .bucket(&fixture.bucket)
        .prefix(prefix)
        .send()
        .await
        .expect("Amazon S3 lists the destination bucket")
        .contents()
        .iter()
        .filter_map(|object| object.key().map(str::to_string))
        .collect()
}

/// The bytes Amazon S3 holds at `key`, read through its own `GetObject`.
async fn object_bytes(fixture: &ExportFixture, key: &str) -> Vec<u8> {
    let answer = s3(&fixture.router)
        .get_object()
        .bucket(&fixture.bucket)
        .key(key)
        .send()
        .await
        .unwrap_or_else(|error| panic!("Amazon S3 holds the exported object {key}: {error}"));
    answer
        .body
        .collect()
        .await
        .expect("the exported object's bytes")
        .to_vec()
}

/// One row of the export table: the case, the export's identifier, its `S3Prefix`, the
/// `ExportOnly` entries it names, and the tables it must then have written.
struct ExportRow {
    case: &'static str,
    identifier: &'static str,
    prefix: Option<&'static str>,
    export_only: &'static [&'static str],
    expected_tables: &'static [&'static str],
}

/// Rule: an export WRITES the source's data — one Parquet object per exported table and the
/// JSON file that says what was exported — and the task it records walks STARTING →
/// IN_PROGRESS → COMPLETE.
///
/// The rows span a prefix given and omitted, and `ExportOnly` lists of zero, one and two
/// entries, which is what the member generalises over.
#[tokio::test]
async fn an_export_writes_its_objects_and_completes() {
    const ROWS: &[ExportRow] = &[
        ExportRow {
            case: "the whole snapshot, at the root of the bucket",
            identifier: "emu-export-all",
            prefix: None,
            export_only: &[],
            expected_tables: &["orders", "regions", "empty_audit"],
        },
        ExportRow {
            case: "one table, under a prefix",
            identifier: "emu-export-one",
            prefix: Some("nightly/exports"),
            export_only: &["appdb.orders"],
            expected_tables: &["orders"],
        },
        ExportRow {
            case: "two tables of one database",
            identifier: "emu-export-two",
            prefix: Some("archive"),
            export_only: &["appdb.orders", "appdb.regions"],
            expected_tables: &["orders", "regions"],
        },
        ExportRow {
            case: "a whole database named by itself",
            identifier: "emu-export-database",
            prefix: None,
            export_only: &["appdb"],
            expected_tables: &["orders", "regions", "empty_audit"],
        },
    ];

    for row in ROWS {
        let ExportRow {
            case,
            identifier,
            prefix,
            export_only,
            expected_tables,
        } = *row;
        let fixture = export_fixture(
            &format!("emu-export-bucket-{identifier}"),
            &format!("rds-export-role-{identifier}"),
        )
        .await;
        let mut request: Vec<(&str, String)> = params![
            "ExportTaskIdentifier" => identifier,
            "SourceArn" => fixture.snapshot_arn,
            "S3BucketName" => fixture.bucket,
            "IamRoleArn" => fixture.role_arn,
            "KmsKeyId" => fixture.key_id,
        ]
        .to_vec();
        if let Some(prefix) = prefix {
            request.push(("S3Prefix", prefix.to_string()));
        }
        for (index, entry) in export_only.iter().enumerate() {
            request.push((
                match index {
                    0 => "ExportOnly.member.1",
                    _ => "ExportOnly.member.2",
                },
                (*entry).to_string(),
            ));
        }
        let started = expect_ok(
            case,
            call(&fixture.router, "StartExportTask", &request).await,
        );
        assert_eq!(
            text(&started, "Status"),
            st::EXPORT_TASK_STATUS_STARTING,
            "case {case:?}: an accepted export starts in STARTING, got {started}"
        );
        assert_eq!(
            text(&started, "SourceType"),
            st::EXPORT_SOURCE_TYPE_SNAPSHOT,
            "case {case:?}: a snapshot export reports SourceType SNAPSHOT, got {started}"
        );

        // The objects the export wrote, read back through Amazon S3's own listing: one part
        // file per exported table under `<prefix>/<task>/<db>/<schema>.<table>/`, and the
        // file that says what was exported beside them.
        let export_prefix = st::export_prefix(prefix, identifier);
        let keys = listed_keys(&fixture, &export_prefix).await;
        let manifest_key = st::tables_info_key(&export_prefix, identifier, expected_tables.len());
        assert!(
            keys.contains(&manifest_key),
            "case {case:?}: expected the export's own JSON file at {manifest_key}, got {keys:?}"
        );
        for table in expected_tables {
            let table_prefix = format!("{export_prefix}{DATABASE}/{DATABASE}.{table}/");
            assert!(
                keys.iter().any(|key| key.starts_with(&table_prefix)
                    && key.ends_with(".gz.parquet")),
                "case {case:?}: expected a Parquet object under {table_prefix}, got {keys:?}"
            );
        }
        for key in &keys {
            assert!(
                key.starts_with(&export_prefix),
                "case {case:?}: every exported object is written under the export's own \
                 prefix {export_prefix}, got {key}"
            );
        }
        let unexpected: Vec<&String> = keys
            .iter()
            .filter(|key| {
                key.ends_with(".gz.parquet")
                    && !expected_tables.iter().any(|table| {
                        key.contains(&format!("{DATABASE}.{table}/"))
                    })
            })
            .collect();
        assert!(
            unexpected.is_empty(),
            "case {case:?}: ExportOnly limits what is written, got {unexpected:?}"
        );

        // The file that says what was exported is JSON and names the tables that were
        // written.
        let manifest = object_bytes(&fixture, &manifest_key).await;
        let document: Value = serde_json::from_slice(&manifest).unwrap_or_else(|error| {
            panic!("case {case:?}: the export's JSON file must parse: {error}")
        });
        let entries = document
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a JSON array, got {document}"));
        assert_eq!(
            entries.len(),
            expected_tables.len(),
            "case {case:?}: expected one entry per exported table, got {document}"
        );
        for table in expected_tables {
            assert!(
                entries.iter().any(|entry| entry.get("table")
                    == Some(&Value::String((*table).to_string()))),
                "case {case:?}: expected {table} in the export's JSON file, got {document}"
            );
        }

        // The exported bytes are a real Apache Parquet file carrying the source's own rows.
        let orders_key = keys
            .iter()
            .find(|key| key.contains(&format!("{DATABASE}.orders/")))
            .unwrap_or_else(|| panic!("case {case:?}: the orders table was exported"))
            .clone();
        let parquet = object_bytes(&fixture, &orders_key).await;
        let reader = SerializedFileReader::new(Bytes::from(parquet))
            .unwrap_or_else(|error| panic!("case {case:?}: the object is Parquet: {error}"));
        let rows_written = reader.metadata().file_metadata().num_rows();
        assert_eq!(
            rows_written, 3,
            "case {case:?}: expected the three seeded rows in the Parquet object, got \
             {rows_written}"
        );
        let schema = reader.metadata().file_metadata().schema_descr();
        let columns: Vec<String> = (0..schema.num_columns())
            .map(|index| schema.column(index).name().to_string())
            .collect();
        assert_eq!(
            columns,
            vec![
                "id".to_string(),
                "customer".to_string(),
                "total".to_string(),
                "ledger".to_string()
            ],
            "case {case:?}: expected the source's own columns, got {columns:?}"
        );
        let rows: Vec<::parquet::record::Row> = reader
            .get_row_iter(None)
            .unwrap_or_else(|error| panic!("case {case:?}: the Parquet rows read: {error}"))
            .filter_map(Result::ok)
            .collect();
        let customers: Vec<String> = rows
            .iter()
            .filter_map(|row| row.get_string(1).ok().cloned())
            .collect();
        assert_eq!(
            customers,
            vec!["alpha".to_string(), "beta".to_string()],
            "case {case:?}: expected the rows the source holds, got {customers:?}"
        );
        // The value beyond a double's exact range survived the export, which it does only
        // because an all-integer column is exported as a 64-bit integer.
        let ledger: Vec<i64> = rows.iter().filter_map(|row| row.get_long(3).ok()).collect();
        assert_eq!(
            ledger,
            vec![9_007_199_254_740_993_i64, 2],
            "case {case:?}: expected the source's own integers, got {ledger:?}"
        );

        // The task the export recorded walks the progression the real service reports.
        wait_for_settle();
        let described = expect_ok(
            case,
            call(
                &fixture.router,
                "DescribeExportTasks",
                &params!["ExportTaskIdentifier" => identifier],
            )
            .await,
        );
        assert_eq!(
            text(&described, "Status"),
            st::EXPORT_TASK_STATUS_IN_PROGRESS,
            "case {case:?}: one span after the start the export is IN_PROGRESS, got {described}"
        );
        wait_for_settle();
        let described = expect_ok(
            case,
            call(
                &fixture.router,
                "DescribeExportTasks",
                &params!["ExportTaskIdentifier" => identifier],
            )
            .await,
        );
        assert_eq!(
            text(&described, "Status"),
            st::EXPORT_TASK_STATUS_COMPLETE,
            "case {case:?}: the export then completes, got {described}"
        );
        assert_eq!(
            text(&described, "PercentProgress"),
            st::COMPLETE_PERCENT_PROGRESS.to_string(),
            "case {case:?}: a completed export reports all of its progress, got {described}"
        );
        assert!(
            !texts(&described, "TaskEndTime").is_empty(),
            "case {case:?}: a completed export reports when it ended, got {described}"
        );
    }
}

/// Rule: every documented refusal of an export is answered with the code AWS answers it
/// with, and nothing is written for a refused export.
#[tokio::test]
async fn each_documented_export_refusal_is_answered_with_its_own_code() {
    let fixture = export_fixture("emu-export-refusals", "rds-export-refusal-role").await;
    let router = &fixture.router;

    // A bucket Amazon S3 holds nothing under.
    expect_error(
        "a destination bucket that does not exist",
        call(
            router,
            "StartExportTask",
            &params![
                "ExportTaskIdentifier" => "emu-refuse-bucket",
                "SourceArn" => fixture.snapshot_arn,
                "S3BucketName" => "emu-export-bucket-that-does-not-exist",
                "IamRoleArn" => fixture.role_arn,
                "KmsKeyId" => fixture.key_id,
            ],
        )
        .await,
        400,
        "InvalidS3BucketFault",
    );

    // A bucket in another Region: the export writes where the source is.
    create_bucket(router, "emu-export-elsewhere", OTHER_REGION).await;
    expect_error(
        "a destination bucket in another Region",
        call(
            router,
            "StartExportTask",
            &params![
                "ExportTaskIdentifier" => "emu-refuse-region",
                "SourceArn" => fixture.snapshot_arn,
                "S3BucketName" => "emu-export-elsewhere",
                "IamRoleArn" => fixture.role_arn,
                "KmsKeyId" => fixture.key_id,
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );

    // A role that does not trust the export principal.
    let untrusted = role(
        router,
        "rds-export-untrusted",
        st::RDS_SERVICE_PRINCIPAL,
        Some(put_object_policy(&fixture.bucket)),
    )
    .await;
    expect_error(
        "a role whose trust policy omits the export principal",
        call(
            router,
            "StartExportTask",
            &params![
                "ExportTaskIdentifier" => "emu-refuse-trust",
                "SourceArn" => fixture.snapshot_arn,
                "S3BucketName" => fixture.bucket,
                "IamRoleArn" => untrusted,
                "KmsKeyId" => fixture.key_id,
            ],
        )
        .await,
        404,
        "IamRoleNotFound",
    );

    // A role that may be assumed but is granted no object write.
    let unprivileged = role(
        router,
        "rds-export-unprivileged",
        st::EXPORT_SERVICE_PRINCIPAL,
        None,
    )
    .await;
    expect_error(
        "a role without s3:PutObject",
        call(
            router,
            "StartExportTask",
            &params![
                "ExportTaskIdentifier" => "emu-refuse-permissions",
                "SourceArn" => fixture.snapshot_arn,
                "S3BucketName" => fixture.bucket,
                "IamRoleArn" => unprivileged,
                "KmsKeyId" => fixture.key_id,
            ],
        )
        .await,
        400,
        "IamRoleMissingPermissions",
    );

    // A KMS key KMS holds nothing under.
    expect_error(
        "a KMS key that does not exist",
        call(
            router,
            "StartExportTask",
            &params![
                "ExportTaskIdentifier" => "emu-refuse-key",
                "SourceArn" => fixture.snapshot_arn,
                "S3BucketName" => fixture.bucket,
                "IamRoleArn" => fixture.role_arn,
                "KmsKeyId" => format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/11111111-2222-3333-4444-555555555555"),
            ],
        )
        .await,
        400,
        "KMSKeyNotAccessibleFault",
    );

    // An `ExportOnly` entry outside the published forms.
    expect_error(
        "an ExportOnly entry naming four segments",
        call(
            router,
            "StartExportTask",
            &params![
                "ExportTaskIdentifier" => "emu-refuse-export-only",
                "SourceArn" => fixture.snapshot_arn,
                "S3BucketName" => fixture.bucket,
                "IamRoleArn" => fixture.role_arn,
                "KmsKeyId" => fixture.key_id,
                "ExportOnly.member.1" => "appdb.public.orders.extra",
            ],
        )
        .await,
        400,
        "InvalidExportOnly",
    );

    // A source this region holds nothing under, for each of the three source kinds.
    let missing_sources: &[(&str, String, &str)] = &[
        (
            "a DB snapshot that does not exist",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:snapshot:emu-missing-snapshot"),
            "DBSnapshotNotFound",
        ),
        (
            "a DB cluster snapshot that does not exist",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster-snapshot:emu-missing-cluster-snap"),
            "DBClusterSnapshotNotFoundFault",
        ),
        (
            "a DB cluster that does not exist",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:emu-missing-cluster"),
            "DBClusterNotFoundFault",
        ),
    ];
    for (case, source_arn, code) in missing_sources {
        expect_error(
            case,
            call(
                router,
                "StartExportTask",
                &params![
                    "ExportTaskIdentifier" => "emu-refuse-source",
                    "SourceArn" => source_arn,
                    "S3BucketName" => fixture.bucket,
                    "IamRoleArn" => fixture.role_arn,
                    "KmsKeyId" => fixture.key_id,
                ],
            )
            .await,
            404,
            code,
        );
    }

    // Nothing above was written: a refused export leaves the bucket and the store as they
    // were.
    let keys = listed_keys(&fixture, "").await;
    assert!(
        keys.is_empty(),
        "a refused export writes no object, got {keys:?}"
    );
    let described = expect_ok(
        "no task was recorded",
        call(router, "DescribeExportTasks", &params!["MaxRecords" => 20]).await,
    );
    assert!(
        subtrees(&described, "ExportTask").is_empty(),
        "a refused export records no task, got {described}"
    );

    // And an identifier an export is already running under takes no second one.
    let request = params![
        "ExportTaskIdentifier" => "emu-export-twice",
        "SourceArn" => fixture.snapshot_arn,
        "S3BucketName" => fixture.bucket,
        "IamRoleArn" => fixture.role_arn,
        "KmsKeyId" => fixture.key_id,
    ];
    expect_ok(
        "the first export under this identifier",
        call(router, "StartExportTask", &request).await,
    );
    expect_error(
        "a second export under the same identifier",
        call(router, "StartExportTask", &request).await,
        400,
        "ExportTaskAlreadyExists",
    );
}

/// Rule: the destination bucket's own policy is part of the permissions decision — an
/// explicit deny of the object write refuses the export — and an export is visible only in
/// the Region it was started in.
#[tokio::test]
async fn a_denying_bucket_policy_refuses_the_export() {
    let fixture = export_fixture("emu-export-denied", "rds-export-denied-role").await;
    s3(&fixture.router)
        .put_bucket_policy()
        .bucket(&fixture.bucket)
        .policy(deny_policy(&fixture.bucket, &fixture.role_arn))
        .send()
        .await
        .expect("the emulated Amazon S3 takes the bucket policy");
    expect_error(
        "a bucket policy denying the export role the object write",
        call(
            &fixture.router,
            "StartExportTask",
            &params![
                "ExportTaskIdentifier" => "emu-export-denied-task",
                "SourceArn" => fixture.snapshot_arn,
                "S3BucketName" => fixture.bucket,
                "IamRoleArn" => fixture.role_arn,
                "KmsKeyId" => fixture.key_id,
            ],
        )
        .await,
        400,
        "IamRoleMissingPermissions",
    );
}

/// One row of the missing-permissions table: the case, the role the export is asked to run
/// under and the destination bucket, which are the two values the refusal's sentence names.
struct MissingPermissionsRow {
    case: &'static str,
    role_arn: String,
    bucket: String,
}

/// Rule: the `IamRoleMissingPermissionsFault` refusal carries Amazon RDS's own fixed
/// sentence for that error class, whose only variable parts are the request's `IamRoleArn`
/// and `S3BucketName` — whichever half of the authorization decision withheld the object
/// write.
///
/// The rows span what the rule generalises over: the role's own policies not granting the
/// write (one role carrying no policy at all, one granted it on a DIFFERENT bucket, one
/// whose name is much longer) and the destination bucket's policy explicitly denying it, so
/// both halves and both variable parts vary across the table.
#[tokio::test]
async fn the_missing_permissions_refusal_carries_the_service_s_own_sentence() {
    let fixture = export_fixture("emu-export-message", "rds-export-message-role").await;
    let router = &fixture.router;

    // A role the export principal may assume that carries no identity policy at all.
    let bare = role(
        router,
        "rds-export-msg-bare",
        st::EXPORT_SERVICE_PRINCIPAL,
        None,
    )
    .await;
    // The same, under a much longer role name: the ARN is taken from the request whatever
    // its length.
    let long_name = role(
        router,
        "rds-export-msg-role-with-a-rather-longer-name-than-the-others",
        st::EXPORT_SERVICE_PRINCIPAL,
        None,
    )
    .await;
    // A role granted the write on the FIXTURE's bucket only, asked to write another one.
    let other_bucket = "emu-export-msg-other";
    create_bucket(router, other_bucket, REGION).await;
    let elsewhere = role(
        router,
        "rds-export-msg-elsewhere",
        st::EXPORT_SERVICE_PRINCIPAL,
        Some(put_object_policy(&fixture.bucket)),
    )
    .await;
    // And a bucket whose own policy DENIES the write to a role whose identity policy grants
    // it: the other half of the decision.
    let deny_bucket = "emu-export-msg-deny";
    create_bucket(router, deny_bucket, REGION).await;
    let denied = role(
        router,
        "rds-export-msg-denied",
        st::EXPORT_SERVICE_PRINCIPAL,
        Some(put_object_policy(deny_bucket)),
    )
    .await;
    s3(router)
        .put_bucket_policy()
        .bucket(deny_bucket)
        .policy(deny_policy(deny_bucket, &denied))
        .send()
        .await
        .expect("the emulated Amazon S3 takes the bucket policy");

    let rows = [
        MissingPermissionsRow {
            case: "a role carrying no identity policy",
            role_arn: bare,
            bucket: fixture.bucket.clone(),
        },
        MissingPermissionsRow {
            case: "a role with a long name carrying no identity policy",
            role_arn: long_name,
            bucket: fixture.bucket.clone(),
        },
        MissingPermissionsRow {
            case: "a role granted the write on another bucket only",
            role_arn: elsewhere,
            bucket: other_bucket.to_string(),
        },
        MissingPermissionsRow {
            case: "a bucket policy denying the role the write",
            role_arn: denied,
            bucket: deny_bucket.to_string(),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let body = expect_error(
            row.case,
            call(
                router,
                "StartExportTask",
                &params![
                    "ExportTaskIdentifier" => format!("emu-export-msg-{index}"),
                    "SourceArn" => fixture.snapshot_arn,
                    "S3BucketName" => row.bucket,
                    "IamRoleArn" => row.role_arn,
                    "KmsKeyId" => fixture.key_id,
                ],
            )
            .await,
            400,
            "IamRoleMissingPermissions",
        );
        // The sentence Amazon RDS words this fault as, with the role and the bucket of THIS
        // request in it and nothing else.
        let expected = format!(
            "The IAM role {} isn't authorized to call s3:PutObject on the S3 bucket {}.",
            row.role_arn, row.bucket
        );
        let message = text(&body, "Message");
        assert_eq!(
            message, expected,
            "case {:?}: expected the refusal message {expected:?}, got {message:?}",
            row.case
        );
    }

    // Nothing was accepted: a refused export records no task.
    let described = expect_ok(
        "no task was recorded",
        call(router, "DescribeExportTasks", &params!["MaxRecords" => 20]).await,
    );
    assert!(
        subtrees(&described, "ExportTask").is_empty(),
        "a refused export records no task, got {described}"
    );
}

/// Rule: an export is reported in the Region it was started in and nowhere else, which is
/// what every regional read of this service holds to.
#[tokio::test]
async fn an_export_is_reported_only_in_its_own_region() {
    let fixture = export_fixture("emu-export-regional", "rds-export-regional-role").await;
    let identifier = "emu-export-regional-task";
    expect_ok(
        "start the export",
        call(
            &fixture.router,
            "StartExportTask",
            &params![
                "ExportTaskIdentifier" => identifier,
                "SourceArn" => fixture.snapshot_arn,
                "S3BucketName" => fixture.bucket,
                "IamRoleArn" => fixture.role_arn,
                "KmsKeyId" => fixture.key_id,
            ],
        )
        .await,
    );
    let here = expect_ok(
        "the export's own region",
        call(
            &fixture.router,
            "DescribeExportTasks",
            &params!["ExportTaskIdentifier" => identifier],
        )
        .await,
    );
    assert_eq!(
        text(&here, "ExportTaskIdentifier"),
        identifier,
        "expected the export to be reported in the region it was started in, got {here}"
    );
    let elsewhere = expect_ok(
        "another region",
        call_in(
            &fixture.router,
            OTHER_REGION,
            "DescribeExportTasks",
            &params!["MaxRecords" => 20],
        )
        .await,
    );
    assert!(
        subtrees(&elsewhere, "ExportTask").is_empty(),
        "an export is not reported in another region, got {elsewhere}"
    );
}
