// SPDX-License-Identifier: CC0-1.0

//! The `DBInstance` envelope every DB instance operation answers with.
//!
//! One rule is under test: an operation that answers with a `DBInstance` projects
//! EVERY modelled member the service holds state for — from the stored record or from
//! the default the record was created with — and the three envelopes that carry a
//! DBInstance (`CreateDBInstance`, `DescribeDBInstances`, `DeleteDBInstance`) project
//! the same record, so none of them can report less than another.
//!
//! The tables below span what the rule generalises over rather than one recorded
//! instance: the engine families whose default parameter group, option group and
//! license model differ; the four ways an `EngineVersion` resolves against the
//! engine-version catalogue (absent, exact, a major version, a version inside a major
//! version the catalogue publishes) plus the values it refuses; both branches of the
//! `StorageThroughput` rule and the reachable branch of the `DbInstancePort` one (no
//! instance here is a cluster member: `CreateDBInstance` resolves `DBClusterIdentifier`
//! against DB cluster records this service cannot yet hold); and, for every member that
//! has a documented default, the supplied value as well as the default.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the production
//! router uses, and every assertion reads the XML the emulator answered with.

use std::thread::sleep;
use std::time::Duration;

use service_com_amazonaws_rds_amazonrdsv19::state;
use service_com_amazonaws_rds_amazonrdsv19::state::STATUS_TRANSITION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, element_is_empty, emulator, expect_error, expect_ok, optional_text, subtrees, text,
    REGION,
};

/// How long a test waits before expecting the `creating` → `available` transition this
/// service owes to have settled: the span itself plus slack for a loaded test machine.
fn wait_for_settle() {
    sleep(STATUS_TRANSITION_DELAY + Duration::from_millis(200));
}

/// The one `<DBInstance>` subtree of a response.
fn the_instance(case: &str, xml: &str) -> String {
    let found = subtrees(xml, "DBInstance");
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one DBInstance, got {} in {xml}",
        found.len()
    );
    found.into_iter().next().expect("checked just above")
}

/// `CreateDBInstance` with its three required members plus `extra`, as the
/// `<DBInstance>` the create envelope carries.
async fn create_instance(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    engine: &str,
    extra: &[(&str, &str)],
) -> String {
    let mut params: Vec<(&str, String)> = vec![
        ("DBInstanceIdentifier", identifier.to_string()),
        ("Engine", engine.to_string()),
        ("DBInstanceClass", "db.t3.medium".to_string()),
    ];
    params.extend(
        extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    let body = expect_ok(case, call(router, "CreateDBInstance", &params).await);
    the_instance(case, &body)
}

/// `DescribeDBInstances` for one identifier, as the one `<DBInstance>` it reports.
async fn describe_instance(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", identifier.to_string())],
        )
        .await,
    );
    the_instance(case, &body)
}

/// `DeleteDBInstance` without a final snapshot, as the `<DBInstance>` it answers with.
async fn delete_instance(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let body = expect_ok(
        case,
        call(
            router,
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", identifier.to_string()),
                ("SkipFinalSnapshot", "true".to_string()),
            ],
        )
        .await,
    );
    the_instance(case, &body)
}

/// Assert one scalar member of an envelope.
fn assert_member(case: &str, envelope: &str, instance: &str, member: &str, expected: &str) {
    let actual = optional_text(instance, member);
    assert_eq!(
        actual.as_deref(),
        Some(expected),
        "case {case:?}: the {envelope} envelope must report {member} as {expected:?}, \
         got {actual:?}"
    );
}

/// Assert the port an envelope PUBLISHES.
///
/// For an engine whose wire this emulator serves, the published port is the one the core
/// assigned the instance's own engine listener — that is the port a client connects to,
/// and `db_instance_engine_endpoint` is where a session is opened on it — so what holds
/// here is that it is a port at all; every other engine publishes the port the request
/// asked for or the engine's documented default.
fn assert_published_port(
    case: &str,
    envelope: &str,
    instance: &str,
    engine: &str,
    configured: &str,
) {
    if service_com_amazonaws_rds_amazonrdsv19::state::engine_wire(engine).is_some() {
        let actual = optional_text(instance, "Port");
        assert!(
            actual
                .as_deref()
                .is_some_and(|port| port.parse::<u16>().is_ok_and(|port| port > 0)),
            "case {case:?}: the {envelope} envelope must report the port of the \
             instance's engine listener, got {actual:?}"
        );
    } else {
        assert_member(case, envelope, instance, "Port", configured);
    }
}

