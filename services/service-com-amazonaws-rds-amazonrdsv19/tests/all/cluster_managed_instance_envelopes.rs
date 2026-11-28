// SPDX-License-Identifier: CC0-1.0

//! The `DBInstance` envelope a MEMBER of a DB cluster that manages its storage answers with.
//!
//! The rule under test: a DB instance that belongs to a cluster which manages its storage,
//! port, master user and initial database reports THAT CLUSTER's values on every envelope
//! that carries it, and none of its own. `CreateDBInstance` states it member by member —
//! `AllocatedStorage` and `StorageType` "doesn't apply to Amazon Aurora DB instances. Storage
//! is managed by the DB cluster", `Port` "doesn't apply to Aurora DB instances. The port
//! number is managed by the cluster", `MasterUsername` "doesn't apply to Amazon Aurora DB
//! instances. The name for the master user is managed by the DB cluster" — so such a member
//! reports the cluster's allocated storage on the cluster's storage type, no provisioned
//! throughput (`StorageThroughput` "applies only to the gp3 storage type"), no instance-level
//! port, the cluster's master user and database name, and the cluster's encryption type
//! (`StorageEncryptionType`: "`sse-rds` - encrypted using an Amazon Web Services owned KMS
//! key", "`sse-kms` - encrypted using a customer managed KMS key or Amazon Web Services
//! managed KMS key"). Every instance, member or not, reports whether it runs in a virtual
//! private cloud and whether an HTTP endpoint answers for it, and reports
//! `PendingModifiedValues` with one subelement per pending change — so an instance with
//! nothing pending carries that element empty.
//!
//! The rows span what the rule generalises over rather than one recorded member: both Aurora
//! engines and a sibling control plane's engine, a cluster created with a database name and
//! one created without, a cluster on the default storage type of its engine and one on that
//! engine's other published type, an unencrypted cluster and an encrypted one, a cluster
//! whose Data API HTTP endpoint is on and clusters left at the documented default, and both
//! published bounds of the member's `PromotionTier`. The control row is an instance that
//! belongs to NO cluster, whose own request carried every one of those settings and which
//! must keep reporting them.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so each assertion
//! reads the XML this emulator itself answered with.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, emulator, expect_ok, optional_text, subtrees, texts, wait_for_settle,
};

/// One cluster-and-member row: how the cluster was created, how the member joined it, and
/// what the member must then report.
struct MemberCase {
    case: &'static str,
    cluster: &'static str,
    instance: &'static str,
    engine: &'static str,
    /// The parameters the cluster create carries beyond `DBClusterIdentifier` and `Engine`.
    cluster_extra: &'static [(&'static str, &'static str)],
    /// The parameters the member create carries beyond the three required ones.
    member_extra: &'static [(&'static str, &'static str)],
    /// `StorageType`: the type the cluster manages its storage on.
    storage_type: &'static str,
    /// `MasterUsername`: the cluster's master user, or `None` for a cluster created without
    /// one, whose member names none either.
    master_username: Option<&'static str>,
    /// `DBName`: the database the cluster was created with, or `None` for a cluster created
    /// without one.
    db_name: Option<&'static str>,
    /// `StorageEncryptionType`: how the cluster's volume is protected.
    storage_encryption_type: &'static str,
    /// `PromotionTier`: the failover order the member was created at.
    promotion_tier: &'static str,
}

/// The compute class every row's member is ordered in.
const CLASS: &str = "db.r5.large";

/// `AllocatedStorage` a member of an Aurora-style cluster reports: the size such a cluster
/// publishes for storage that "adjusts as needed"
/// (`state::ELASTIC_CLUSTER_ALLOCATED_STORAGE`), which is the size the cluster itself
/// reports.
const CLUSTER_ALLOCATED_STORAGE: &str = "1";

