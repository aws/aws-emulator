// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `DBCluster` ENVELOPE every operation of this front door that
//! reports a cluster answers with, and for what an accepted `DeleteDBCluster` answers.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper
//! this test target has, as a real awsQuery form POST carrying the SigV4 credential scope
//! the router keys on, so each assertion reads the XML the emulator itself produced.
//!
//! The rules under test:
//!
//! * ONE serializer, ONE envelope: whichever operation reports a cluster — the create that
//!   minted it, the read, the modification and the delete — reports the same members for it.
//!   The members this suite pins are the ones the API reports for EVERY cluster it holds,
//!   independent of engine: the two express-configuration settings, the two capability
//!   flags, the database-activity-stream status under both names the envelope carries it
//!   under, the Route 53 hosted zone its endpoints sit in, the DB subnet group it is placed
//!   in and the VPC security groups it belongs to.
//! * A member a cluster's CONFIGURATION decides is reported for a cluster that has it and
//!   left out for one that does not: the Aurora Serverless v2 platform version is reported
//!   for a cluster with Serverless v2 capacity, and the KMS key identifier for a cluster
//!   whose storage is encrypted — the key the request named, or the account's default key
//!   for this service where it named none.
//! * A create that names no `EngineVersion` provisions the ENGINE's default version, and the
//!   default DB cluster parameter group it reports is the one of THAT version's family. Both
//!   are read out of `DescribeDBEngineVersions`, so the catalogue this service publishes and
//!   the cluster it creates cannot disagree.
//! * An accepted delete answers the cluster AS IT FOUND IT and moves it on: a delete that
//!   skips the final snapshot answers the status the cluster carried, a delete that asks for
//!   a final DB cluster snapshot takes that snapshot and answers the cluster `backing-up`,
//!   and either way the cluster reaches `deleting` and then leaves the service. A
//!   modification answers the same way.
//!
//! Each rule is asserted over a TABLE spanning what it generalises over: the cluster TYPES
//! `CreateDBCluster` documents (Aurora DB clusters and Multi-AZ DB clusters), four engines
//! across the three control planes this front door serves, a request that names nothing
//! beyond the two required members and requests that name a subnet group, security groups,
//! Serverless v2 capacity and storage encryption, and both regions whose published hosted
//! zone id this service carries.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_rds_amazonrdsv19::state;

use crate::db_clusters::{describe_one, the_cluster};
use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, optional_text, subtrees, text,
    wait_for_settle, REGION,
};

/// One cluster of the envelope table.
struct EnvelopeCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    /// The parameters this row adds beyond `DBClusterIdentifier` and `Engine`.
    extra: &'static [(&'static str, &'static str)],
    /// The `DBSubnetGroup` the cluster must report: the group the request named, or the
    /// account's `default` group where it named none.
    subnet_group: &'static str,
    /// The `VpcSecurityGroupId`s the cluster must report, in order; empty means its VPC's
    /// default group, which the assertion resolves for itself.
    security_groups: &'static [&'static str],
    /// Whether the row configures Aurora Serverless v2 capacity, and so must report the
    /// platform version that capacity runs on.
    serverless_v2: bool,
    /// Whether the row turns storage encryption on, and so must report the key it is
    /// encrypted under.
    encrypted: bool,
}

/// The subnet group every row that names one names, created by the test.
const NAMED_SUBNET_GROUP: &str = "emu-envelope-subnets";

