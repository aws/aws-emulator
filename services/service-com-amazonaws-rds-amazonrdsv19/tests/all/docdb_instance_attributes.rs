// SPDX-License-Identifier: CC0-1.0

//! The attribute set this front door reports for an instance of a SIBLING control plane's
//! engine whose instance-level members that control plane does not model.
//!
//! The rule under test: an instance ordered through a control plane that models no
//! instance-level storage, port, public-access, licence or master-user member
//! (`com.amazonaws.docdb#CreateDBInstanceMessage` carries none of them, and
//! `com.amazonaws.docdb#DBInstance$PubliclyAccessible` states the value "is always
//! `false`") reports, on EVERY path that projects it, the storage its CLUSTER manages (the
//! size that cluster reports, on the cluster's storage type, with no provisioned
//! throughput), that cluster's master user, its VPC-networking and HTTP-endpoint state, the
//! one licence model its engine publishes, no instance-level port, no public endpoint, and
//! the pending-changes element.
//!
//! The rows span what the rule generalises over: whether the cluster carries a master user,
//! which of the engine's two storage types manages its storage, the promotion tier the
//! member was created at (both published bounds of `0-15`), and the member's place in its
//! cluster. The control row is an instance of one of THIS model's own engines, whose
//! request carries every one of those members and which must keep reporting them.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so each
//! assertion reads the XML this emulator itself answered with.

use crate::db_instances::{call, emulator, expect_ok, optional_text, texts};

/// The members such an instance reports with the same value whatever the instance is, and
/// the value each one carries for the document engine.
///
/// Sources: `com.amazonaws.docdb#DBInstance$PubliclyAccessible` ("The value of
/// `PubliclyAccessible` is always `false`"); the storage size a cluster whose size is not
/// fixed reports (`state::ELASTIC_CLUSTER_ALLOCATED_STORAGE`); the throughput a cluster
/// provisions per instance, which is none; the licence model that engine's catalogue
/// publishes (`service_common::docdb_engine_catalog::LICENSE_MODEL`); and the port an
/// instance whose port is its cluster's carries, which is none.
const CLUSTER_MANAGED_ATTRIBUTES: [(&str, &str); 7] = [
    ("AllocatedStorage", "1"),
    ("StorageThroughput", "0"),
    ("DbInstancePort", "0"),
    ("PubliclyAccessible", "false"),
    ("VPCNetworkingEnabled", "true"),
    ("LicenseModel", "na"),
    // Reported on every such instance, one subelement per pending change: an instance with
    // nothing pending carries the element with nothing inside it.
    ("PendingModifiedValues", ""),
];

/// The engine of the sibling control plane under test, as its own `Engine` member publishes
/// it ("Valid values: docdb").
const DOCUMENT_ENGINE: &str = "docdb";

/// One row of the table: how the cluster and the member were created, and the values that
/// follow from them.
struct Row {
    label: &'static str,
    cluster: &'static str,
    instance: &'static str,
    /// The `MasterUsername` the cluster was created with, when it was created with one.
    master_username: Option<&'static str>,
    /// The `StorageType` the cluster named, when it named one.
    storage_type: Option<&'static str>,
    /// The `PromotionTier` the instance asked for, when it asked for one.
    promotion_tier: Option<&'static str>,
    /// The storage type the member must report: the one its cluster manages its storage on,
    /// which is `standard` for a cluster that named none.
    expected_storage_type: &'static str,
    /// The tier the member must report; `PromotionTier` documents "Default: 1".
    expected_promotion_tier: &'static str,
}

fn rows() -> [Row; 4] {
    [
        Row {
            label: "a cluster with a master user, on the default storage type",
            cluster: "docdb-attrs-default",
            instance: "docdb-attrs-default-1",
            master_username: Some("probeadmin"),
            storage_type: None,
            promotion_tier: Some("1"),
            expected_storage_type: "standard",
            expected_promotion_tier: "1",
        },
        Row {
            label: "a member that asked for no promotion tier",
            cluster: "docdb-attrs-notier",
            instance: "docdb-attrs-notier-1",
            master_username: Some("dbadmin"),
            storage_type: Some("standard"),
            promotion_tier: None,
            expected_storage_type: "standard",
            expected_promotion_tier: "1",
        },
        Row {
            label: "a cluster on the io-optimized storage type, at the top of the range",
            cluster: "docdb-attrs-iopt1",
            instance: "docdb-attrs-iopt1-1",
            master_username: Some("ioadmin"),
            storage_type: Some("iopt1"),
            promotion_tier: Some("15"),
            expected_storage_type: "iopt1",
            expected_promotion_tier: "15",
        },
        Row {
            label: "a cluster created without a master user, at the bottom of the range",
            cluster: "docdb-attrs-nouser",
            instance: "docdb-attrs-nouser-1",
            master_username: None,
            storage_type: None,
            promotion_tier: Some("0"),
            expected_storage_type: "standard",
            expected_promotion_tier: "0",
        },
    ]
}

/// Assert the whole attribute set on one answer: the members this engine fixes
/// ([`CLUSTER_MANAGED_ATTRIBUTES`]) and the ones that follow from the row.
fn assert_attributes(case: &str, row: &Row, xml: &str) {
    for (member, expected) in CLUSTER_MANAGED_ATTRIBUTES {
        let actual = optional_text(xml, member);
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "case {case:?}: the instance envelope must carry {member}={expected:?}, got \
             {actual:?} in {xml}"
        );
    }
    for (member, expected) in [
        ("StorageType", Some(row.expected_storage_type)),
        ("PromotionTier", Some(row.expected_promotion_tier)),
        ("MasterUsername", row.master_username),
    ] {
        let actual = optional_text(xml, member);
        assert_eq!(
            actual.as_deref(),
            expected,
            "case {case:?}: the instance envelope must report {member}={expected:?}, got \
             {actual:?} in {xml}"
        );
    }
}