/// Assert that an envelope carries exactly one `<element>` — the check an EMPTY list or
/// an empty structure needs, which carries no text of its own.
fn assert_element_present(case: &str, envelope: &str, instance: &str, element: &str) {
    let found = subtrees(instance, element);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: the {envelope} envelope must report exactly one {element} \
         element, got {found:?}"
    );
}

/// The CA an instance created without a `CACertificateIdentifier` is issued under.
const DEFAULT_CA_CERTIFICATE: &str = "rds-ca-rsa2048-g1";

/// `CACertificateIdentifier` as `envelope` reports it for an instance created under
/// `issued_under`.
///
/// The member names the CA of the certificate the instance HAS, so an envelope that
/// answers an accepted delete reports none: the certificate goes with the instance the
/// delete has started removing, and the member comes back empty. `CertificateDetails`
/// still describes that certificate, which is why `CAIdentifier` is asserted separately.
fn expected_ca_certificate_identifier<'a>(envelope: &str, issued_under: &'a str) -> &'a str {
    match envelope {
        "delete" => "",
        _ => issued_under,
    }
}

/// The year of an ISO-8601 timestamp the emulator reported.
fn year_of(case: &str, member: &str, value: &str) -> i32 {
    value
        .get(..4)
        .and_then(|year| year.parse().ok())
        .unwrap_or_else(|| {
            panic!("case {case:?}: {member} must be an ISO-8601 timestamp, got {value:?}")
        })
}

/// One row of the engine table: an engine, the `EngineVersion` the request names (if
/// any), and what the service must report having provisioned for it.
struct EngineCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    /// `EngineVersion` as the request names it, or `None` when it names none.
    requested_version: Option<&'static str>,
    /// The catalogue version the service provisioned.
    engine_version: &'static str,
    /// `default.` + that version's DB parameter group family.
    db_parameter_group: &'static str,
    /// `default:<engine>-<major engine version>`, the version's dots as hyphens.
    option_group: &'static str,
    /// The license model of the engine, from the value list
    /// `CreateDBInstanceMessage$LicenseModel` publishes.
    license_model: &'static str,
    /// The engine's default port.
    port: &'static str,
}

/// The engines this table spans: one per documented license-model row, one per
/// parameter-group-family shape (`mysql8.4`, `postgres16`, `sqlserver-ee-16.0`,
/// `oracle-se2-19`, `db2-se-11.5`), and the four ways an `EngineVersion` resolves.
const ENGINE_CASES: &[EngineCase] = &[
    EngineCase {
        case: "mysql, no version named",
        identifier: "emu-env-mysql-default",
        engine: "mysql",
        requested_version: None,
        engine_version: "8.4.4",
        db_parameter_group: "default.mysql8.4",
        option_group: "default:mysql-8-4",
        license_model: "general-public-license",
        port: "3306",
    },
    EngineCase {
        case: "mysql, an exact version that is not its engine's default",
        identifier: "emu-env-mysql-8-0-40",
        engine: "mysql",
        requested_version: Some("8.0.40"),
        engine_version: "8.0.40",
        db_parameter_group: "default.mysql8.0",
        option_group: "default:mysql-8-0",
        license_model: "general-public-license",
        port: "3306",
    },
    EngineCase {
        case: "postgres, a major version named",
        identifier: "emu-env-postgres-16",
        engine: "postgres",
        requested_version: Some("16"),
        engine_version: "16.13",
        db_parameter_group: "default.postgres16",
        option_group: "default:postgres-16",
        license_model: "postgresql-license",
        port: "5432",
    },
    EngineCase {
        case: "mariadb, an exact version named",
        identifier: "emu-env-mariadb-11-4-4",
        engine: "mariadb",
        requested_version: Some("11.4.4"),
        engine_version: "11.4.4",
        db_parameter_group: "default.mariadb11.4",
        option_group: "default:mariadb-11-4",
        license_model: "general-public-license",
        port: "3306",
    },
    EngineCase {
        case: "sqlserver-ee, a major version whose form carries two digits",
        identifier: "emu-env-sqlserver-ee-16",
        engine: "sqlserver-ee",
        requested_version: Some("16.00"),
        engine_version: "16.00.4165.4.v1",
        db_parameter_group: "default.sqlserver-ee-16.0",
        option_group: "default:sqlserver-ee-16-00",
        license_model: "license-included",
        port: "1433",
    },
    EngineCase {
        case: "oracle-se2, no version named",
        identifier: "emu-env-oracle-se2",
        engine: "oracle-se2",
        requested_version: None,
        engine_version: "19.0.0.0.ru-2024-10.rur-2024-10.r1",
        db_parameter_group: "default.oracle-se2-19",
        option_group: "default:oracle-se2-19",
        license_model: "license-included",
        port: "1521",
    },
    EngineCase {
        case: "db2-se, no version named",
        identifier: "emu-env-db2-se",
        engine: "db2-se",
        requested_version: None,
        engine_version: "12.1.4.0.sb00080714.r1",
        db_parameter_group: "default.db2-se-12.1",
        option_group: "default:db2-se-12-1",
        license_model: "bring-your-own-license",
        port: "50000",
    },
    EngineCase {
        case: "aurora-postgresql, no version named",
        identifier: "emu-env-aurora-postgresql",
        engine: "aurora-postgresql",
        requested_version: None,
        engine_version: "17.9",
        db_parameter_group: "default.aurora-postgresql17",
        option_group: "default:aurora-postgresql-17",
        license_model: "postgresql-license",
        port: "5432",
    },
];