/// The clusters the envelope rules are asserted over.
const ENVELOPE_CASES: &[EnvelopeCase] = &[
    EnvelopeCase {
        case: "aurora-mysql, nothing beyond the two required members",
        identifier: "emu-envelope-mysql",
        engine: "aurora-mysql",
        extra: &[],
        subnet_group: "default",
        security_groups: &[],
        serverless_v2: false,
        encrypted: false,
    },
    EnvelopeCase {
        case: "aurora-postgresql with Serverless v2 capacity and encrypted storage",
        identifier: "emu-envelope-serverless",
        engine: "aurora-postgresql",
        extra: &[
            ("ServerlessV2ScalingConfiguration.MinCapacity", "0.5"),
            ("ServerlessV2ScalingConfiguration.MaxCapacity", "1.0"),
            ("StorageEncrypted", "true"),
        ],
        subnet_group: "default",
        security_groups: &[],
        serverless_v2: true,
        encrypted: true,
    },
    EnvelopeCase {
        case: "aurora-postgresql naming its own subnet group and security groups",
        identifier: "emu-envelope-placed",
        engine: "aurora-postgresql",
        extra: &[
            ("DBSubnetGroupName", NAMED_SUBNET_GROUP),
            ("VpcSecurityGroupIds.VpcSecurityGroupId.1", "sg-0a1b2c3d4e5f60711"),
            ("VpcSecurityGroupIds.VpcSecurityGroupId.2", "sg-0a1b2c3d4e5f60722"),
        ],
        subnet_group: NAMED_SUBNET_GROUP,
        security_groups: &["sg-0a1b2c3d4e5f60711", "sg-0a1b2c3d4e5f60722"],
        serverless_v2: false,
        encrypted: false,
    },
    EnvelopeCase {
        case: "docdb, a cluster of a sibling control plane's engine",
        identifier: "emu-envelope-docdb",
        engine: "docdb",
        extra: &[
            ("MasterUsername", "emuadmin"),
            ("MasterUserPassword", "pw1-Secret"),
        ],
        subnet_group: "default",
        security_groups: &[],
        serverless_v2: false,
        encrypted: false,
    },
    EnvelopeCase {
        case: "postgres, a Multi-AZ DB cluster",
        identifier: "emu-envelope-multiaz",
        engine: "postgres",
        extra: &[
            ("AllocatedStorage", "100"),
            ("DBClusterInstanceClass", "db.m6gd.xlarge"),
            ("StorageType", "io1"),
            ("Iops", "1000"),
            ("KmsKeyId", "emu-envelope-key"),
            ("StorageEncrypted", "true"),
        ],
        subnet_group: "default",
        security_groups: &[],
        serverless_v2: false,
        encrypted: true,
    },
];

/// `CreateDBSubnetGroup` over the two subnets a group must name.
async fn create_subnet_group(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "CreateDBSubnetGroup",
        &[
            ("DBSubnetGroupName", name),
            ("DBSubnetGroupDescription", "the envelope suite's group"),
            ("SubnetIds.SubnetIdentifier.1", "subnet-0a1b2c3d4e5f60711"),
            ("SubnetIds.SubnetIdentifier.2", "subnet-0a1b2c3d4e5f60722"),
        ],
    )
    .await
}

