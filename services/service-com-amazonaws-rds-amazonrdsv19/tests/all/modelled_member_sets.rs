// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MEMBER SET each resource envelope of this service carries.
//!
//! The rule under test: a response names exactly the members the service's Smithy model
//! declares for the shape it answers with — every declared member of the resource is
//! reported, and a name the model does not declare is reported by nothing. The emulator
//! serializes the modelled member set, so a capability or identity member the model does
//! not carry is not part of any answer, whichever operation produced it.
//!
//! The table spans what that rule generalises over: the six shapes whose declared member
//! set this service projects from its own state or from its reference catalogue
//! (`DBEngineVersion` and the `UpgradeTarget` nested in it, `OrderableDBInstanceOption`,
//! `DBCluster`, `DBInstance` and `DBClusterSnapshot`), across engines whose DB INSTANCE
//! owns its storage and engines whose CLUSTER manages it, and — for the engine-version and
//! orderable-option catalogues — over every entry the report answers rather than one row.
//!
//! Every call goes over the wire through the router the production emulator mounts
//! (`crate::db_instances::emulator`), as a real awsQuery form POST, so every assertion
//! reads the XML the emulator itself produced.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_cluster_snapshot_envelope_members::cluster_once_available;
use crate::db_clusters::the_cluster;
use crate::db_instances::{call, emulator, expect_ok, subtrees, text, texts};

/// One shape's member set, as the table reads it off an envelope.
struct MemberSet {
    /// Why this row is in the table.
    case: &'static str,
    /// Members the shape DECLARES, so the envelope must name each of them.
    declared: &'static [&'static str],
    /// Names the shape does NOT declare, so the envelope must name none of them.
    undeclared: &'static [&'static str],
}

/// `DBEngineVersion`: the identity and family of a version, and the capability flags a
/// client gates on. The undeclared names are the ones no member of the shape carries, so
/// no entry of the catalogue and no custom engine version may report them.
///
/// Source: `smithy_query(query="shape", shape="DBEngineVersion")`.
const ENGINE_VERSION: MemberSet = MemberSet {
    case: "an engine-version entry",
    declared: &[
        "Engine",
        "EngineVersion",
        "MajorEngineVersion",
        "DBParameterGroupFamily",
        "DBEngineDescription",
        "DBEngineVersionDescription",
        "Status",
        "SupportsLogExportsToCloudwatchLogs",
        "SupportsReadReplica",
        "SupportsGlobalDatabases",
        "SupportsIntegrations",
        "SupportsCertificateRotationWithoutRestart",
    ],
    undeclared: &[
        "MinorEngineVersion",
        "SupportsRDSProxy",
        "SupportsCluster",
        "SupportsDataGroups",
        "SupportsEnhancedBinlog",
        "SupportsChangeDataCaptureStreams",
    ],
};

/// `UpgradeTarget`: what a version can be upgraded to, how that upgrade is classified, and
/// the capabilities the caller would be running afterwards.
///
/// Source: `smithy_query(query="shape", shape="UpgradeTarget")`.
const UPGRADE_TARGET: MemberSet = MemberSet {
    case: "an upgrade target of an engine-version entry",
    declared: &[
        "Engine",
        "EngineVersion",
        "Description",
        "AutoUpgrade",
        "IsMajorVersionUpgrade",
        "SupportsGlobalDatabases",
        "SupportsParallelQuery",
    ],
    undeclared: &[
        "MajorEngineVersion",
        "MinorEngineVersion",
        "SupportsSnapshotUpgrade",
        "SupportsDataGroups",
    ],
};

/// `OrderableDBInstanceOption`: one orderable combination and the capabilities it offers.
///
/// Source: `smithy_query(query="shape", shape="OrderableDBInstanceOption")`.
const ORDERABLE_OPTION: MemberSet = MemberSet {
    case: "an orderable DB instance option",
    declared: &[
        "Engine",
        "EngineVersion",
        "DBInstanceClass",
        "StorageType",
        "SupportsClusters",
        "SupportsDedicatedLogVolume",
        "SupportsAdditionalStorageVolumes",
        "SupportsHttpEndpoint",
    ],
    undeclared: &[
        "SupportsOptimizedReads",
        "SupportsOptimizedWrites",
        "RequiresCustomProcessorFeatures",
    ],
};