const MEMBER_CASES: &[MemberCase] = &[
    MemberCase {
        case: "an aurora-mysql writer of a cluster with a database and a master user",
        cluster: "emu-cm-mysql",
        instance: "emu-cm-mysql-1",
        engine: "aurora-mysql",
        cluster_extra: &[
            ("MasterUsername", "clusteradmin"),
            ("MasterUserPassword", "a-strong-password-1"),
            ("DatabaseName", "appdb"),
        ],
        member_extra: &[("PromotionTier", "0")],
        // `CreateDBCluster$StorageType` "Default: Aurora DB clusters - aurora".
        storage_type: "aurora",
        master_username: Some("clusteradmin"),
        db_name: Some("appdb"),
        // Aurora storage is encrypted at rest under a key the service owns unless the
        // request named one of its own.
        storage_encryption_type: "sse-rds",
        promotion_tier: "0",
    },
    MemberCase {
        case: "an aurora-postgresql member of an encrypted cluster with the Data API on",
        cluster: "emu-cm-pg",
        instance: "emu-cm-pg-1",
        engine: "aurora-postgresql",
        cluster_extra: &[
            ("MasterUsername", "pgadmin"),
            ("MasterUserPassword", "a-strong-password-2"),
            ("StorageEncrypted", "true"),
            ("EnableHttpEndpoint", "true"),
            ("StorageType", "aurora-iopt1"),
        ],
        member_extra: &[("PromotionTier", "15")],
        storage_type: "aurora-iopt1",
        master_username: Some("pgadmin"),
        db_name: None,
        storage_encryption_type: "sse-kms",
        promotion_tier: "15",
    },
    MemberCase {
        case: "a document-engine member of a cluster created without a master user",
        cluster: "emu-cm-docdb",
        instance: "emu-cm-docdb-1",
        engine: "docdb",
        cluster_extra: &[],
        member_extra: &[],
        // The sibling control plane's own `StorageType`: "Valid values for storage type -
        // standard | iopt1 … Default value is standard".
        storage_type: "standard",
        master_username: None,
        db_name: None,
        storage_encryption_type: "sse-rds",
        // `PromotionTier` "Default: 1" for a member that asked for none.
        promotion_tier: "1",
    },
];