/// `CreateDBCluster` for one row of a table.
async fn create_cluster(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut request: Vec<(&str, String)> = vec![
        ("DBClusterIdentifier", identifier.to_string()),
        ("Engine", engine.to_string()),
    ];
    request.extend(
        extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    call(router, "CreateDBCluster", &request).await
}

/// Assert the members EVERY cluster envelope of this front door carries, whichever
/// operation answered with it.
fn assert_always_reported(case: &str, label: &str, cluster: &str, row: &EnvelopeCase) {
    let case = format!("{case} ({label})");

    // The flags the recorded envelope carries on every cluster: a cluster created here is
    // VPC-based with no internet access gateway in front of it, and runs no database
    // activity stream.
    for (member, expected) in [
        ("VPCNetworkingEnabled", "true"),
        ("InternetAccessGatewayEnabled", "false"),
        ("ActivityStreamStatus", "stopped"),
    ] {
        let reported = optional_text(cluster, member);
        assert_eq!(
            reported.as_deref(),
            Some(expected),
            "case {case:?}: expected <{member}> {expected:?}, got {reported:?} in {cluster}"
        );
    }

    // One hosted zone per region and control-plane DNS domain, reported for every cluster
    // reached under that domain.
    let expected_zone = state::hosted_zone_id(REGION, state::endpoint_domain(row.engine));
    let hosted_zone = optional_text(cluster, "HostedZoneId");
    assert_eq!(
        hosted_zone.as_deref(),
        Some(expected_zone.as_str()),
        "case {case:?}: expected <HostedZoneId> {expected_zone:?}, got {hosted_zone:?}"
    );

    // Where the cluster is PLACED: the DB subnet group the request named, or the group the
    // account's default VPC carries.
    let subnet_group = optional_text(cluster, "DBSubnetGroup");
    assert_eq!(
        subnet_group.as_deref(),
        Some(row.subnet_group),
        "case {case:?}: expected <DBSubnetGroup> {:?}, got {subnet_group:?}",
        row.subnet_group
    );

    // The security groups it belongs to, each in the status an association in place has.
    let expected_groups: Vec<String> = if row.security_groups.is_empty() {
        vec![state::default_security_group_id(REGION)]
    } else {
        row.security_groups
            .iter()
            .map(|id| (*id).to_string())
            .collect()
    };
    let memberships = subtrees(cluster, "VpcSecurityGroupMembership");
    assert_eq!(
        memberships.len(),
        expected_groups.len(),
        "case {case:?}: expected {} security-group membership(s), got {memberships:?}",
        expected_groups.len()
    );
    for (membership, expected) in memberships.iter().zip(&expected_groups) {
        let reported = text(membership, "VpcSecurityGroupId");
        assert_eq!(
            reported, *expected,
            "case {case:?}: expected <VpcSecurityGroupId> {expected:?}, got {reported:?}"
        );
        let status = text(membership, "Status");
        assert_eq!(
            status, "active",
            "case {case:?}: expected the membership status \"active\", got {status:?}"
        );
    }

    // The Aurora Serverless v2 platform version: reported for a cluster that HAS Serverless
    // v2 capacity, and for one only. Its form is the one
    // `DescribeServerlessV2PlatformVersions$ServerlessV2PlatformVersion` publishes — a whole
    // number ("Example: 3") — rather than a particular release this suite restates.
    let platform = optional_text(cluster, "ServerlessV2PlatformVersion");
    if row.serverless_v2 {
        let reported = platform.clone().unwrap_or_default();
        assert!(
            reported.parse::<u32>().is_ok_and(|version| version > 0),
            "case {case:?}: a cluster with Serverless v2 capacity reports the platform \
             version it runs on, got {platform:?}"
        );
    } else {
        assert_eq!(
            platform, None,
            "case {case:?}: a cluster with no Serverless v2 capacity runs on no Serverless \
             v2 platform, got {platform:?}"
        );
    }

    // The key the cluster's storage is encrypted under: reported for an encrypted cluster,
    // and for one only. A request that named a key reports that key; one that named none
    // reports the key this service encrypts under by default, whose identifier is not a
    // value the request carried.
    let encrypted = text(cluster, "StorageEncrypted");
    assert_eq!(
        encrypted,
        row.encrypted.to_string(),
        "case {case:?}: expected <StorageEncrypted> {}, got {encrypted:?}",
        row.encrypted
    );
    let key = optional_text(cluster, "KmsKeyId");
    match (row.encrypted, row.named_kms_key()) {
        (true, Some(named)) => assert_eq!(
            key.as_deref(),
            Some(named),
            "case {case:?}: expected the key the request named, got {key:?}"
        ),
        (true, None) => {
            // The key this service encrypts under by default: an identifier of a KMS key,
            // which is what the member carries — an ARN of a key in this Region, or the
            // published alias of the account's default key where KMS could not be asked.
            // The key's own id is per-run, so only its FORM is asserted.
            let reported = key.clone().unwrap_or_default();
            let is_key_arn = reported.starts_with("arn:aws:kms:") && reported.contains(":key/");
            let is_alias = reported.starts_with("alias/");
            assert!(
                is_key_arn || is_alias,
                "case {case:?}: an encrypted cluster reports the KMS key it is encrypted \
                 under, got {key:?} in {cluster}"
            );
        }
        (false, _) => assert_eq!(
            key, None,
            "case {case:?}: a cluster whose storage is not encrypted is encrypted under no \
             key, got {key:?}"
        ),
    }
}

impl EnvelopeCase {
    /// The `KmsKeyId` this row's request names, if it names one.
    fn named_kms_key(&self) -> Option<&'static str> {
        self.extra
            .iter()
            .find(|(name, _)| *name == "KmsKeyId")
            .map(|(_, value)| *value)
    }
}