/// Every member the projection reports for an instance whatever the request named: a
/// scalar the record always holds, or a list that is EMPTY when the instance has no
/// such association. `DBSubnetGroup` is among them: every DB instance is placed in a DB
/// subnet group — the one the request named, or the account's `default` group — so every
/// envelope reports one. `PendingModifiedValues` is among them too: "specific changes are
/// identified by subelements", so an instance with nothing pending carries the element with
/// no subelement inside it.
const ALWAYS_PROJECTED: &[&str] = &[
    "DBInstanceIdentifier",
    "DBInstanceClass",
    "Engine",
    "DBInstanceStatus",
    "Endpoint",
    "InstanceCreateTime",
    "PreferredBackupWindow",
    "BackupRetentionPeriod",
    "DBSecurityGroups",
    "VpcSecurityGroups",
    "DBParameterGroups",
    "OptionGroupMemberships",
    "AvailabilityZone",
    "DBSubnetGroup",
    "PreferredMaintenanceWindow",
    "UpgradeRolloutOrder",
    "PendingModifiedValues",
    "VPCNetworkingEnabled",
    "MultiAZ",
    "EngineVersion",
    "AutoMinorVersionUpgrade",
    "ReadReplicaDBInstanceIdentifiers",
    "LicenseModel",
    "StorageThroughput",
    "PubliclyAccessible",
    "StorageType",
    "DbInstancePort",
    "StorageEncrypted",
    "DbiResourceId",
    "CACertificateIdentifier",
    "CertificateDetails",
    "DomainMemberships",
    "CopyTagsToSnapshot",
    "MonitoringInterval",
    "DBInstanceArn",
    "IAMDatabaseAuthenticationEnabled",
    "DatabaseInsightsMode",
    "PerformanceInsightsEnabled",
    "DeletionProtection",
    "AssociatedRoles",
    "TagList",
    "CustomerOwnedIpEnabled",
    "NetworkType",
    "ActivityStreamStatus",
    "BackupTarget",
    "DedicatedLogVolume",
    "IsStorageConfigUpgradeAvailable",
    "EngineLifecycleSupport",
];

/// The defaults every instance in [`ENGINE_CASES`] is created with, since none of their
/// requests names any of these members. Each value is the one its member documents, or
/// the only member of the enum that describes an instance nothing configured.
const DEFAULTS: &[(&str, &str)] = &[
    ("PreferredBackupWindow", "07:00-07:30"),
    ("PreferredMaintenanceWindow", "mon:08:00-mon:08:30"),
    ("MonitoringInterval", "0"),
    ("PerformanceInsightsEnabled", "false"),
    ("IAMDatabaseAuthenticationEnabled", "false"),
    ("DatabaseInsightsMode", "standard"),
    ("DedicatedLogVolume", "false"),
    ("CustomerOwnedIpEnabled", "false"),
    ("NetworkType", "IPV4"),
    ("BackupTarget", "region"),
    (
        "EngineLifecycleSupport",
        "open-source-rds-extended-support",
    ),
    ("ActivityStreamStatus", "stopped"),
    ("UpgradeRolloutOrder", "second"),
    ("IsStorageConfigUpgradeAvailable", "false"),
    // `StorageType` "Default: io1, if the Iops parameter is specified", and the recorded
    // service provisions general purpose SSD for a request that specifies neither;
    // `StorageThroughput` "applies only to the gp3 storage type", so this instance is
    // provisioned none.
    ("StorageType", "gp2"),
    ("StorageThroughput", "0"),
    // Every DB instance runs inside a virtual private cloud.
    ("VPCNetworkingEnabled", "true"),
    // An instance that belongs to no DB cluster has no cluster-member port.
    ("DbInstancePort", "0"),
];