/// `CreateDBCluster` for one row.
async fn create_cluster(router: &MultiServiceRouter, row: &MemberCase) {
    let mut params: Vec<(&str, String)> = vec![
        ("DBClusterIdentifier", row.cluster.to_string()),
        ("Engine", row.engine.to_string()),
    ];
    params.extend(
        row.cluster_extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    expect_ok(row.case, call(router, "CreateDBCluster", &params).await);
}

/// `CreateDBInstance` for one row's member, as the one `<DBInstance>` it answers with.
async fn create_member(router: &MultiServiceRouter, row: &MemberCase) -> String {
    let mut params: Vec<(&str, String)> = vec![
        ("DBInstanceIdentifier", row.instance.to_string()),
        ("Engine", row.engine.to_string()),
        ("DBInstanceClass", CLASS.to_string()),
        ("DBClusterIdentifier", row.cluster.to_string()),
    ];
    params.extend(
        row.member_extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    let body = expect_ok(row.case, call(router, "CreateDBInstance", &params).await);
    the_instance(row.case, &body)
}

/// The one `<DBInstance>` subtree `body` carries.
fn the_instance(case: &str, body: &str) -> String {
    let reported = subtrees(body, "DBInstance");
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: exactly one DBInstance must be reported, got {body}"
    );
    reported.into_iter().next().expect("checked just above")
}

/// `DescribeDBInstances` for one instance, as the one `<DBInstance>` it reports.
async fn describe_instance(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
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

/// `DeleteDBInstance` for one instance, as the one `<DBInstance>` it answers with.
async fn delete_instance(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let body = expect_ok(
        case,
        call(
            router,
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", identifier),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await,
    );
    the_instance(case, &body)
}

/// Assert one member's value, naming the case, the envelope and what was reported.
fn assert_member(case: &str, envelope: &str, instance: &str, member: &str, expected: Option<&str>) {
    let actual = optional_text(instance, member);
    assert_eq!(
        actual.as_deref(),
        expected,
        "case {case:?}: the {envelope} envelope must report {member}={expected:?}, got \
         {actual:?} in {instance}"
    );
}

/// Assert the whole cluster-managed set on one envelope.
fn assert_cluster_managed(case: &str, envelope: &str, row: &MemberCase, instance: &str) {
    for (member, expected) in [
        ("AllocatedStorage", Some(CLUSTER_ALLOCATED_STORAGE)),
        ("StorageType", Some(row.storage_type)),
        // Throughput is provisioned with INSTANCE storage, and a cluster's is not
        // configurable, so a member has none.
        ("StorageThroughput", Some("0")),
        // The port is the cluster's, so the member has no port of its own.
        ("DbInstancePort", Some("0")),
        ("MasterUsername", row.master_username),
        ("DBName", row.db_name),
        ("StorageEncryptionType", Some(row.storage_encryption_type)),
        ("PromotionTier", Some(row.promotion_tier)),
        ("VPCNetworkingEnabled", Some("true")),
    ] {
        assert_member(case, envelope, instance, member, expected);
    }
    // One subelement per pending change, and nothing is pending for a member nothing has
    // modified.
    assert_eq!(
        texts(instance, "PendingModifiedValues").as_slice(),
        [String::new()],
        "case {case:?}: the {envelope} envelope must report an empty PendingModifiedValues \
         for a member with nothing pending, got {instance}"
    );
}

#[tokio::test]
async fn a_cluster_member_reports_the_storage_and_master_user_its_cluster_manages() {
    let router = emulator();

    for row in MEMBER_CASES {
        create_cluster(&router, row).await;
    }
    for row in MEMBER_CASES {
        let created = create_member(&router, row).await;
        assert_cluster_managed(row.case, "create", row, &created);
    }
    wait_for_settle();
    for row in MEMBER_CASES {
        let described = describe_instance(&router, row.case, row.instance).await;
        assert_cluster_managed(row.case, "describe", row, &described);
        // The third envelope answers with the instance the delete was accepted for, which
        // reports the same record.
        let deleted = delete_instance(&router, row.case, row.instance).await;
        assert_cluster_managed(row.case, "delete", row, &deleted);
    }
}

#[tokio::test]
async fn an_instance_that_belongs_to_no_cluster_keeps_the_storage_its_request_named() {
    let router = emulator();
    let case = "a standalone postgres instance";
    let identifier = "emu-cm-standalone";
    let supplied: &[(&str, &str)] = &[
        ("AllocatedStorage", "40"),
        ("StorageType", "gp3"),
        ("StorageThroughput", "500"),
        ("Iops", "3000"),
        ("Port", "5433"),
        ("MasterUsername", "ownadmin"),
        ("MasterUserPassword", "a-strong-password-3"),
        ("DBName", "owndb"),
        ("StorageEncrypted", "true"),
    ];
    let mut params: Vec<(&str, String)> = vec![
        ("DBInstanceIdentifier", identifier.to_string()),
        ("Engine", "postgres".to_string()),
        ("DBInstanceClass", CLASS.to_string()),
    ];
    params.extend(
        supplied
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    expect_ok(case, call(&router, "CreateDBInstance", &params).await);
    wait_for_settle();
    let described = describe_instance(&router, case, identifier).await;

    for (member, expected) in [
        ("AllocatedStorage", Some("40")),
        ("StorageType", Some("gp3")),
        ("StorageThroughput", Some("500")),
        ("MasterUsername", Some("ownadmin")),
        ("DBName", Some("owndb")),
        // Encrypted under a KMS key, which is what the member's `sse-kms` value names.
        ("StorageEncryptionType", Some("sse-kms")),
        // An instance that belongs to no DB cluster has no cluster-member port, whatever
        // port it was asked to listen on, and no failover order inside a cluster.
        ("DbInstancePort", Some("0")),
        ("PromotionTier", None),
        // The flag is reported for every instance: this one runs in a VPC of its own.
        ("VPCNetworkingEnabled", Some("true")),
    ] {
        assert_member(case, "describe", &described, member, expected);
    }
}

#[tokio::test]
async fn an_unencrypted_instance_of_its_own_reports_no_storage_encryption_type() {
    let router = emulator();
    let case = "an unencrypted mysql instance";
    let identifier = "emu-cm-plain";
    expect_ok(
        case,
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", identifier),
                ("Engine", "mysql"),
                ("DBInstanceClass", CLASS),
                ("AllocatedStorage", "20"),
                ("MasterUsername", "emuadmin"),
                ("MasterUserPassword", "a-strong-password-4"),
            ],
        )
        .await,
    );
    wait_for_settle();
    let described = describe_instance(&router, case, identifier).await;
    // An instance whose data at rest is not encrypted has no encryption type to report.
    assert_member(case, "describe", &described, "StorageEncryptionType", None);
    assert_member(case, "describe", &described, "StorageEncrypted", Some("false"));
}