#[tokio::test]
async fn every_operation_that_reports_a_cluster_reports_the_same_envelope_members() {
    let router = emulator();
    expect_ok(
        NAMED_SUBNET_GROUP,
        create_subnet_group(&router, NAMED_SUBNET_GROUP).await,
    );

    for row in ENVELOPE_CASES {
        let created = the_cluster(
            row.case,
            &expect_ok(
                row.case,
                create_cluster(&router, row.identifier, row.engine, row.extra).await,
            ),
        );
        assert_always_reported(row.case, "CreateDBCluster", &created, row);

        wait_for_settle();
        let read = describe_one(&router, row.case, row.identifier).await;
        assert_always_reported(row.case, "DescribeDBClusters", &read, row);

        // A modification reports the same envelope, and its acknowledgement carries the
        // status the cluster was in when the call arrived: the transition the modification
        // starts is what the next read reports.
        let modified = the_cluster(
            row.case,
            &expect_ok(
                row.case,
                call(
                    &router,
                    "ModifyDBCluster",
                    &[
                        ("DBClusterIdentifier", row.identifier),
                        ("PreferredBackupWindow", "03:00-03:30"),
                    ],
                )
                .await,
            ),
        );
        assert_always_reported(row.case, "ModifyDBCluster", &modified, row);
        let status = text(&modified, "Status");
        assert_eq!(
            status, "available",
            "case {:?}: a modification answers the status the cluster carried, got \
             {status:?}",
            row.case
        );

        // And so does the delete, which is the last envelope a client ever sees for the
        // cluster.
        let deleted = the_cluster(
            row.case,
            &expect_ok(
                row.case,
                call(
                    &router,
                    "DeleteDBCluster",
                    &[
                        ("DBClusterIdentifier", row.identifier),
                        ("SkipFinalSnapshot", "true"),
                    ],
                )
                .await,
            ),
        );
        assert_always_reported(row.case, "DeleteDBCluster", &deleted, row);
    }
}

#[tokio::test]
async fn the_hosted_zone_a_cluster_reports_is_the_regions_and_holds_still() {
    let router = emulator();

    // One zone per region and domain: two clusters of one region report the same id, a
    // cluster of another region reports that region's, and a read reports what the create
    // did. The two regions are the ones whose published RDS hosted zone id this service
    // carries, so neither row's id is one this emulator minted.
    let mut per_region: Vec<(&str, String)> = Vec::new();
    for (region, identifiers) in [
        ("us-west-2", ["emu-zone-west-one", "emu-zone-west-two"]),
        ("us-east-1", ["emu-zone-east-one", "emu-zone-east-two"]),
    ] {
        for identifier in identifiers {
            let created = the_cluster(
                identifier,
                &expect_ok(
                    identifier,
                    call_in(
                        &router,
                        region,
                        "CreateDBCluster",
                        &[
                            ("DBClusterIdentifier", identifier),
                            ("Engine", "aurora-mysql"),
                        ],
                    )
                    .await,
                ),
            );
            let zone = text(&created, "HostedZoneId");
            assert!(
                zone.starts_with('Z') && zone.len() > 1,
                "case {identifier:?}: a hosted zone id is written in the published Z-prefixed \
                 form, got {zone:?}"
            );
            // Read back: the id is a fact about the region, not a value recomputed per
            // response.
            let read = the_cluster(
                identifier,
                &expect_ok(
                    identifier,
                    call_in(
                        &router,
                        region,
                        "DescribeDBClusters",
                        &[("DBClusterIdentifier", identifier)],
                    )
                    .await,
                ),
            );
            assert_eq!(
                text(&read, "HostedZoneId"),
                zone,
                "case {identifier:?}: the hosted zone id a read reports is the one the create \
                 did"
            );
            per_region.push((region, zone));
        }
    }
    for (region, zone) in &per_region {
        for (other_region, other_zone) in &per_region {
            if region == other_region {
                assert_eq!(
                    zone, other_zone,
                    "every cluster of {region} sits in one hosted zone, got {zone:?} and \
                     {other_zone:?}"
                );
            } else {
                assert_ne!(
                    zone, other_zone,
                    "{region} and {other_region} are different hosted zones, both reported \
                     {zone:?}"
                );
            }
        }
    }
}

