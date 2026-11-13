// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the gates a create of the Amazon Neptune control plane's resources
//! passes through on this front door, and for the order they are evaluated in.
//!
//! One awsQuery front door serves three control planes (`src/service.rs`), so a client
//! compiled against the Amazon Neptune API signs for `rds` and is answered here. The rules
//! pinned below, each over a TABLE spanning what it generalises over rather than over one
//! request:
//!
//! * a `MasterUserPassword` supplied for an engine whose own API documents the member as one
//!   it does not take is refused `400 InvalidParameterValue` with one fixed sentence, on
//!   every operation that carries the member, and leaves nothing created — while an engine
//!   that DOES take one accepts the same values;
//! * a create resolves the resource it REFERENCES before it reports on the request's own
//!   members: a `CreateDBInstance` naming a DB cluster that resolves to nothing answers
//!   `404 DBClusterNotFoundFault` whichever required member the request also left out, and a
//!   `CreateDBCluster` naming a DB subnet group that resolves to nothing answers
//!   `404 DBSubnetGroupNotFoundFault` whatever scaling configuration it asks for;
//! * an identifier-scoped `DescribeDBInstances` for an instance nothing created answers
//!   `404 DBInstanceNotFound`, so a refused create leaves no instance behind;
//! * an instance of an engine whose instances are cluster MEMBERS cannot be created outside
//!   a cluster at all: it would have no master user credential anywhere, and the create is
//!   refused with the same fixed sentence;
//! * a serverless scaling configuration is accepted for the Neptune engine inside the
//!   capacity range its releases publish, in half-step increments, and a floor above the
//!   ceiling is refused `400 InvalidParameterValue` with AWS's one fixed sentence;
//! * the resources a cluster create REFERENCES in another service — the KMS key it is
//!   encrypted under and the VPC security groups it joins — are resolved against the
//!   services that OWN them, so a key or a group the account does not hold is refused
//!   instead of being stored as a reference nothing could resolve.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so each assertion
//! reads the XML the emulator itself answered with.

use std::sync::Arc;

use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use service_com_amazonaws_rds_amazonrdsv19::state::{
    db_cluster_cannot_be_accessed_message, db_subnet_group_does_not_exist_message,
    invalid_security_group_message, kms_key_not_accessible_message,
    INVALID_MASTER_PASSWORD_MESSAGE, SERVERLESS_V2_FLOOR_ABOVE_CEILING_MESSAGE,
};
use service_com_amazonaws_rds_amazonrdsv19::EmulatedService;

use crate::db_clusters::describe_one;
use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, subtrees, text, wait_for_settle, REGION,
};

/// The Neptune engine, whose API documents `MasterUserPassword` as "Not supported by
/// Neptune" and whose serverless clusters scale in Neptune capacity units.
const NEPTUNE: &str = "neptune";

/// The instance class a Neptune cluster member is ordered in.
const NEPTUNE_CLASS: &str = "db.r5.large";

/// `CreateDBSubnetGroup` with the two subnets a group must name.
async fn create_subnet_group(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "CreateDBSubnetGroup",
        &[
            ("DBSubnetGroupName", name),
            ("DBSubnetGroupDescription", "neptune create gates"),
            ("SubnetIds.SubnetIdentifier.1", "subnet-0123456789abcdef0"),
            ("SubnetIds.SubnetIdentifier.2", "subnet-0fedcba987654321f"),
        ],
    )
    .await
}

/// The passwords the master-password rule is spanned over: one well inside every engine's
/// published bounds, each published length bound of the engine that takes the member
/// (8 and 128 characters) and one carrying a character no engine's member admits.
///
/// They differ along what the rule generalises over — length and alphabet — and the point
/// of the table is that the ENGINE decides, not the value: every one of them is refused for
/// an engine that takes no master password, and the first three are accepted by one that
/// does.
fn password_rows() -> Vec<(&'static str, String)> {
    vec![
        ("a password well inside every published bound", "pw1-Secret".to_string()),
        ("the lower bound every engine publishes", "p".repeat(8)),
        ("the upper bound the control engine publishes", "p".repeat(128)),
        ("a character the member excludes", "pw1/Secret".to_string()),
    ]
}