#[tokio::test]
async fn every_db_instance_envelope_projects_the_whole_stored_record() {
    let router = emulator();

    for row in ENGINE_CASES {
        let extra: Vec<(&str, &str)> = row
            .requested_version
            .map(|version| vec![("EngineVersion", version)])
            .unwrap_or_default();
        let created = create_instance(&router, row.case, row.identifier, row.engine, &extra).await;
        wait_for_settle();
        let described = describe_instance(&router, row.case, row.identifier).await;
        // The same record through the third envelope, which answers with the instance
        // it removed.
        let deleted = delete_instance(&router, row.case, row.identifier).await;

        for (envelope, instance) in [
            ("create", &created),
            ("describe", &described),
            ("delete", &deleted),
        ] {
            for member in ALWAYS_PROJECTED {
                assert_element_present(row.case, envelope, instance, member);
            }
            // An instance created without naming a DB subnet group is placed in the
            // account's `default` group, and reports THAT group.
            assert_member(
                row.case,
                envelope,
                instance,
                "DBSubnetGroupName",
                "default",
            );
            // `PendingModifiedValues` carries one subelement per pending change, and
            // nothing is pending for an instance nothing has modified: the element is
            // there (above) with nothing inside it.
            assert!(
                subtrees(instance, "PendingModifiedValues")
                    .iter()
                    .all(|pending| element_is_empty(pending, "PendingModifiedValues")),
                "case {:?}: the {envelope} envelope must report no pending change \
                 for an instance with no pending change, got {instance}",
                row.case
            );

            assert_member(row.case, envelope, instance, "Engine", row.engine);
            assert_member(
                row.case,
                envelope,
                instance,
                "EngineVersion",
                row.engine_version,
            );
            assert_member(
                row.case,
                envelope,
                instance,
                "DBParameterGroupName",
                row.db_parameter_group,
            );
            assert_member(
                row.case,
                envelope,
                instance,
                "ParameterApplyStatus",
                "in-sync",
            );
            assert_member(
                row.case,
                envelope,
                instance,
                "OptionGroupName",
                row.option_group,
            );
            assert_member(row.case, envelope, instance, "LicenseModel", row.license_model);
            assert_published_port(row.case, envelope, instance, row.engine, row.port);
            for (member, expected) in DEFAULTS {
                assert_member(row.case, envelope, instance, member, expected);
            }
            assert_member(
                row.case,
                envelope,
                instance,
                "CACertificateIdentifier",
                expected_ca_certificate_identifier(envelope, DEFAULT_CA_CERTIFICATE),
            );

            // The server certificate is the one the CA identifier names, and it expires
            // a year after the instance was created.
            assert_member(
                row.case,
                envelope,
                instance,
                "CAIdentifier",
                DEFAULT_CA_CERTIFICATE,
            );
            let created_at = text(instance, "InstanceCreateTime");
            let valid_till = text(instance, "ValidTill");
            assert!(
                valid_till > created_at,
                "case {:?}: the {envelope} envelope must report a ValidTill after the \
                 instance was created, got {valid_till:?} and {created_at:?}",
                row.case
            );
            assert_eq!(
                year_of(row.case, "ValidTill", &valid_till),
                year_of(row.case, "InstanceCreateTime", &created_at) + 1,
                "case {:?}: the {envelope} envelope must report a ValidTill one year \
                 after InstanceCreateTime, got {valid_till:?} and {created_at:?}",
                row.case
            );

            // The lists an instance with no such association carries: present and
            // empty, which is what says it has none. `VpcSecurityGroups` is NOT one of
            // them: an instance always belongs to at least one security group — its VPC's
            // default group where the request named none — which the next block asserts.
            for member in [
                "DBSecurityGroups",
                "AssociatedRoles",
                "DomainMemberships",
                "ReadReplicaDBInstanceIdentifiers",
                "TagList",
            ] {
                let element = subtrees(instance, member);
                let reported = element.first().map(String::as_str).unwrap_or_default();
                assert!(
                    reported.ends_with("/>") || reported == format!("<{member}></{member}>"),
                    "case {:?}: the {envelope} envelope must report {member} as an empty \
                     list, got {reported:?}",
                    row.case
                );
            }

            // An instance whose request named no security group reports the one group RDS
            // placed it in: its VPC's default group, in the `active` status.
            let groups = subtrees(instance, "VpcSecurityGroupMembership");
            assert_eq!(
                groups.len(),
                1,
                "case {:?}: the {envelope} envelope must report the one security group the \
                 instance belongs to, got {groups:?}",
                row.case
            );
            let membership = groups.first().map(String::as_str).unwrap_or_default();
            assert_member(
                row.case,
                envelope,
                membership,
                "VpcSecurityGroupId",
                &state::default_security_group_id(REGION),
            );
            assert_member(row.case, envelope, membership, "Status", "active");
        }
    }
}