/// The engines whose default version a create with no `EngineVersion` provisions: the ones
/// this front door resolves a default for, as `DescribeDBEngineVersions$DefaultOnly` does
/// (it resolves none for an engine this door reports on behalf of another control plane).
/// One Aurora engine per family and both Multi-AZ DB cluster engines, so the rule is not
/// asserted on one engine's catalogue alone.
const DEFAULT_VERSION_ENGINES: &[&str] = &["aurora-mysql", "aurora-postgresql", "postgres", "mysql"];

/// The `EngineVersion` and `DBParameterGroupFamily` `DescribeDBEngineVersions` publishes as
/// `engine`'s default.
async fn published_default_version(
    router: &MultiServiceRouter,
    engine: &str,
) -> (String, String) {
    let body = expect_ok(
        engine,
        call(
            router,
            "DescribeDBEngineVersions",
            &[("Engine", engine), ("DefaultOnly", "true")],
        )
        .await,
    );
    let published = subtrees(&body, "DBEngineVersion");
    assert_eq!(
        published.len(),
        1,
        "case {engine:?}: one version is an engine's default, got {published:?}"
    );
    let entry = &published[0];
    (
        text(entry, "EngineVersion"),
        text(entry, "DBParameterGroupFamily"),
    )
}

#[tokio::test]
async fn a_create_that_names_no_version_provisions_the_published_engine_default() {
    let router = emulator();

    for engine in DEFAULT_VERSION_ENGINES {
        let (version, family) = published_default_version(&router, engine).await;
        let identifier = format!("emu-default-{}", engine.replace('-', ""));
        let mut extra: Vec<(&str, &str)> = Vec::new();
        if !engine.starts_with("aurora-") {
            // The settings a Multi-AZ DB cluster is required to carry.
            extra.extend_from_slice(&[
                ("AllocatedStorage", "100"),
                ("DBClusterInstanceClass", "db.m6gd.xlarge"),
                ("StorageType", "io1"),
                ("Iops", "1000"),
            ]);
        }
        let created = the_cluster(
            engine,
            &expect_ok(
                engine,
                create_cluster(&router, &identifier, engine, &extra).await,
            ),
        );
        let reported = text(&created, "EngineVersion");
        assert_eq!(
            reported, version,
            "case {engine:?}: a create that names no version provisions the engine's \
             published default {version:?}, got {reported:?}"
        );
        // "If you don't specify a value, then the default DB cluster parameter group for the
        // specified DB engine and version is used": the group of the family THAT version
        // belongs to, so the two cannot name different families.
        let group = text(&created, "DBClusterParameterGroup");
        assert_eq!(
            group,
            format!("default.{family}"),
            "case {engine:?}: expected the default group of family {family:?}, got {group:?}"
        );
    }
}

/// The engines the backup-status rule is asserted over: one per Aurora family, so the rule
/// is not pinned to one engine's defaults.
const BACKUP_STATUS_ENGINES: &[&str] = &["aurora-postgresql", "aurora-mysql"];