/// One operation of this front door that carries a `MasterUserPassword`: the action, the
/// member naming the resource it addresses, and the other members the call needs.
struct PasswordOperation {
    action: &'static str,
    identifier_member: &'static str,
    /// The members beyond the identifier and the password, with `Engine` filled in with the
    /// engine under test where the call carries one.
    extra: &'static [(&'static str, &'static str)],
}

/// Every such operation.
///
/// Source: `smithy_query(query="operation",operation="CreateDBCluster,CreateDBInstance,
/// ModifyDBCluster,ModifyDBInstance")` on the Amazon Neptune model, each of whose
/// `MasterUserPassword` members documents "Not supported by Neptune".
const PASSWORD_OPERATIONS: &[PasswordOperation] = &[
    PasswordOperation {
        action: "CreateDBCluster",
        identifier_member: "DBClusterIdentifier",
        extra: &[("Engine", "")],
    },
    PasswordOperation {
        action: "CreateDBInstance",
        identifier_member: "DBInstanceIdentifier",
        extra: &[("Engine", ""), ("DBInstanceClass", NEPTUNE_CLASS)],
    },
    PasswordOperation {
        action: "ModifyDBCluster",
        identifier_member: "DBClusterIdentifier",
        extra: &[],
    },
    PasswordOperation {
        action: "ModifyDBInstance",
        identifier_member: "DBInstanceIdentifier",
        extra: &[],
    },
];

#[tokio::test]
async fn a_master_password_is_refused_for_an_engine_whose_api_takes_none() {
    let router = emulator();

    // The Neptune cluster and member instance the two modifications address, created
    // without a master password — which is how this engine's resources are created at all.
    expect_ok(
        "the Neptune cluster the modifications address",
        call(
            &router,
            "CreateDBCluster",
            &[("DBClusterIdentifier", "emu-npt-pass"), ("Engine", NEPTUNE)],
        )
        .await,
    );
    expect_ok(
        "the Neptune instance the modifications address",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-npt-pass-1"),
                ("Engine", NEPTUNE),
                ("DBInstanceClass", NEPTUNE_CLASS),
                ("DBClusterIdentifier", "emu-npt-pass"),
            ],
        )
        .await,
    );
    // A modification is taken only on an instance that has reached `available`, so the two
    // rows addressing one wait out the transition this service owes it first.
    wait_for_settle();

    for (index, (case, password)) in password_rows().into_iter().enumerate() {
        for operation in PASSWORD_OPERATIONS {
            let created = format!("emu-npt-pass-new-{index}");
            let addressed = match operation.action {
                "ModifyDBCluster" => "emu-npt-pass".to_string(),
                "ModifyDBInstance" => "emu-npt-pass-1".to_string(),
                _ => created,
            };
            let mut params: Vec<(&str, String)> = vec![
                (operation.identifier_member, addressed.clone()),
                ("MasterUserPassword", password.clone()),
            ];
            for (member, value) in operation.extra.iter() {
                let value = if *member == "Engine" {
                    NEPTUNE.to_string()
                } else {
                    (*value).to_string()
                };
                params.push((member, value));
            }
            let case = format!("{case} ({})", operation.action);
            let body = expect_error(
                &case,
                call(&router, operation.action, &params).await,
                400,
                "InvalidParameterValue",
            );
            let message = text(&body, "Message");
            assert_eq!(
                message, INVALID_MASTER_PASSWORD_MESSAGE,
                "case {case:?}: expected the fixed sentence \
                 {INVALID_MASTER_PASSWORD_MESSAGE:?}, got {message:?} in {body}"
            );
            assert!(
                !body.contains(password.as_str()),
                "case {case:?}: the refusal must echo no part of the password, got {body}"
            );
            // The refused create left nothing behind: the identifier it named resolves to
            // no instance at all.
            if operation.action == "CreateDBInstance" {
                expect_error(
                    &format!("{case}: the instance was not created"),
                    call(
                        &router,
                        "DescribeDBInstances",
                        &[("DBInstanceIdentifier", addressed.as_str())],
                    )
                    .await,
                    404,
                    "DBInstanceNotFound",
                );
            }
        }
    }

    // The control: an engine whose own API DOES publish the member takes the same values.
    // Aurora PostgreSQL, whose `MasterUserPassword` publishes "8 to 128 characters", so
    // every row but the last (a character no engine's member admits) is inside its bounds.
    for (index, (case, password)) in password_rows().into_iter().enumerate().take(3) {
        let identifier = format!("emu-aurora-pass-{index}");
        expect_ok(
            &format!("{case} (an engine that takes a master password)"),
            call(
                &router,
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", identifier.as_str()),
                    ("Engine", "aurora-postgresql"),
                    ("MasterUsername", "emuadmin"),
                    ("MasterUserPassword", password.as_str()),
                ],
            )
            .await,
        );
    }
}