/// `DBCluster`: the cluster envelope every operation that answers with a cluster carries.
///
/// Source: `smithy_query(query="shape", shape="DBCluster")`.
const DB_CLUSTER: MemberSet = MemberSet {
    case: "a DB cluster envelope",
    declared: &[
        "DBClusterIdentifier",
        "Engine",
        "EngineVersion",
        "Status",
        "ActivityStreamStatus",
        "HttpEndpointEnabled",
        "VPCNetworkingEnabled",
        "InternetAccessGatewayEnabled",
        "StorageEncryptionType",
    ],
    undeclared: &[
        "DocdbAnalyticsEnabled",
        "CustomDataGroupPresent",
        "DBActivityStreamStatus",
        "GlobalClusterResourceId",
    ],
};

/// `DBInstance`: the instance envelope every operation that answers with an instance
/// carries. The Data API HTTP endpoint is a DB CLUSTER's, and the shape declares no member
/// for it on an instance.
///
/// Source: `smithy_query(query="shape", shape="DBInstance")`.
const DB_INSTANCE: MemberSet = MemberSet {
    case: "a DB instance envelope",
    declared: &[
        "DBInstanceIdentifier",
        "Engine",
        "DBInstanceStatus",
        "DBInstanceClass",
        "VPCNetworkingEnabled",
    ],
    undeclared: &["HttpEndpointEnabled"],
};

/// `DBClusterSnapshot`: the snapshot envelope every operation that answers with a cluster
/// snapshot carries.
///
/// Source: `smithy_query(query="shape", shape="DBClusterSnapshot")`.
const CLUSTER_SNAPSHOT: MemberSet = MemberSet {
    case: "a DB cluster snapshot envelope",
    declared: &[
        "DBClusterSnapshotIdentifier",
        "DBClusterIdentifier",
        "Engine",
        "Status",
        "SnapshotType",
        "StorageEncryptionType",
    ],
    undeclared: &["CustomDataGroupPresent", "FullSnapshotSizeInBytes"],
};

/// Assert one envelope carries every declared member of its shape and no undeclared name.
fn assert_member_set(case: &str, set: &MemberSet, envelope: &str) {
    let case = format!("{case}: {}", set.case);
    for member in set.declared {
        assert!(
            !texts(envelope, member).is_empty(),
            "case {case:?}: the model declares {member}, so the envelope must report it, \
             got {envelope}"
        );
    }
    for member in set.undeclared {
        let found = texts(envelope, member);
        assert!(
            found.is_empty(),
            "case {case:?}: the model declares no {member}, so nothing may report it, got \
             {found:?} in {envelope}"
        );
    }
}

/// Every entry `DescribeDBEngineVersions` answers for `engine`, deprecated ones included.
async fn engine_versions(router: &MultiServiceRouter, case: &str, engine: &str) -> Vec<String> {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBEngineVersions",
            &[
                ("Engine", engine),
                ("IncludeAll", "true"),
                ("MaxRecords", "100"),
            ],
        )
        .await,
    );
    subtrees(&body, "DBEngineVersion")
}

/// An engine-version entry names only the members `DBEngineVersion` declares, and so does
/// every upgrade target nested in it.
#[tokio::test]
async fn an_engine_version_entry_names_only_the_members_the_model_declares() {
    let router = emulator();

    // Engines whose DB instance owns its storage beside engines whose cluster manages it,
    // and one engine of each sibling control plane this one front door serves.
    let mut targets_seen = 0usize;
    for engine in [
        "mysql",
        "postgres",
        "oracle-se2",
        "aurora-mysql",
        "aurora-postgresql",
        "docdb",
        "neptune",
    ] {
        let case = format!("{engine} versions");
        let entries = engine_versions(&router, &case, engine).await;
        assert!(
            !entries.is_empty(),
            "case {case:?}: this service publishes versions for this engine, got none"
        );
        for entry in &entries {
            let case = format!("{engine} {}", text(entry, "EngineVersion"));
            assert_member_set(&case, &ENGINE_VERSION, entry);
            for target in subtrees(entry, "UpgradeTarget") {
                targets_seen += 1;
                assert_member_set(&case, &UPGRADE_TARGET, &target);
            }
        }
    }
    assert!(
        targets_seen > 0,
        "case \"every engine\": the catalogue publishes upgrade paths, so the report must \
         carry upgrade targets to read, got none"
    );
}