#[tokio::test]
async fn a_cluster_reports_backing_up_while_a_backup_of_it_is_in_flight() {
    for engine in BACKUP_STATUS_ENGINES {
        // Each engine gets its own emulator, so one row's transitions cannot carry into the
        // next.
        let router = emulator();
        let identifier = format!("emu-backup-{}", engine.replace('-', ""));
        expect_ok(
            engine,
            create_cluster(&router, &identifier, engine, &[]).await,
        );
        wait_for_settle();
        let settled = text(&describe_one(&router, engine, &identifier).await, "Status");
        assert_eq!(
            settled, "available",
            "case {engine:?}: expected the cluster to settle before the snapshot, got \
             {settled:?}"
        );

        // A snapshot of the cluster is a backup of it: while it is being taken the cluster
        // reports that it is being backed up.
        let snapshot = format!("{identifier}-snap");
        let taken = expect_ok(
            engine,
            call(
                &router,
                "CreateDBClusterSnapshot",
                &[
                    ("DBClusterIdentifier", identifier.as_str()),
                    ("DBClusterSnapshotIdentifier", snapshot.as_str()),
                ],
            )
            .await,
        );
        let snapshot_status = text(&taken, "Status");
        assert_eq!(
            snapshot_status, "creating",
            "case {engine:?}: expected the snapshot to still be being taken, got \
             {snapshot_status:?}"
        );
        let during = text(&describe_one(&router, engine, &identifier).await, "Status");
        assert_eq!(
            during, "backing-up",
            "case {engine:?}: a cluster whose snapshot is still being taken reports that it \
             is being backed up, got {during:?}"
        );
        // And a delete arriving then answers the cluster as it found it: being backed up.
        let deleted = the_cluster(
            engine,
            &expect_ok(
                engine,
                call(
                    &router,
                    "DeleteDBCluster",
                    &[
                        ("DBClusterIdentifier", identifier.as_str()),
                        ("SkipFinalSnapshot", "true"),
                    ],
                )
                .await,
            ),
        );
        let answered = text(&deleted, "Status");
        assert_eq!(
            answered, "backing-up",
            "case {engine:?}: the delete answers the status the cluster carried, got \
             {answered:?}"
        );
    }
}

#[tokio::test]
async fn a_cluster_is_available_again_once_its_backup_has_been_taken() {
    let router = emulator();
    expect_ok(
        "settled",
        create_cluster(&router, "emu-backup-settles", "aurora-postgresql", &[]).await,
    );
    wait_for_settle();
    expect_ok(
        "settled",
        call(
            &router,
            "CreateDBClusterSnapshot",
            &[
                ("DBClusterIdentifier", "emu-backup-settles"),
                ("DBClusterSnapshotIdentifier", "emu-backup-settles-snap"),
            ],
        )
        .await,
    );
    // The backup finishes on its own: the snapshot becomes available and the cluster is in
    // service again, with no further call of any kind.
    wait_for_settle();
    let reported = expect_ok(
        "settled",
        call(
            &router,
            "DescribeDBClusterSnapshots",
            &[("DBClusterSnapshotIdentifier", "emu-backup-settles-snap")],
        )
        .await,
    );
    let snapshot_status = text(&reported, "Status");
    assert_eq!(
        snapshot_status, "available",
        "expected the snapshot to have been taken, got {snapshot_status:?}"
    );
    let status = text(
        &describe_one(&router, "settled", "emu-backup-settles").await,
        "Status",
    );
    assert_eq!(
        status, "available",
        "a cluster whose backup has been taken is in service again, got {status:?}"
    );
}

/// One row of the delete table: how the delete names the final snapshot, and the status the
/// acknowledgement must carry.
struct DeleteCase {
    case: &'static str,
    identifier: &'static str,
    /// Whether the test waits the create out before deleting.
    settle_first: bool,
    /// The `FinalDBSnapshotIdentifier` the delete names, or `None` for a delete that skips
    /// the snapshot.
    final_snapshot: Option<&'static str>,
    /// The `Status` the accepted delete answers with.
    answered_status: &'static str,
}

/// The deletes the rule is asserted over: a skipped snapshot from both the status a cluster
/// is created in and the one it settles on, and a final snapshot of a settled cluster.
const DELETE_CASES: &[DeleteCase] = &[
    DeleteCase {
        case: "a skipped final snapshot, deleted once the cluster has settled",
        identifier: "emu-final-skip-available",
        settle_first: true,
        final_snapshot: None,
        answered_status: "available",
    },
    DeleteCase {
        case: "a skipped final snapshot, deleted while the cluster is still being created",
        identifier: "emu-final-skip-creating",
        settle_first: false,
        final_snapshot: None,
        answered_status: "creating",
    },
    DeleteCase {
        case: "a final DB cluster snapshot the delete asks for",
        identifier: "emu-final-snapshot",
        settle_first: true,
        final_snapshot: Some("emu-final-snapshot-copy"),
        answered_status: "backing-up",
    },
];