/// The required members of `CreateDBInstanceMessage` a row leaves out, from
/// `smithy_query(query="operation",operation="CreateDBInstance")`: each of them on its own,
/// and the complete request as the control.
const OMITTED_MEMBERS: [(&str, Option<&str>); 4] = [
    ("the class the instance is ordered in", Some("DBInstanceClass")),
    ("the engine it runs", Some("Engine")),
    ("the identifier it would be stored under", Some("DBInstanceIdentifier")),
    ("nothing at all", None),
];

#[tokio::test]
async fn an_instance_create_resolves_its_cluster_before_it_reports_the_request_shape() {
    let router = emulator();

    // The cluster the control rows below name, so a request that resolves is a row too.
    expect_ok(
        "the cluster that does resolve",
        call(
            &router,
            "CreateDBCluster",
            &[("DBClusterIdentifier", "emu-npt-live"), ("Engine", NEPTUNE)],
        )
        .await,
    );

    // A cluster identifier nothing created, in the shapes the member admits.
    for (shape, absent_cluster) in [
        ("a name nothing was created under", "emu-npt-no-such-cluster"),
        ("a near miss of a stored name", "emu-npt-live-2"),
        ("the stored name in mixed case, misspelled", "EMU-NPT-Absent"),
    ] {
        for (index, (omitted_case, omitted)) in OMITTED_MEMBERS.iter().enumerate() {
            let identifier = format!("emu-npt-member-{index}");
            let mut params: Vec<(&str, String)> = vec![
                ("DBClusterIdentifier", absent_cluster.to_string()),
                ("DBInstanceIdentifier", identifier.clone()),
                ("Engine", NEPTUNE.to_string()),
                ("DBInstanceClass", NEPTUNE_CLASS.to_string()),
            ];
            if let Some(member) = omitted {
                params.retain(|(name, _)| name != member);
            }
            let case = format!("{shape}, omitting {omitted_case}");
            let body = expect_error(
                &case,
                call(&router, "CreateDBInstance", &params).await,
                404,
                "DBClusterNotFoundFault",
            );
            let expected = db_cluster_cannot_be_accessed_message(absent_cluster);
            let message = text(&body, "Message");
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?} in {body}"
            );
        }
    }

    // The gate reaches no further than that: a request whose cluster RESOLVES is served by
    // the handler, which refuses an absent member exactly as this front door always has.
    for (case, omitted) in OMITTED_MEMBERS.iter().filter_map(|(case, omitted)| {
        omitted.map(|member| (case, member))
    }) {
        let mut params: Vec<(&str, String)> = vec![
            ("DBClusterIdentifier", "emu-npt-live".to_string()),
            ("DBInstanceIdentifier", "emu-npt-live-member".to_string()),
            ("Engine", NEPTUNE.to_string()),
            ("DBInstanceClass", NEPTUNE_CLASS.to_string()),
        ];
        params.retain(|(name, _)| *name != omitted);
        let case = format!("a cluster that resolves, omitting {case}");
        let (status, body) = call(&router, "CreateDBInstance", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the boundary refusal's HTTP 400, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn an_identifier_scoped_instance_report_refuses_an_instance_nothing_created() {
    let router = emulator();

    // The instance that DOES exist, so the report that resolves is a row of the table.
    expect_ok(
        "the cluster the instance joins",
        call(
            &router,
            "CreateDBCluster",
            &[("DBClusterIdentifier", "emu-npt-report"), ("Engine", NEPTUNE)],
        )
        .await,
    );
    expect_ok(
        "the instance that exists",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-npt-report-1"),
                ("Engine", NEPTUNE),
                ("DBInstanceClass", NEPTUNE_CLASS),
                ("DBClusterIdentifier", "emu-npt-report"),
            ],
        )
        .await,
    );

    for (case, identifier) in [
        ("a name nothing was created under", "emu-npt-no-such-instance"),
        ("a near miss of a stored name", "emu-npt-report-2"),
        ("a single letter", "e"),
        ("the stored name in mixed case, misspelled", "EMU-NPT-Report-9"),
    ] {
        let body = expect_error(
            case,
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", identifier)],
            )
            .await,
            404,
            "DBInstanceNotFound",
        );
        // "This parameter is stored as a lowercase string", so the sentence names the
        // identifier in the spelling the store keys on.
        let expected = format!("DBInstance {} not found.", identifier.to_ascii_lowercase());
        let message = text(&body, "Message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?} in {body}"
        );
    }

    let reported = expect_ok(
        "the instance that exists is still reported",
        call(
            &router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "emu-npt-report-1")],
        )
        .await,
    );
    assert_eq!(
        text(&reported, "DBInstanceIdentifier"),
        "emu-npt-report-1",
        "the report of an instance that exists must carry it, got {reported}"
    );
}