#[tokio::test]
async fn an_instance_of_a_sibling_control_plane_reports_its_clusters_managed_attributes() {
    let router = emulator();

    for row in rows() {
        let mut cluster_params: Vec<(&str, &str)> = vec![
            ("DBClusterIdentifier", row.cluster),
            ("Engine", DOCUMENT_ENGINE),
        ];
        if let Some(user) = row.master_username {
            cluster_params.push(("MasterUsername", user));
            cluster_params.push(("MasterUserPassword", "secret99"));
        }
        if let Some(storage) = row.storage_type {
            cluster_params.push(("StorageType", storage));
        }
        expect_ok(
            row.label,
            call(&router, "CreateDBCluster", &cluster_params).await,
        );

        let mut instance_params: Vec<(&str, &str)> = vec![
            ("DBInstanceIdentifier", row.instance),
            ("DBInstanceClass", "db.r5.large"),
            ("Engine", DOCUMENT_ENGINE),
            ("DBClusterIdentifier", row.cluster),
        ];
        if let Some(tier) = row.promotion_tier {
            instance_params.push(("PromotionTier", tier));
        }

        // 1. The create answer.
        let created = expect_ok(
            row.label,
            call(&router, "CreateDBInstance", &instance_params).await,
        );
        assert_attributes(&format!("{} / create", row.label), &row, &created);

        // 2. The read back, which projects the stored record again: the attribute set is a
        //    property of the record and its cluster, so it does not wait on the status the
        //    clock owes (the lifecycle suites pin that transition).
        let described = expect_ok(
            row.label,
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", row.instance)],
            )
            .await,
        );
        assert_attributes(&format!("{} / describe", row.label), &row, &described);

        // 3. The delete answer, the last path that reports the instance.
        let deleted = expect_ok(
            row.label,
            call(
                &router,
                "DeleteDBInstance",
                &[
                    ("DBInstanceIdentifier", row.instance),
                    ("SkipFinalSnapshot", "true"),
                ],
            )
            .await,
        );
        assert_attributes(&format!("{} / delete", row.label), &row, &deleted);
    }
}

#[tokio::test]
async fn every_member_of_one_cluster_reports_the_same_managed_attributes() {
    let router = emulator();
    expect_ok(
        "host cluster",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "docdb-attrs-page"),
                ("Engine", DOCUMENT_ENGINE),
                ("MasterUsername", "pageadmin"),
                ("MasterUserPassword", "secret99"),
            ],
        )
        .await,
    );
    // The cluster's writer and two replicas: the attribute set does not depend on a
    // member's place in its cluster.
    let members = ["docdb-attrs-page-1", "docdb-attrs-page-2", "docdb-attrs-page-3"];
    for member in members {
        expect_ok(
            member,
            call(
                &router,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", member),
                    ("DBInstanceClass", "db.r5.large"),
                    ("Engine", DOCUMENT_ENGINE),
                    ("DBClusterIdentifier", "docdb-attrs-page"),
                ],
            )
            .await,
        );
    }
    let page = expect_ok(
        "whole page",
        call(&router, "DescribeDBInstances", &[("MaxRecords", "20")]).await,
    );
    for (member, expected) in CLUSTER_MANAGED_ATTRIBUTES {
        let values = texts(&page, member);
        assert_eq!(
            values.len(),
            members.len(),
            "case \"whole page\": every listed instance must carry one {member}, got \
             {values:?} in {page}"
        );
        for (index, value) in values.into_iter().enumerate() {
            assert_eq!(
                value, expected,
                "case \"whole page\" (instance {index}): expected {member}={expected:?}, got \
                 {value:?} in {page}"
            );
        }
    }
    assert_eq!(
        texts(&page, "MasterUsername").len(),
        members.len(),
        "case \"whole page\": every listed instance must report its cluster's master user, \
         got {page}"
    );
}

#[tokio::test]
async fn an_instance_of_this_doors_own_engine_keeps_reporting_what_its_request_carried() {
    // The control row: every member above IS an input of this model's own
    // `CreateDBInstance`, so an instance of one of its engines reports the values the
    // request carried and the documented defaults — never the cluster-managed ones.
    let router = emulator();
    let created = expect_ok(
        "rds instance",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "own-engine-instance"),
                ("DBInstanceClass", "db.m5.large"),
                ("Engine", "postgres"),
                ("AllocatedStorage", "40"),
                ("MasterUsername", "ownadmin"),
                ("MasterUserPassword", "secret99"),
                ("StorageType", "gp3"),
                ("Port", "5433"),
                ("PubliclyAccessible", "true"),
            ],
        )
        .await,
    );
    for (member, expected) in [
        ("AllocatedStorage", "40"),
        ("StorageType", "gp3"),
        ("MasterUsername", "ownadmin"),
        ("PubliclyAccessible", "true"),
        ("LicenseModel", "postgresql-license"),
        // The flag is reported for EVERY instance, read off the network the instance is
        // in: this one is in a VPC of its own.
        ("VPCNetworkingEnabled", "true"),
    ] {
        let actual = optional_text(&created, member);
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "case \"rds instance\": expected {member}={expected:?}, got {actual:?} in \
             {created}"
        );
    }
}