#[tokio::test]
async fn a_supplied_member_is_reported_from_the_store_rather_than_defaulted() {
    let router = emulator();
    let identifier = "emu-env-supplied";
    // A `DBSubnetGroupName`, a `DBParameterGroupName` and an `OptionGroupName` are RESOLVED
    // against the store rather than echoed, so the groups this row names have to exist
    // before an instance can reference them.
    expect_ok(
        "the named parameter group is created first",
        call(
            &router,
            "CreateDBParameterGroup",
            &[
                ("DBParameterGroupName", "custom-mysql80".to_string()),
                ("DBParameterGroupFamily", "mysql8.0".to_string()),
                ("Description", "a-group-under-test".to_string()),
            ],
        )
        .await,
    );
    expect_ok(
        "the named option group is created first",
        call(
            &router,
            "CreateOptionGroup",
            &[
                ("OptionGroupName", "custom-option-group".to_string()),
                ("EngineName", "mysql".to_string()),
                ("MajorEngineVersion", "8.0".to_string()),
                ("OptionGroupDescription", "a-group-under-test".to_string()),
            ],
        )
        .await,
    );
    expect_ok(
        "the named subnet group is created first",
        call(
            &router,
            "CreateDBSubnetGroup",
            &[
                ("DBSubnetGroupName", "emu-subnets".to_string()),
                ("DBSubnetGroupDescription", "two subnets".to_string()),
                (
                    "SubnetIds.SubnetIdentifier.1",
                    "subnet-0a1b2c3d".to_string(),
                ),
                (
                    "SubnetIds.SubnetIdentifier.2",
                    "subnet-1b2c3d4e".to_string(),
                ),
            ],
        )
        .await,
    );
    // The other branch of every default the table above takes, plus the members whose
    // values are lists.
    let supplied: &[(&str, &str)] = &[
        ("EngineVersion", "8.0.37"),
        ("DBParameterGroupName", "custom-mysql80"),
        ("OptionGroupName", "custom-option-group"),
        ("CACertificateIdentifier", "rds-ca-ecc384-g1"),
        ("PreferredBackupWindow", "02:00-02:30"),
        ("PreferredMaintenanceWindow", "wed:03:00-wed:03:30"),
        ("MonitoringInterval", "60"),
        ("EnablePerformanceInsights", "true"),
        ("EnableIAMDatabaseAuthentication", "true"),
        ("DatabaseInsightsMode", "advanced"),
        ("DedicatedLogVolume", "true"),
        ("EnableCustomerOwnedIp", "true"),
        ("NetworkType", "DUAL"),
        ("BackupTarget", "outposts"),
        (
            "EngineLifecycleSupport",
            "open-source-rds-extended-support-disabled",
        ),
        ("LicenseModel", "general-public-license"),
        // A non-gp3 storage type provisions no separate throughput.
        ("StorageType", "gp2"),
        ("DBSubnetGroupName", "emu-subnets"),
        // `DBSecurityGroups` is NOT among them: this instance is placed in a VPC, and a DB
        // security group — the EC2-Classic association — can't be given to a VPC DB
        // instance through this API version, which `create_path_refusal_wording` pins.
        ("VpcSecurityGroupIds.VpcSecurityGroupId.1", "sg-00000001"),
        ("VpcSecurityGroupIds.VpcSecurityGroupId.2", "sg-00000002"),
        ("Domain", "d-9067d45678"),
        ("Tags.Tag.1.Key", "team"),
        ("Tags.Tag.1.Value", "platform"),
    ];
    let case = "every member supplied";
    let created = create_instance(&router, case, identifier, "mysql", supplied).await;
    wait_for_settle();
    let described = describe_instance(&router, case, identifier).await;

    for (envelope, instance) in [("create", &created), ("describe", &described)] {
        for (member, expected) in [
            ("EngineVersion", "8.0.37"),
            ("DBParameterGroupName", "custom-mysql80"),
            ("OptionGroupName", "custom-option-group"),
            ("CACertificateIdentifier", "rds-ca-ecc384-g1"),
            ("CAIdentifier", "rds-ca-ecc384-g1"),
            ("PreferredBackupWindow", "02:00-02:30"),
            ("PreferredMaintenanceWindow", "wed:03:00-wed:03:30"),
            ("MonitoringInterval", "60"),
            ("PerformanceInsightsEnabled", "true"),
            ("IAMDatabaseAuthenticationEnabled", "true"),
            ("DatabaseInsightsMode", "advanced"),
            ("DedicatedLogVolume", "true"),
            ("CustomerOwnedIpEnabled", "true"),
            ("NetworkType", "DUAL"),
            ("BackupTarget", "outposts"),
            (
                "EngineLifecycleSupport",
                "open-source-rds-extended-support-disabled",
            ),
            ("StorageType", "gp2"),
            ("StorageThroughput", "0"),
            ("DBSubnetGroupName", "emu-subnets"),
            ("SubnetGroupStatus", "Complete"),
            ("Domain", "d-9067d45678"),
        ] {
            assert_member(case, envelope, instance, member, expected);
        }
        // A VPC DB instance holds no DB security group, so the element is reported EMPTY:
        // the list says the instance is a member of none rather than being absent.
        let security_groups = subtrees(instance, "DBSecurityGroup");
        assert!(
            security_groups.is_empty(),
            "case {case:?}: the {envelope} envelope of a VPC DB instance must report no \
             DBSecurityGroup membership, got {security_groups:?}"
        );
        // The membership lists carry one entry per supplied id, each with the status of a
        // membership that is in effect.
        let vpc_groups = subtrees(instance, "VpcSecurityGroupMembership");
        assert_eq!(
            vpc_groups.len(),
            2,
            "case {case:?}: the {envelope} envelope must report one \
             VpcSecurityGroupMembership per supplied id, got {vpc_groups:?}"
        );
        for (membership, expected) in vpc_groups.iter().zip(["sg-00000001", "sg-00000002"]) {
            assert_member(case, envelope, membership, "VpcSecurityGroupId", expected);
            assert_member(case, envelope, membership, "Status", "active");
        }
        let domains = subtrees(instance, "DomainMembership");
        assert_eq!(
            domains.len(),
            1,
            "case {case:?}: the {envelope} envelope must report the domain the instance \
             joined, got {domains:?}"
        );
    }
}