/// The scaling configurations a cluster create is spanned over: `(case, min, max)`, with
/// whether the configuration is one the Neptune engine serves.
///
/// The capacities span the range a Neptune Serverless CLUSTER is configured in — zero
/// capacity up to the published ceiling, in the half-step increments
/// `ServerlessV2ScalingConfiguration$MinCapacity` documents — so the rows are both bounds
/// of that range, values inside them, one past the ceiling, one off the half-step grid and
/// a floor above the ceiling. That range is the one a refusal of a capacity value states;
/// the floor the engine VERSION publishes in `ServerlessV2FeaturesSupport` is a different
/// member, asserted where that report is.
const SCALING_ROWS: &[(&str, &str, &str, bool)] = &[
    ("both bounds of the configurable range", "0.0", "128.0", true),
    ("a range inside the bounds", "2.5", "8.0", true),
    ("a half capacity unit floor", "0.5", "8.0", true),
    ("one capacity unit, floor and ceiling alike", "1.0", "1.0", true),
    ("a ceiling past the published maximum", "1.0", "128.5", false),
    ("a capacity off the half-step grid", "1.2", "8.0", false),
    ("a floor above the ceiling", "8.0", "4.0", false),
    ("a floor above the ceiling, at the bounds", "128.0", "0.5", false),
];

#[tokio::test]
async fn a_cluster_create_resolves_its_subnet_group_before_its_scaling_configuration() {
    let router = emulator();

    for (case, min, max, _) in SCALING_ROWS {
        let absent_group = "emu-npt-no-such-subnet-group";
        let body = expect_error(
            case,
            call(
                &router,
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", "emu-npt-sls-nosng"),
                    ("Engine", NEPTUNE),
                    ("DBSubnetGroupName", absent_group),
                    ("ServerlessV2ScalingConfiguration.MinCapacity", min),
                    ("ServerlessV2ScalingConfiguration.MaxCapacity", max),
                ],
            )
            .await,
            404,
            "DBSubnetGroupNotFoundFault",
        );
        let expected = db_subnet_group_does_not_exist_message(absent_group);
        let message = text(&body, "Message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?} in {body}"
        );
        // Nothing was created under the identifier the refused request named.
        expect_error(
            &format!("{case}: the cluster was not created"),
            call(
                &router,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", "emu-npt-sls-nosng")],
            )
            .await,
            404,
            "DBClusterNotFoundFault",
        );
    }
}