/// An orderable DB instance option names only the members `OrderableDBInstanceOption`
/// declares.
#[tokio::test]
async fn an_orderable_option_names_only_the_members_the_model_declares() {
    let router = emulator();

    // A class with local NVMe storage and one without, on an engine whose instance owns its
    // storage; the serverless capacity class of a cluster engine; and an engine of a
    // sibling control plane.
    for (engine, class) in [
        ("mysql", "db.m5d.large"),
        ("postgres", "db.m6i.large"),
        ("aurora-postgresql", "db.serverless"),
        ("docdb", "db.r5.large"),
    ] {
        let case = format!("{engine} on {class}");
        let body = expect_ok(
            &case,
            call(
                &router,
                "DescribeOrderableDBInstanceOptions",
                &[
                    ("Engine", engine),
                    ("DBInstanceClass", class),
                    ("MaxRecords", "100"),
                ],
            )
            .await,
        );
        let options = subtrees(&body, "OrderableDBInstanceOption");
        assert!(
            !options.is_empty(),
            "case {case:?}: this class is one this service offers for this engine, so it \
             must be reported, got {body}"
        );
        for option in &options {
            assert_member_set(&case, &ORDERABLE_OPTION, option);
        }
    }
}

/// A DB cluster, a DB instance and a DB cluster snapshot each name only the members their
/// own shape declares, on the create's answer and on the read that polls it alike.
#[tokio::test]
async fn a_cluster_an_instance_and_a_snapshot_name_only_the_members_the_model_declares() {
    let router = emulator();

    // A cluster of an engine whose cluster manages its storage, with the Data API endpoint
    // asked for so the member that reports it carries a value of its own.
    let cluster = "emu-mms-cluster";
    let case = "an aurora-mysql cluster with the Data API on";
    let created = the_cluster(
        case,
        &expect_ok(
            case,
            call(
                &router,
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", cluster),
                    ("Engine", "aurora-mysql"),
                    ("MasterUsername", "clusteradmin"),
                    ("MasterUserPassword", "a-strong-password-1"),
                    ("EnableHttpEndpoint", "true"),
                ],
            )
            .await,
        ),
    );
    assert_member_set(&format!("{case} (create)"), &DB_CLUSTER, &created);
    let ready = cluster_once_available(&router, case, cluster).await;
    assert_member_set(&format!("{case} (describe)"), &DB_CLUSTER, &ready);

    // A DB instance of an engine that owns its own storage, which is the envelope the Data
    // API member is NOT declared on.
    let instance = "emu-mms-instance";
    let instance_case = "a standalone postgres instance";
    let created = expect_ok(
        instance_case,
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", instance),
                ("Engine", "postgres"),
                ("DBInstanceClass", "db.m6i.large"),
            ],
        )
        .await,
    );
    let created = crate::db_instances::the_instance(instance_case, &created);
    assert_member_set(
        &format!("{instance_case} (create)"),
        &DB_INSTANCE,
        &created,
    );

    // A snapshot of the cluster above, read off the create that took it.
    let snapshot = "emu-mms-cluster-s1";
    let snapshot_case = "a snapshot of the aurora-mysql cluster";
    let taken = expect_ok(
        snapshot_case,
        call(
            &router,
            "CreateDBClusterSnapshot",
            &[
                ("DBClusterIdentifier", cluster),
                ("DBClusterSnapshotIdentifier", snapshot),
            ],
        )
        .await,
    );
    let taken = subtrees(&taken, "DBClusterSnapshot");
    assert_eq!(
        taken.len(),
        1,
        "case {snapshot_case:?}: the create answers with the one snapshot it took, got \
         {taken:?}"
    );
    assert_member_set(
        &format!("{snapshot_case} (create)"),
        &CLUSTER_SNAPSHOT,
        &taken[0],
    );
}