#[tokio::test]
async fn an_instance_that_belongs_to_no_cluster_reports_no_cluster_member_port() {
    let router = emulator();
    let case = "an explicit port, no cluster";
    let identifier = "emu-env-cluster-member";
    // `DBClusterIdentifier` is NOT supplied: `CreateDBInstance` resolves it against this
    // service's DB cluster records before admitting the instance, and no cluster was
    // created in this router, so this instance belongs to none and the refusal for one
    // that names a cluster nothing created is pinned in `docdb_query_surface` (the
    // admitted-member path is pinned in `db_clusters`). That is what this test reads back:
    // the port the create named is the port the instance listens on, while
    // `DbInstancePort` — the CLUSTER MEMBER's port, "if the DB instance is part of a DB
    // cluster, this can be a different port than the DB cluster port" — stays zero,
    // whatever port was asked for.
    let created = create_instance(
        &router,
        case,
        identifier,
        "aurora-mysql",
        &[("Port", "13306")],
    )
    .await;
    wait_for_settle();
    let described = describe_instance(&router, case, identifier).await;
    for (envelope, instance) in [("create", &created), ("describe", &described)] {
        assert_member(case, envelope, instance, "Port", "13306");
        assert_member(case, envelope, instance, "DbInstancePort", "0");
    }
}