#[tokio::test]
async fn a_neptune_serverless_cluster_is_created_inside_the_published_capacity_range() {
    let router = emulator();
    expect_ok(
        "the subnet group the serverless clusters name",
        create_subnet_group(&router, "emu-npt-sls-subnets").await,
    );

    for (index, (case, min, max, served)) in SCALING_ROWS.iter().enumerate() {
        let identifier = format!("emu-npt-sls-{index}");
        let response = call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", identifier.as_str()),
                ("Engine", NEPTUNE),
                ("DBSubnetGroupName", "emu-npt-sls-subnets"),
                ("ServerlessV2ScalingConfiguration.MinCapacity", min),
                ("ServerlessV2ScalingConfiguration.MaxCapacity", max),
            ],
        )
        .await;
        if !served {
            let body = expect_error(case, response, 400, "InvalidParameterValue");
            let message = text(&body, "Message");
            // A floor above the ceiling is the one case AWS words with a fixed sentence of
            // its own; a value outside the range states the value and the range.
            if min.parse::<f64>().unwrap_or_default() > max.parse::<f64>().unwrap_or_default() {
                assert_eq!(
                    message, SERVERLESS_V2_FLOOR_ABOVE_CEILING_MESSAGE,
                    "case {case:?}: expected the fixed sentence \
                     {SERVERLESS_V2_FLOOR_ABOVE_CEILING_MESSAGE:?}, got {message:?} in {body}"
                );
            } else {
                assert!(
                    message.contains("Serverless v2 capacity value"),
                    "case {case:?}: expected a refusal naming the capacity it would not \
                     take, got {message:?}"
                );
            }
            continue;
        }

        expect_ok(case, response);
        // The configuration the create accepted is read back through the report, from the
        // record the create stored.
        let cluster = describe_one(&router, case, identifier.as_str()).await;
        let scaling = subtrees(&cluster, "ServerlessV2ScalingConfiguration")
            .into_iter()
            .next()
            .unwrap_or_else(|| {
                panic!("case {case:?}: a serverless cluster reports its configuration, got {cluster}")
            });
        assert_eq!(
            (
                text(&scaling, "MinCapacity").as_str(),
                text(&scaling, "MaxCapacity").as_str()
            ),
            (*min, *max),
            "case {case:?}: expected the stored capacity range, got {scaling}"
        );
    }
}

/// The engines whose instances are cluster MEMBERS, and the class each one's members are
/// ordered in: the two sibling control planes' engines, whose own `CreateDBInstance` APIs
/// carry no usable master-user member at all.
const CLUSTER_MEMBER_ENGINES: [(&str, &str); 2] = [("neptune", NEPTUNE_CLASS), ("docdb", "db.r5.large")];