#[tokio::test]
async fn an_accepted_delete_answers_the_cluster_as_it_found_it_and_takes_the_snapshot_asked_for()
{
    for row in DELETE_CASES {
        // Each row gets its own emulator, so one row's transitions cannot carry into the
        // next.
        let router = emulator();
        expect_ok(
            row.case,
            create_cluster(&router, row.identifier, "aurora-postgresql", &[]).await,
        );
        if row.settle_first {
            wait_for_settle();
        }

        let mut request: Vec<(&str, String)> =
            vec![("DBClusterIdentifier", row.identifier.to_string())];
        match row.final_snapshot {
            Some(snapshot) => {
                request.push(("FinalDBSnapshotIdentifier", snapshot.to_string()));
            }
            None => request.push(("SkipFinalSnapshot", "true".to_string())),
        }
        let accepted = the_cluster(
            row.case,
            &expect_ok(row.case, call(&router, "DeleteDBCluster", &request).await),
        );
        let status = text(&accepted, "Status");
        assert_eq!(
            status, row.answered_status,
            "case {:?}: expected the accepted delete to answer {:?}, got {status:?}",
            row.case, row.answered_status
        );

        // The final snapshot the request asked for is a DB cluster snapshot of its own, read
        // back through the operation that reports snapshots — it OUTLIVES the cluster, which
        // is the point of asking for one.
        if let Some(snapshot) = row.final_snapshot {
            let reported = expect_ok(
                row.case,
                call(
                    &router,
                    "DescribeDBClusterSnapshots",
                    &[("DBClusterSnapshotIdentifier", snapshot)],
                )
                .await,
            );
            let taken = subtrees(&reported, "DBClusterSnapshot");
            assert_eq!(
                taken.len(),
                1,
                "case {:?}: expected the final snapshot to be reported, got {reported}",
                row.case
            );
            assert_eq!(
                text(&taken[0], "DBClusterIdentifier"),
                row.identifier,
                "case {:?}: the final snapshot is a snapshot OF the deleted cluster, got {}",
                row.case,
                taken[0]
            );
            assert_eq!(
                text(&taken[0], "SnapshotType"),
                "manual",
                "case {:?}: a snapshot a caller asked for is a manual one, got {}",
                row.case,
                taken[0]
            );
        }

        // The delete cannot be started again, whatever the cluster is doing while it runs:
        // it "can't be canceled or reverted after it begins".
        expect_error(
            row.case,
            call(&router, "DeleteDBCluster", &request).await,
            400,
            "InvalidDBClusterStateFault",
        );

        // The cluster reaches `deleting` and then leaves the service, which is what a waiter
        // polls for. A delete that took a final snapshot passes through `backing-up` first,
        // so this loop reads until the identifier names nothing rather than counting spans.
        let mut statuses: Vec<String> = Vec::new();
        let mut gone = false;
        for _ in 0..20 {
            let (code, body) = call(
                &router,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", row.identifier)],
            )
            .await;
            if code == 404 {
                gone = true;
                break;
            }
            assert_eq!(
                code, 200,
                "case {:?}: a read of a cluster being deleted is a 200 or the modelled 404, \
                 got {code} with {body}",
                row.case
            );
            statuses.push(text(&the_cluster(row.case, &body), "Status"));
            wait_for_settle();
        }
        assert!(
            gone,
            "case {:?}: the cluster must leave the service once the delete has run, saw \
             {statuses:?}",
            row.case
        );
        assert!(
            statuses.iter().any(|status| status == "deleting"),
            "case {:?}: the cluster reports deleting on its way out, saw {statuses:?}",
            row.case
        );
        if row.final_snapshot.is_some() {
            assert_eq!(
                statuses.first().map(String::as_str),
                Some("backing-up"),
                "case {:?}: the cluster is backing up while the final snapshot is taken, saw \
                 {statuses:?}",
                row.case
            );
            // And the snapshot is still there once the cluster is gone.
            let reported = expect_ok(
                row.case,
                call(
                    &router,
                    "DescribeDBClusterSnapshots",
                    &[(
                        "DBClusterSnapshotIdentifier",
                        row.final_snapshot.expect("checked just above"),
                    )],
                )
                .await,
            );
            assert_eq!(
                subtrees(&reported, "DBClusterSnapshot").len(),
                1,
                "case {:?}: the final snapshot outlives the cluster, got {reported}",
                row.case
            );
        }
    }
}