/// One row of the engine-version resolution table: what a request names, and the
/// version the service provisions for it.
struct VersionCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    requested: Option<&'static str>,
    /// The version the service must report, or `None` when the request must be refused.
    provisioned: Option<&'static str>,
}

/// Every branch of the resolution: no value, an exact version (default or not), a major
/// version, a version the catalogue publishes no entry for even though its major version
/// IS published (`8.0.99` is not `8.0`, which the real service refuses: the recorded
/// `CreateDBInstance` answers 400 `InvalidParameterCombination` "Cannot find version
/// 8.0.99 for mysql"), a major version the catalogue publishes no AVAILABLE version for
/// (`aurora-postgresql` 11 is deprecated), a version no major version matches, and an
/// engine the catalogue publishes no version for at all.
const VERSION_CASES: &[VersionCase] = &[
    VersionCase {
        case: "no version named",
        identifier: "emu-ver-mysql-none",
        engine: "mysql",
        requested: None,
        provisioned: Some("8.4.4"),
    },
    VersionCase {
        case: "a major version named",
        identifier: "emu-ver-mysql-80",
        engine: "mysql",
        requested: Some("8.0"),
        provisioned: Some("8.0.40"),
    },
    VersionCase {
        case: "an exact version that is not the default of its major version",
        identifier: "emu-ver-mysql-8037",
        engine: "mysql",
        requested: Some("8.0.37"),
        provisioned: Some("8.0.37"),
    },
    VersionCase {
        case: "a version the catalogue publishes no entry for, inside a published major version",
        identifier: "emu-ver-mysql-8099",
        engine: "mysql",
        requested: Some("8.0.99"),
        provisioned: None,
    },
    VersionCase {
        case: "a version named in another case",
        identifier: "emu-ver-mysql-case",
        engine: "mysql",
        requested: Some("8.4.3"),
        provisioned: Some("8.4.3"),
    },
    VersionCase {
        case: "a major version whose only versions are deprecated",
        identifier: "emu-ver-aurora-pg-11",
        engine: "aurora-postgresql",
        requested: Some("11"),
        provisioned: None,
    },
    VersionCase {
        case: "a deprecated version named exactly",
        identifier: "emu-ver-aurora-pg-1121",
        engine: "aurora-postgresql",
        requested: Some("11.21"),
        provisioned: None,
    },
    // The same two branches on the other engine family whose major version is past its
    // community end of life: the `mysql` 5.7 community release and the RDS Extended
    // Support builds superseded before this catalogue was published are deprecated, so
    // neither can be provisioned, while the Extended Support builds the family is still
    // offered — the ones this report publishes as available — are provisioned like any
    // other build, which is what Extended Support is for.
    VersionCase {
        case: "a major version in extended support, provisioned at its latest build",
        identifier: "emu-ver-mysql-57",
        engine: "mysql",
        requested: Some("5.7"),
        provisioned: Some("5.7.44-rds.20260212"),
    },
    VersionCase {
        case: "an extended-support build named exactly",
        identifier: "emu-ver-mysql-5744",
        engine: "mysql",
        requested: Some("5.7.44-rds.20250213"),
        provisioned: Some("5.7.44-rds.20250213"),
    },
    VersionCase {
        case: "the deprecated community release the extended-support line continues",
        identifier: "emu-ver-mysql-5744-community",
        engine: "mysql",
        requested: Some("5.7.44"),
        provisioned: None,
    },
    VersionCase {
        case: "a superseded extended-support build named exactly",
        identifier: "emu-ver-mysql-5744-old",
        engine: "mysql",
        requested: Some("5.7.44-rds.20240408"),
        provisioned: None,
    },
    VersionCase {
        case: "a version of no major version the engine has",
        identifier: "emu-ver-mysql-999",
        engine: "mysql",
        requested: Some("9.9.9"),
        provisioned: None,
    },
    VersionCase {
        case: "a major version of postgres",
        identifier: "emu-ver-postgres-17",
        engine: "postgres",
        requested: Some("17"),
        provisioned: Some("17.9"),
    },
    VersionCase {
        case: "a postgres version the catalogue publishes no entry for",
        identifier: "emu-ver-postgres-1699",
        engine: "postgres",
        requested: Some("16.99"),
        provisioned: None,
    },
    VersionCase {
        case: "an aurora major version",
        identifier: "emu-ver-aurora-mysql",
        engine: "aurora-mysql",
        requested: Some("8.0"),
        provisioned: Some("8.0.mysql_aurora.3.10.3"),
    },
    VersionCase {
        case: "an engine the catalogue publishes no version for",
        identifier: "emu-ver-custom-oracle",
        engine: "custom-oracle-ee",
        requested: Some("19.my_cev1"),
        provisioned: Some("19.my_cev1"),
    },
];