#[tokio::test]
async fn an_instance_of_a_cluster_engine_is_not_created_outside_a_cluster() {
    let router = emulator();

    for (index, (engine, class)) in CLUSTER_MEMBER_ENGINES.iter().enumerate() {
        let identifier = format!("emu-member-only-{index}");
        // Without a cluster there is no master user credential anywhere: the request may
        // carry none for this engine, and no cluster manages one.
        let body = expect_error(
            engine,
            call(
                &router,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", identifier.as_str()),
                    ("Engine", engine),
                    ("DBInstanceClass", class),
                ],
            )
            .await,
            400,
            "InvalidParameterValue",
        );
        let message = text(&body, "Message");
        assert_eq!(
            message, INVALID_MASTER_PASSWORD_MESSAGE,
            "case {engine:?}: expected the fixed sentence \
             {INVALID_MASTER_PASSWORD_MESSAGE:?}, got {message:?} in {body}"
        );
        expect_error(
            engine,
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", identifier.as_str())],
            )
            .await,
            404,
            "DBInstanceNotFound",
        );

        // The same instance joined to a cluster of that engine IS created: the credential
        // lives on the cluster, which is the only place this engine has one.
        let cluster = format!("emu-member-only-cl-{index}");
        expect_ok(
            engine,
            call(
                &router,
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", cluster.as_str()),
                    ("Engine", engine),
                ],
            )
            .await,
        );
        let created = expect_ok(
            engine,
            call(
                &router,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", identifier.as_str()),
                    ("Engine", engine),
                    ("DBInstanceClass", class),
                    ("DBClusterIdentifier", cluster.as_str()),
                ],
            )
            .await,
        );
        assert_eq!(
            text(&created, "DBClusterIdentifier"),
            cluster,
            "case {engine:?}: the created instance must report the cluster it joined, got \
             {created}"
        );
    }

    // The control: an engine whose instances carry a master credential of their own is
    // created outside a cluster, exactly as before.
    expect_ok(
        "an RDS engine, which has no cluster to carry its credential",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-member-only-standalone"),
                ("Engine", "postgres"),
                ("DBInstanceClass", "db.m5.large"),
            ],
        )
        .await,
    );
}

/// The refusal one reference case must be answered with: the wire error `<Code>` and the
/// exact `<Message>`.
struct ExpectedRefusal {
    code: &'static str,
    message: String,
}

/// One emulator over one shared engine: Amazon RDS the way the router mounts it, plus the
/// two services that OWN the resources a cluster create references — Amazon EC2 (VPC
/// security groups) and KMS (the encryption key) — with a client for each over the router's
/// own in-process callback, which is the very seam the resolution under test uses.
struct ReferencedAccount {
    router: MultiServiceRouter,
    ec2: ::aws_sdk_ec2::Client,
    kms: ::aws_sdk_kms::Client,
}

impl ReferencedAccount {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not the subject of these tests; the requests still carry the SigV4
        // credential scope, which is what routes them and names the region.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
            Arc::clone(&engine),
        ));
        let callback = EmbeddedEmulator::from_router(router.clone());
        let ec2 = ::aws_sdk_ec2::Client::from_conf(
            ::aws_sdk_ec2::Config::builder()
                .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
                .region(::aws_sdk_ec2::config::Region::new(REGION.to_string()))
                .endpoint_url(format!("https://ec2.{REGION}.amazonaws.com"))
                .credentials_provider(callback.aws_credentials_provider())
                .http_client(callback.http_client())
                .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
                .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
                .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
                .stalled_stream_protection(
                    ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
                )
                .build(),
        );
        let kms = ::aws_sdk_kms::Client::from_conf(
            ::aws_sdk_kms::Config::builder()
                .behavior_version(::aws_sdk_kms::config::BehaviorVersion::latest())
                .region(::aws_sdk_kms::config::Region::new(REGION.to_string()))
                .endpoint_url(format!("https://kms.{REGION}.amazonaws.com"))
                .credentials_provider(callback.aws_credentials_provider())
                .http_client(callback.http_client())
                .retry_config(::aws_sdk_kms::config::retry::RetryConfig::disabled())
                .timeout_config(::aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
                .identity_cache(::aws_sdk_kms::config::IdentityCache::no_cache())
                .stalled_stream_protection(
                    ::aws_sdk_kms::config::StalledStreamProtectionConfig::disabled(),
                )
                .build(),
        );
        Self { router, ec2, kms }
    }

    /// The security group `ec2:CreateSecurityGroup` mints, as the id EC2 reported.
    async fn create_security_group(&self, name: &str) -> String {
        self.ec2
            .create_security_group()
            .group_name(name)
            .description("a group a DB cluster joins")
            .send()
            .await
            .unwrap_or_else(|error| panic!("EC2 must create the security group {name}: {error:?}"))
            .group_id()
            .expect("CreateSecurityGroup must report the group it minted")
            .to_string()
    }

    /// The key `kms:CreateKey` mints, as the ARN KMS reported.
    async fn create_key(&self) -> String {
        self.kms
            .create_key()
            .description("the key a DB cluster is encrypted under")
            .send()
            .await
            .unwrap_or_else(|error| panic!("KMS must create the key: {error:?}"))
            .key_metadata()
            .and_then(|metadata| metadata.arn())
            .expect("CreateKey must report the key it minted")
            .to_string()
    }
}