#[tokio::test]
async fn the_engine_version_is_resolved_through_the_catalogue() {
    let router = emulator();

    for row in VERSION_CASES {
        let mut params: Vec<(&str, String)> = vec![
            ("DBInstanceIdentifier", row.identifier.to_string()),
            ("Engine", row.engine.to_string()),
            ("DBInstanceClass", "db.t3.medium".to_string()),
        ];
        if let Some(requested) = row.requested {
            params.push(("EngineVersion", requested.to_string()));
        }
        let response = call(&router, "CreateDBInstance", &params).await;
        match row.provisioned {
            Some(expected) => {
                let created = the_instance(row.case, &expect_ok(row.case, response));
                assert_member(row.case, "create", &created, "EngineVersion", expected);
                wait_for_settle();
                let described = describe_instance(&router, row.case, row.identifier).await;
                assert_member(row.case, "describe", &described, "EngineVersion", expected);
            }
            None => {
                // No modelled error covers a version the service cannot provision, so
                // it is refused with the query error code the real service answers —
                // `InvalidParameterCombination` — in a message naming the engine and
                // the version that cannot be found for it.
                let body =
                    expect_error(row.case, response, 400, "InvalidParameterCombination");
                let message = text(&body, "Message");
                assert!(
                    message.contains(row.engine)
                        && message.contains(row.requested.unwrap_or_default()),
                    "case {:?}: the refusal must name the engine and the version, \
                     got {message:?}",
                    row.case
                );
            }
        }
    }
}

#[tokio::test]
async fn modifying_an_instance_resolves_the_new_engine_version_too() {
    let router = emulator();
    let case = "modify to another version of the engine";
    let identifier = "emu-env-modified";
    create_instance(&router, case, identifier, "postgres", &[("EngineVersion", "15.8")]).await;
    wait_for_settle();

    // The rows run in order against one instance, ASCENDING through the catalogue: an engine
    // version change is an upgrade, so each row names a build later than the one the row
    // before it left the instance on. What each row asserts is the RESOLUTION — an exact
    // build is taken as named, a major version resolves to that family's own build.
    let rows: &[(&str, &str, &str)] = &[
        ("a major version", "15", "15.17"),
        ("another major version", "16", "16.13"),
        ("an exact version", "17.2", "17.2"),
    ];
    for (row_case, requested, expected) in rows {
        let modified = expect_ok(
            row_case,
            call(
                &router,
                "ModifyDBInstance",
                &[
                    ("DBInstanceIdentifier", identifier.to_string()),
                    ("ApplyImmediately", "true".to_string()),
                    ("EngineVersion", (*requested).to_string()),
                ],
            )
            .await,
        );
        let modified = the_instance(row_case, &modified);
        // The version the modification resolved is what it reports as PENDING: a
        // modification is applied when the instance finishes modifying, so until then the
        // instance reports the version it is RUNNING (see `pending_modified_values`).
        let staged = subtrees(&modified, "PendingModifiedValues")
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {row_case:?}: expected a pending element, got {modified}"));
        assert_member(row_case, "modify", &staged, "EngineVersion", expected);
        wait_for_settle();
        let described = describe_instance(&router, row_case, identifier).await;
        assert_member(row_case, "describe", &described, "EngineVersion", expected);
    }

    // And a version it cannot provision is refused, leaving the stored one in place.
    let body = expect_error(
        "modify to an unprovisionable version",
        call(
            &router,
            "ModifyDBInstance",
            &[
                ("DBInstanceIdentifier", identifier.to_string()),
                ("EngineVersion", "42.0".to_string()),
            ],
        )
        .await,
        400,
        "InvalidParameterCombination",
    );
    assert!(
        text(&body, "Message").contains("42.0"),
        "the refusal must name the version it cannot provision, got {body}"
    );
    let described = describe_instance(&router, "after the refusal", identifier).await;
    assert_member(
        "after the refusal",
        "describe",
        &described,
        "EngineVersion",
        // The build the last accepted row left the instance on.
        "17.2",
    );
}