#[tokio::test]
async fn a_cluster_create_resolves_the_resources_other_services_own() {
    let account = ReferencedAccount::new();
    let group_id = account.create_security_group("emu-npt-ref-group").await;
    let key_arn = account.create_key().await;

    // A well-formed identifier of each kind that the owning service holds nothing for: EC2
    // minted no such group and KMS no such key, so each resolves to nothing however often
    // it is named.
    let absent_group = "sg-00000000000000fff";
    let absent_key = format!(
        "arn:aws:kms:{REGION}:000000000000:key/00000000-0000-0000-0000-000000000000"
    );

    // One row: the case, the members the create adds beyond its two required ones, and the
    // refusal it must be answered with (`None` for a reference the owning service resolves).
    struct ReferenceCase {
        case: &'static str,
        extra: Vec<(&'static str, String)>,
        refusal: Option<ExpectedRefusal>,
    }

    let rows: [ReferenceCase; 5] = [
        ReferenceCase {
            case: "a security group EC2 minted",
            extra: vec![(
                "VpcSecurityGroupIds.VpcSecurityGroupId.1",
                group_id.clone(),
            )],
            refusal: None,
        },
        ReferenceCase {
            case: "a security group the account does not hold",
            extra: vec![(
                "VpcSecurityGroupIds.VpcSecurityGroupId.1",
                absent_group.to_string(),
            )],
            refusal: Some(ExpectedRefusal {
                code: "InvalidParameterValue",
                message: invalid_security_group_message(absent_group),
            }),
        },
        ReferenceCase {
            case: "one group EC2 minted beside one it did not",
            extra: vec![
                (
                    "VpcSecurityGroupIds.VpcSecurityGroupId.1",
                    group_id.clone(),
                ),
                (
                    "VpcSecurityGroupIds.VpcSecurityGroupId.2",
                    absent_group.to_string(),
                ),
            ],
            refusal: Some(ExpectedRefusal {
                code: "InvalidParameterValue",
                message: invalid_security_group_message(absent_group),
            }),
        },
        ReferenceCase {
            case: "a KMS key the account holds",
            extra: vec![
                ("StorageEncrypted", "true".to_string()),
                ("KmsKeyId", key_arn.clone()),
            ],
            refusal: None,
        },
        ReferenceCase {
            case: "a KMS key the account does not hold",
            extra: vec![
                ("StorageEncrypted", "true".to_string()),
                ("KmsKeyId", absent_key.clone()),
            ],
            refusal: Some(ExpectedRefusal {
                code: "KMSKeyNotAccessibleFault",
                message: kms_key_not_accessible_message(&absent_key),
            }),
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let identifier = format!("emu-npt-ref-{index}");
        let case = row.case;
        let mut params: Vec<(&str, String)> = vec![
            ("DBClusterIdentifier", identifier.clone()),
            ("Engine", NEPTUNE.to_string()),
        ];
        params.extend(row.extra);
        let response = call_in(&account.router, REGION, "CreateDBCluster", &params).await;
        match row.refusal {
            Some(ExpectedRefusal { code, message }) => {
                let body = expect_error(case, response, 400, code);
                let answered = text(&body, "Message");
                assert_eq!(
                    answered, message,
                    "case {case:?}: expected the message {message:?}, got {answered:?} in \
                     {body}"
                );
                // A create the reference refused stored nothing.
                expect_error(
                    case,
                    call_in(
                        &account.router,
                        REGION,
                        "DescribeDBClusters",
                        &[("DBClusterIdentifier", identifier.as_str())],
                    )
                    .await,
                    404,
                    "DBClusterNotFoundFault",
                );
            }
            None => {
                expect_ok(case, response);
            }
        }
    }
}
