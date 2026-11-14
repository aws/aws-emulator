// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's DB cluster slice: `CreateDBCluster` and the
//! `DescribeDBClusters` report that reads what it persisted, plus the membership a
//! `CreateDBInstance` into the cluster adds.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter`
//! exactly as the production router mounts it (through `db_instances::emulator`, the one
//! mounting helper this test target has), and each request is a real awsQuery form POST
//! carrying the SigV4 credential scope the router keys on. The assertions read the XML the
//! emulator answers with, so a handler that computed nothing could not pass them.
//!
//! The rule under test: a create persists the resource it acknowledges — every member set
//! to the value the request named, or to the default that member's own documentation gives
//! it — and every later lookup of that resource reports the same record, while a request
//! outside a documented constraint is refused with the error code the real service answers
//! it with and leaves nothing behind.
//!
//! The tables below span the classes those rules generalise over: both cluster TYPES
//! `CreateDBCluster` documents (Aurora DB clusters and Multi-AZ DB clusters), three of the
//! engines `Engine` publishes, the forms `EngineVersion` resolves from, each published
//! bound of `Port`, `BackupRetentionPeriod`, `BacktrackWindow` and the Aurora Serverless v2
//! capacity range plus one value past it, the identifier length bound of each cluster type,
//! an identifier stored lowercased and read back in another case and as an ARN, the
//! documented defaults of the boolean and window members, and the region scope a cluster is
//! visible in.

use std::thread::sleep;
use std::time::Duration;

use service_com_amazonaws_rds_amazonrdsv19::state::{
    DB_CLUSTER_QUOTA_PER_REGION, STATUS_TRANSITION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts, REGION,
};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// A second region, so the REGIONAL scope of a cluster is observable.
const OTHER_REGION: &str = "eu-west-1";

/// How long a test waits before expecting the transition this service owes a new cluster
/// to have settled: the span itself plus slack for a loaded test machine.
fn wait_for_settle() {
    sleep(STATUS_TRANSITION_DELAY + Duration::from_millis(200));
}

/// One row of the create/describe round trip.
struct ClusterCase {
    case: &'static str,
    /// The `DBClusterIdentifier` the create names.
    identifier: &'static str,
    /// What the identifier is stored and reported as: "this parameter is stored as a
    /// lowercase string".
    stored_identifier: &'static str,
    engine: &'static str,
    /// The `EngineVersion` the create names, if any.
    requested_version: Option<&'static str>,
    /// The version the cluster runs, resolved against the engine-version catalogue
    /// `DescribeDBEngineVersions` publishes.
    engine_version: &'static str,
    /// The DB cluster parameter group the cluster reports: the default group of the
    /// resolved version's family, where the request names none.
    parameter_group: &'static str,
    /// `BackupRetentionPeriod` as reported — "Default: 1".
    expected_backup_retention: &'static str,
    /// `StorageEncryptionType` as reported, which follows from `StorageEncrypted` and the
    /// cluster type.
    expected_encryption_type: &'static str,
    /// Whether the cluster reports an HTTP (RDS Data API) endpoint.
    expected_http_endpoint: bool,
    /// The parameters this row adds beyond `DBClusterIdentifier` and `Engine`.
    extra: &'static [(&'static str, &'static str)],
    /// The elements this row expects to find, as `(element, text)`.
    expected: &'static [(&'static str, &'static str)],
}

/// The clusters the round trip creates and reads back.
///
/// The rows differ along what the rule generalises over: the cluster type and engine, the
/// form `EngineVersion` takes (absent, an exact catalogue version, a bare major version),
/// whether the Aurora Serverless v2 capacity range is named, the case of the identifier,
/// each published bound of the numeric members, and whether the booleans are asked for or
/// left at their documented defaults.
const CLUSTER_CASES: &[ClusterCase] = &[
    ClusterCase {
        case: "aurora-mysql, serverless v2 capacity, Data API endpoint, encrypted",
        identifier: "emu-cluster-dsi",
        stored_identifier: "emu-cluster-dsi",
        engine: "aurora-mysql",
        requested_version: None,
        // The engine's own default available version, which is what RDS provisions for a
        // request that names no version.
        engine_version: "8.0.mysql_aurora.3.10.3",
        parameter_group: "default.aurora-mysql8.0",
        expected_backup_retention: "1",
        expected_encryption_type: "sse-kms",
        expected_http_endpoint: true,
        extra: &[
            ("MasterUsername", "appsyncadmin"),
            ("MasterUserPassword", "a-strong-password"),
            ("DatabaseName", "testdb"),
            ("EnableHttpEndpoint", "true"),
            ("StorageEncrypted", "true"),
            ("ServerlessV2ScalingConfiguration.MinCapacity", "0.5"),
            ("ServerlessV2ScalingConfiguration.MaxCapacity", "2.0"),
        ],
        expected: &[
            ("DatabaseName", "testdb"),
            ("MasterUsername", "appsyncadmin"),
            ("StorageEncrypted", "true"),
            ("MinCapacity", "0.5"),
            // A double carries a fractional part on the wire, so the `2.0` the request
            // named is reported as `2.0` rather than as `2`.
            ("MaxCapacity", "2.0"),
            // The documented defaults of the members this row leaves out.
            ("DeletionProtection", "false"),
            ("CopyTagsToSnapshot", "false"),
            ("AutoMinorVersionUpgrade", "true"),
            ("IAMDatabaseAuthenticationEnabled", "false"),
            ("BacktrackWindow", "0"),
            ("MonitoringInterval", "0"),
            ("DatabaseInsightsMode", "standard"),
            ("NetworkType", "IPV4"),
            ("EngineMode", "provisioned"),
            ("ClusterScalabilityType", "standard"),
            ("LocalWriteForwardingStatus", "disabled"),
            ("PreferredBackupWindow", "07:00-07:30"),
            ("PreferredMaintenanceWindow", "mon:08:00-mon:08:30"),
        ],
    },
    ClusterCase {
        case: "aurora-postgresql, exact version, lowest Port, every boolean on, mixed case",
        identifier: "EMU-Cluster-PG",
        stored_identifier: "emu-cluster-pg",
        engine: "aurora-postgresql",
        requested_version: Some("15.8"),
        engine_version: "15.8",
        parameter_group: "default.aurora-postgresql15",
        // The published lower bound of `BackupRetentionPeriod` ("a value from 1 to 35").
        expected_backup_retention: "1",
        expected_encryption_type: "sse-kms",
        expected_http_endpoint: true,
        extra: &[
            ("Port", "1150"),
            ("BackupRetentionPeriod", "1"),
            ("StorageEncrypted", "true"),
            ("EnableHttpEndpoint", "true"),
            ("DeletionProtection", "true"),
            ("CopyTagsToSnapshot", "true"),
            ("EnableIAMDatabaseAuthentication", "true"),
            ("EnableLocalWriteForwarding", "true"),
            ("AutoMinorVersionUpgrade", "false"),
            ("NetworkType", "DUAL"),
            ("CharacterSetName", "UTF8"),
            ("PreferredBackupWindow", "03:00-03:30"),
            ("PreferredMaintenanceWindow", "sun:05:00-sun:05:30"),
            ("Tags.Tag.1.Key", "owner"),
            ("Tags.Tag.1.Value", "platform"),
        ],
        expected: &[
            ("DeletionProtection", "true"),
            ("CopyTagsToSnapshot", "true"),
            ("IAMDatabaseAuthenticationEnabled", "true"),
            ("AutoMinorVersionUpgrade", "false"),
            ("LocalWriteForwardingStatus", "enabled"),
            ("NetworkType", "DUAL"),
            ("CharacterSetName", "UTF8"),
            ("PreferredBackupWindow", "03:00-03:30"),
            ("PreferredMaintenanceWindow", "sun:05:00-sun:05:30"),
            ("Key", "owner"),
            ("Value", "platform"),
        ],
    },
    ClusterCase {
        case: "aurora-mysql, bare major version, highest Port and retention, backtracking",
        identifier: "emu-cluster-bt",
        stored_identifier: "emu-cluster-bt",
        engine: "aurora-mysql",
        requested_version: Some("5.7"),
        // A bare major version resolves to that major version's default.
        engine_version: "5.7.mysql_aurora.2.11.6",
        parameter_group: "default.aurora-mysql5.7",
        // The published upper bound of `BackupRetentionPeriod`.
        expected_backup_retention: "35",
        // Aurora storage is encrypted either way: without KMS encryption the key is the
        // service's own.
        expected_encryption_type: "sse-rds",
        expected_http_endpoint: false,
        extra: &[
            ("Port", "65535"),
            ("BackupRetentionPeriod", "35"),
            // The published upper bound of `BacktrackWindow` ("a number from 0 to
            // 259,200").
            ("BacktrackWindow", "259200"),
            ("AvailabilityZones.AvailabilityZone.1", "us-west-2b"),
            ("AvailabilityZones.AvailabilityZone.2", "us-west-2c"),
            ("DBClusterParameterGroupName", "default.aurora-mysql5.7"),
            ("OptionGroupName", "default:aurora-mysql-5-7"),
            ("Domain", "d-9067f6d4bc"),
            ("DomainIAMRoleName", "rds-directoryservice-access"),
            (
                "AssociatedRoles.DBClusterAssociatedRole.1.RoleArn",
                "arn:aws:iam::000000000000:role/emu-s3-import",
            ),
            ("AssociatedRoles.DBClusterAssociatedRole.1.FeatureName", "s3Import"),
        ],
        expected: &[
            ("BacktrackWindow", "259200"),
            ("StorageEncrypted", "false"),
            ("HttpEndpointEnabled", "false"),
            ("DBClusterOptionGroupName", "default:aurora-mysql-5-7"),
            ("Domain", "d-9067f6d4bc"),
            ("IAMRoleName", "rds-directoryservice-access"),
            ("RoleArn", "arn:aws:iam::000000000000:role/emu-s3-import"),
            ("FeatureName", "s3Import"),
            // An association is in place from the create onwards.
            ("Status", "ACTIVE"),
        ],
    },
    ClusterCase {
        case: "mysql: a Multi-AZ DB cluster, with the four settings it requires",
        identifier: "emu-cluster-multiaz",
        stored_identifier: "emu-cluster-multiaz",
        engine: "mysql",
        requested_version: None,
        engine_version: "8.4.4",
        parameter_group: "default.mysql8.4",
        expected_backup_retention: "1",
        // A Multi-AZ DB cluster created without encryption is not encrypted at all.
        expected_encryption_type: "none",
        expected_http_endpoint: false,
        extra: &[
            ("AllocatedStorage", "100"),
            ("DBClusterInstanceClass", "db.m6gd.xlarge"),
            ("StorageType", "io1"),
            ("Iops", "1000"),
            ("DBSubnetGroupName", "default"),
            ("PubliclyAccessible", "false"),
            ("MonitoringInterval", "60"),
            ("EnablePerformanceInsights", "true"),
        ],
        expected: &[
            ("AllocatedStorage", "100"),
            ("DBClusterInstanceClass", "db.m6gd.xlarge"),
            ("StorageType", "io1"),
            ("Iops", "1000"),
            ("DBSubnetGroup", "default"),
            ("PubliclyAccessible", "false"),
            ("MonitoringInterval", "60"),
            ("PerformanceInsightsEnabled", "true"),
        ],
    },
];

/// `CreateDBCluster` for one row of the table.
async fn create_case(router: &MultiServiceRouter, row: &ClusterCase) -> (u16, String) {
    let mut params: Vec<(&str, String)> = vec![
        ("DBClusterIdentifier", row.identifier.to_string()),
        ("Engine", row.engine.to_string()),
    ];
    if let Some(version) = row.requested_version {
        params.push(("EngineVersion", version.to_string()));
    }
    params.extend(
        row.extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    call(router, "CreateDBCluster", &params).await
}

/// The one `<DBCluster>` subtree in `xml`.
///
/// `pub(crate)` so the sibling suites of this one test target read a cluster envelope the
/// same way rather than each writing the check again.
pub(crate) fn the_cluster(case: &str, xml: &str) -> String {
    let found = subtrees(xml, "DBCluster");
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one DBCluster, got {} in {xml}",
        found.len()
    );
    found.into_iter().next().expect("checked just above")
}

/// `DescribeDBClusters` for one identifier, as the one `<DBCluster>` it reports.
///
/// `pub(crate)` for the same reason as [`the_cluster`].
pub(crate) async fn describe_one(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> String {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBClusters",
            &[("DBClusterIdentifier", identifier)],
        )
        .await,
    );
    the_cluster(case, &body)
}

/// The query parameters of one call, owned so a table row can hold them.
type Params = Vec<(&'static str, String)>;

/// One filter row: the case, the filter the request carries, and the clusters it selects.
type FilterRow = (&'static str, Params, Vec<&'static str>);

/// One refusal row: the case, the create's parameters, and the HTTP status and wire error
/// `<Code>` the real service answers it with.
type RefusalRow = (&'static str, Params, u16, &'static str);

/// The cluster's own `<Status>`.
///
/// The first `<Status>` of the subtree: the modelled `DBCluster` carries the cluster's own
/// status before the option-group, domain and security-group memberships, each of which
/// carries a `Status` of its own.
fn cluster_status(xml: &str) -> String {
    texts(xml, "Status")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("expected a cluster Status in {xml}"))
}

/// Assert every member of one row on a `<DBCluster>` subtree.
fn assert_reported(case: &str, label: &str, xml: &str, row: &ClusterCase) {
    let case = format!("{case} ({label})");
    let checks: Vec<(&str, String)> = vec![
        ("DBClusterIdentifier", row.stored_identifier.to_string()),
        (
            "DBClusterArn",
            format!(
                "arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{}",
                row.stored_identifier
            ),
        ),
        ("Engine", row.engine.to_string()),
        ("EngineVersion", row.engine_version.to_string()),
        ("DBClusterParameterGroup", row.parameter_group.to_string()),
        (
            "BackupRetentionPeriod",
            row.expected_backup_retention.to_string(),
        ),
        (
            "StorageEncryptionType",
            row.expected_encryption_type.to_string(),
        ),
        (
            "HttpEndpointEnabled",
            row.expected_http_endpoint.to_string(),
        ),
    ];
    for (element, expected) in checks {
        let actual = text(xml, element);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected <{element}> {expected:?}, got {actual:?} in {xml}"
        );
    }
    for (element, expected) in row.expected {
        let actual = texts(xml, element);
        assert!(
            actual.iter().any(|found| found == expected),
            "case {case:?}: expected <{element}> {expected:?}, got {actual:?} in {xml}"
        );
    }
    // The cluster's WRITER endpoint is the address a client dials, so a cluster whose
    // engine this emulator serves publishes the host its own engine listener is bound to —
    // the core binds on loopback — and the port that listener answers on. Its READER
    // endpoint is the address of the read-only listener beside it, written whole
    // (`host:port`) because the `Port` member beside it is the writer's: a client dials the
    // cluster's replicas there and is answered read-only.
    let endpoint = text(xml, "Endpoint");
    assert_eq!(
        endpoint, "127.0.0.1",
        "case {case:?}: expected the endpoint the cluster's engine listener is bound to, \
         got {endpoint:?} in {xml}"
    );
    let port = text(xml, "Port");
    assert!(
        port.parse::<u16>().is_ok_and(|port| port != 0),
        "case {case:?}: expected the port the cluster's engine listener answers on, got \
         {port:?} in {xml}"
    );
    let reader = text(xml, "ReaderEndpoint");
    assert!(
        reader
            .strip_prefix("127.0.0.1:")
            .and_then(|port| port.parse::<u16>().ok())
            .is_some_and(|reader_port| reader_port != 0 && reader_port.to_string() != port),
        "case {case:?}: expected the address of the cluster's own read-only listener, got \
         {reader:?} beside the writer's port {port:?} in {xml}"
    );
    let resource_id = text(xml, "DbClusterResourceId");
    assert!(
        resource_id.starts_with("cluster-") && resource_id.len() > "cluster-".len(),
        "case {case:?}: expected a minted DbClusterResourceId, got {resource_id:?} in {xml}"
    );
    // The master password is `@sensitive` and RDS reports no member carrying it, so no
    // element of the answer may carry what the request sent.
    assert!(
        !xml.contains("a-strong-password"),
        "case {case:?}: the master user password must never be reported, got {xml}"
    );
}

#[tokio::test]
async fn create_persists_every_member_and_describe_reads_it_back() {
    let router = emulator();

    for row in CLUSTER_CASES {
        let created = expect_ok(row.case, create_case(&router, row).await);
        let created = the_cluster(row.case, &created);
        // A freshly created DB cluster is not usable yet.
        let status = cluster_status(&created);
        assert_eq!(
            status, "creating",
            "case {:?}: expected Status creating on create, got {status:?} in {created}",
            row.case
        );

        // The same record, read back out of the store — the round trip is what proves the
        // cluster was PERSISTED rather than echoed, which is the defect this suite pins.
        let read_back = describe_one(&router, row.case, row.identifier).await;
        for (label, xml) in [
            ("CreateDBCluster", &created),
            ("DescribeDBClusters", &read_back),
        ] {
            assert_reported(row.case, label, xml, row);
        }
        // The endpoints are minted once and stable across reads, not recomputed per
        // response.
        assert_eq!(
            text(&created, "Endpoint"),
            text(&read_back, "Endpoint"),
            "case {:?}: expected one stable cluster endpoint, got {created} and {read_back}",
            row.case
        );
        // A cluster the request named no Availability Zones for spans the three zones RDS
        // picks for it; one that named them spans those.
        let zones = texts(&read_back, "AvailabilityZone");
        let expected_zones: Vec<String> = row
            .extra
            .iter()
            .filter(|(name, _)| name.starts_with("AvailabilityZones."))
            .map(|(_, value)| (*value).to_string())
            .collect();
        let expected_zones = if expected_zones.is_empty() {
            ["a", "b", "c"]
                .iter()
                .map(|suffix| format!("{REGION}{suffix}"))
                .collect()
        } else {
            expected_zones
        };
        assert_eq!(
            zones, expected_zones,
            "case {:?}: expected the cluster's Availability Zones, got {zones:?} in \
             {read_back}",
            row.case
        );
        // An empty cluster has no member instance and no read replica, and so sits in one
        // Availability Zone as far as its members go.
        assert!(
            texts(&read_back, "DBInstanceIdentifier").is_empty(),
            "case {:?}: a cluster with no DB instance has no member, got {read_back}",
            row.case
        );
        assert_eq!(
            text(&read_back, "MultiAZ"),
            "false",
            "case {:?}: a cluster with no member instance is not multi-AZ, got {read_back}",
            row.case
        );
        // "This parameter isn't case-sensitive", and the ARN names the same cluster as the
        // identifier does.
        for lookup in [
            row.identifier.to_ascii_uppercase(),
            format!(
                "arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{}",
                row.stored_identifier
            ),
        ] {
            let found = describe_one(&router, row.case, &lookup).await;
            assert_eq!(
                text(&found, "DBClusterIdentifier"),
                row.stored_identifier,
                "case {:?}: expected {lookup} to name the same cluster, got {found}",
                row.case
            );
        }
    }

    // Every created cluster is reported by the unnamed report too, in identifier order.
    let all = expect_ok(
        "every cluster",
        call(&router, "DescribeDBClusters", &[] as &[(&str, &str)]).await,
    );
    let mut expected: Vec<&str> = CLUSTER_CASES
        .iter()
        .map(|row| row.stored_identifier)
        .collect();
    expected.sort_unstable();
    assert_eq!(
        subtrees(&all, "DBCluster")
            .iter()
            .map(|cluster| text(cluster, "DBClusterIdentifier"))
            .collect::<Vec<String>>(),
        expected,
        "expected every created cluster in identifier order, got {all}"
    );
}

#[tokio::test]
async fn a_new_cluster_reaches_available_on_its_own() {
    let router = emulator();

    let created = expect_ok(
        "create",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-cluster-status"),
                ("Engine", "aurora-mysql"),
            ],
        )
        .await,
    );
    assert_eq!(
        cluster_status(&the_cluster("create", &created)),
        "creating",
        "expected the transient status a create answers with, got {created}"
    );

    wait_for_settle();
    let settled = describe_one(&router, "after the transition", "emu-cluster-status").await;
    assert_eq!(
        cluster_status(&settled),
        "available",
        "expected the cluster to have reached available on its own, got {settled}"
    );
}

#[tokio::test]
async fn an_instance_created_in_a_cluster_becomes_its_member() {
    let router = emulator();

    expect_ok(
        "create the cluster",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-cluster-members"),
                ("Engine", "aurora-mysql"),
            ],
        )
        .await,
    );
    // The writer is created first, and in another case to show the member record resolves
    // the cluster rather than storing the request's spelling of it.
    for (identifier, zone) in [
        ("emu-cluster-writer", "us-west-2a"),
        ("emu-cluster-reader", "us-west-2b"),
    ] {
        expect_ok(
            identifier,
            call(
                &router,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", identifier),
                    ("Engine", "aurora-mysql"),
                    ("DBInstanceClass", "db.serverless"),
                    ("DBClusterIdentifier", "EMU-Cluster-Members"),
                    ("AvailabilityZone", zone),
                ],
            )
            .await,
        );
    }

    let cluster = describe_one(&router, "the cluster's members", "emu-cluster-members").await;
    let members = subtrees(&cluster, "DBClusterMember");
    assert_eq!(
        members.len(),
        2,
        "expected both instances to be members of the cluster, got {cluster}"
    );
    assert_eq!(
        members
            .iter()
            .map(|member| (
                text(member, "DBInstanceIdentifier"),
                text(member, "IsClusterWriter")
            ))
            .collect::<Vec<(String, String)>>(),
        vec![
            ("emu-cluster-writer".to_string(), "true".to_string()),
            ("emu-cluster-reader".to_string(), "false".to_string()),
        ],
        "expected the first instance created in the cluster to be its writer, got {cluster}"
    );
    // The members sit in two Availability Zones, which is what `MultiAZ` reports.
    assert_eq!(
        text(&cluster, "MultiAZ"),
        "true",
        "expected a cluster whose members span two zones to report MultiAZ, got {cluster}"
    );
    // And the instance records agree: each names the cluster it joined.
    let instance = expect_ok(
        "the member instance",
        call(
            &router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "emu-cluster-writer")],
        )
        .await,
    );
    assert_eq!(
        text(&instance, "DBClusterIdentifier"),
        "emu-cluster-members",
        "expected the instance to name the cluster it joined, got {instance}"
    );

    // An instance naming a cluster nothing created is still the modelled not-found fault.
    expect_error(
        "an instance in a cluster no create stored",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-cluster-orphan"),
                ("Engine", "aurora-mysql"),
                ("DBInstanceClass", "db.serverless"),
                ("DBClusterIdentifier", "emu-cluster-absent"),
            ],
        )
        .await,
        404,
        "DBClusterNotFoundFault",
    );
}

#[tokio::test]
async fn a_cluster_that_joins_a_global_database_becomes_its_member() {
    let router = emulator();

    expect_ok(
        "create the global cluster",
        call(
            &router,
            "CreateGlobalCluster",
            &[
                ("GlobalClusterIdentifier", "emu-global-primary"),
                ("Engine", "aurora-mysql"),
            ],
        )
        .await,
    );
    let created = expect_ok(
        "create the member cluster",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-cluster-global"),
                ("Engine", "aurora-mysql"),
                ("GlobalClusterIdentifier", "emu-global-primary"),
                ("EnableGlobalWriteForwarding", "true"),
            ],
        )
        .await,
    );
    assert_eq!(
        text(&created, "GlobalClusterIdentifier"),
        "emu-global-primary",
        "expected the cluster to name the global database it joined, got {created}"
    );
    assert_eq!(
        text(&created, "GlobalWriteForwardingRequested"),
        "true",
        "expected the write-forwarding request to be reported, got {created}"
    );

    // The global database's own record carries the membership, which is where
    // `DescribeGlobalClusters` reports it from.
    let global = expect_ok(
        "the global cluster's members",
        call(
            &router,
            "DescribeGlobalClusters",
            &[("GlobalClusterIdentifier", "emu-global-primary")],
        )
        .await,
    );
    assert_eq!(
        text(&global, "DBClusterArn"),
        format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:emu-cluster-global"),
        "expected the joined cluster as the global database's member, got {global}"
    );
    assert_eq!(
        text(&global, "IsWriter"),
        "true",
        "expected the first member of a global database to be its writer, got {global}"
    );
}

#[tokio::test]
async fn a_cluster_is_reported_only_in_the_region_it_was_created_in() {
    let router = emulator();

    expect_ok(
        "create in us-west-2",
        call_in(
            &router,
            REGION,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-cluster-regional"),
                ("Engine", "aurora-mysql"),
            ],
        )
        .await,
    );
    expect_error(
        "the same identifier, read in another region",
        call_in(
            &router,
            OTHER_REGION,
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "emu-cluster-regional")],
        )
        .await,
        404,
        "DBClusterNotFoundFault",
    );
    let elsewhere = expect_ok(
        "every cluster in another region",
        call_in(
            &router,
            OTHER_REGION,
            "DescribeDBClusters",
            &[] as &[(&str, &str)],
        )
        .await,
    );
    assert!(
        subtrees(&elsewhere, "DBCluster").is_empty(),
        "expected another region to hold no cluster, got {elsewhere}"
    );
    // One identifier can name a different cluster in each region.
    expect_ok(
        "the same identifier in another region",
        call_in(
            &router,
            OTHER_REGION,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-cluster-regional"),
                ("Engine", "aurora-postgresql"),
            ],
        )
        .await,
    );
    let there = describe_one_in(&router, OTHER_REGION, "emu-cluster-regional").await;
    assert_eq!(
        text(&there, "DBClusterArn"),
        format!("arn:aws:rds:{OTHER_REGION}:{ACCOUNT}:cluster:emu-cluster-regional"),
        "expected the other region's cluster to carry that region's ARN, got {there}"
    );
}

/// `DescribeDBClusters` for one identifier in `region`, as the one `<DBCluster>` it
/// reports.
async fn describe_one_in(router: &MultiServiceRouter, region: &str, identifier: &str) -> String {
    let body = expect_ok(
        identifier,
        call_in(
            router,
            region,
            "DescribeDBClusters",
            &[("DBClusterIdentifier", identifier)],
        )
        .await,
    );
    the_cluster(identifier, &body)
}

#[tokio::test]
async fn the_report_filters_and_pages_the_clusters_it_holds() {
    let router = emulator();

    for (identifier, engine) in [
        ("emu-filter-a", "aurora-mysql"),
        ("emu-filter-b", "aurora-postgresql"),
        ("emu-filter-c", "aurora-mysql"),
    ] {
        expect_ok(
            identifier,
            call(
                &router,
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", identifier),
                    ("Engine", engine),
                    ("Domain", "d-9067f6d4bc"),
                ],
            )
            .await,
        );
    }
    let resource_id = text(
        &describe_one(&router, "the resource id", "emu-filter-b").await,
        "DbClusterResourceId",
    );

    // One row per documented filter name, each on a value the create really stored.
    let rows: Vec<FilterRow> = vec![
        (
            "db-cluster-id by identifier",
            vec![
                ("Filters.Filter.1.Name", "db-cluster-id".to_string()),
                ("Filters.Filter.1.Values.Value.1", "emu-filter-a".to_string()),
            ],
            vec!["emu-filter-a"],
        ),
        (
            "db-cluster-id by ARN",
            vec![
                ("Filters.Filter.1.Name", "db-cluster-id".to_string()),
                (
                    "Filters.Filter.1.Values.Value.1",
                    format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:emu-filter-c"),
                ),
            ],
            vec!["emu-filter-c"],
        ),
        (
            "db-cluster-resource-id",
            vec![
                (
                    "Filters.Filter.1.Name",
                    "db-cluster-resource-id".to_string(),
                ),
                ("Filters.Filter.1.Values.Value.1", resource_id.clone()),
            ],
            vec!["emu-filter-b"],
        ),
        (
            "engine",
            vec![
                ("Filters.Filter.1.Name", "engine".to_string()),
                ("Filters.Filter.1.Values.Value.1", "aurora-mysql".to_string()),
            ],
            vec!["emu-filter-a", "emu-filter-c"],
        ),
        (
            "domain",
            vec![
                ("Filters.Filter.1.Name", "domain".to_string()),
                (
                    "Filters.Filter.1.Values.Value.1",
                    "d-9067f6d4bc".to_string(),
                ),
            ],
            vec!["emu-filter-a", "emu-filter-b", "emu-filter-c"],
        ),
        (
            "clone-group-id: no cluster here is a clone",
            vec![
                ("Filters.Filter.1.Name", "clone-group-id".to_string()),
                ("Filters.Filter.1.Values.Value.1", "any-group".to_string()),
            ],
            vec![],
        ),
    ];
    for (case, params, expected) in rows {
        let body = expect_ok(case, call(&router, "DescribeDBClusters", &params).await);
        let reported: Vec<String> = subtrees(&body, "DBCluster")
            .iter()
            .map(|cluster| text(cluster, "DBClusterIdentifier"))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the filtered clusters, got {reported:?} in {body}"
        );
    }

    // A filter name this operation does not support is refused rather than ignored.
    expect_error(
        "a filter name no cluster report supports",
        call(
            &router,
            "DescribeDBClusters",
            &[
                ("Filters.Filter.1.Name", "db-instance-id"),
                ("Filters.Filter.1.Values.Value.1", "emu-filter-a"),
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );

    // `MaxRecords` pages the report, and the marker it hands back resumes it.
    let first = expect_ok(
        "the first page",
        call(&router, "DescribeDBClusters", &[("MaxRecords", "20")]).await,
    );
    assert_eq!(
        subtrees(&first, "DBCluster").len(),
        3,
        "expected every cluster on one page of 20, got {first}"
    );
    assert_eq!(
        optional_text(&first, "Marker"),
        None,
        "expected no marker where no record remains, got {first}"
    );
    // Each published bound of `MaxRecords`, and one value past each. A value outside the
    // documented window is one the parameter does not accept, so it is refused with
    // `InvalidParameterValue`.
    for (case, value, status, code) in [
        ("the lowest MaxRecords", "20", 200, ""),
        ("the highest MaxRecords", "100", 200, ""),
        ("one below the lowest", "19", 400, "InvalidParameterValue"),
        ("one above the highest", "101", 400, "InvalidParameterValue"),
    ] {
        let response = call(&router, "DescribeDBClusters", &[("MaxRecords", value)]).await;
        if status == 200 {
            expect_ok(case, response);
        } else {
            expect_error(case, response, status, code);
        }
    }
}

#[tokio::test]
async fn a_request_outside_a_documented_constraint_is_refused_and_stores_nothing() {
    let router = emulator();

    expect_ok(
        "the cluster the duplicate and replica rows need",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-cluster-taken"),
                ("Engine", "aurora-mysql"),
            ],
        )
        .await,
    );

    // Each row: the parameters, the HTTP status and the wire error `<Code>` the real
    // service answers it with. The bounds rows name one value past a published bound.
    let rows: Vec<RefusalRow> = vec![
        (
            "an engine with no cluster form",
            vec![
                ("DBClusterIdentifier", "emu-cluster-bad".to_string()),
                ("Engine", "sqlserver-ee".to_string()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "an identifier that is not of the documented form",
            vec![
                ("DBClusterIdentifier", "1-leading-digit".to_string()),
                ("Engine", "aurora-mysql".to_string()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "an Aurora identifier one character past the 63 it allows",
            vec![
                ("DBClusterIdentifier", "e".repeat(64)),
                ("Engine", "aurora-mysql".to_string()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a Multi-AZ identifier one character past the 52 it allows",
            vec![
                ("DBClusterIdentifier", "e".repeat(53)),
                ("Engine", "mysql".to_string()),
                ("AllocatedStorage", "100".to_string()),
                ("DBClusterInstanceClass", "db.m6gd.xlarge".to_string()),
                ("StorageType", "io1".to_string()),
                ("Iops", "1000".to_string()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a second create under an identifier one cluster already has",
            vec![
                ("DBClusterIdentifier", "EMU-Cluster-Taken".to_string()),
                ("Engine", "aurora-mysql".to_string()),
            ],
            400,
            "DBClusterAlreadyExistsFault",
        ),
        (
            "one port below the lowest the member publishes",
            vec![
                ("DBClusterIdentifier", "emu-cluster-port".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("Port", "1149".to_string()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "one backup retention day below the lowest",
            vec![
                ("DBClusterIdentifier", "emu-cluster-retention".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("BackupRetentionPeriod", "0".to_string()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "one backup retention day above the highest",
            vec![
                ("DBClusterIdentifier", "emu-cluster-retention".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("BackupRetentionPeriod", "36".to_string()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "one second of backtrack window past the highest",
            vec![
                ("DBClusterIdentifier", "emu-cluster-backtrack".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("BacktrackWindow", "259201".to_string()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "backtracking on an engine that does not support it",
            vec![
                ("DBClusterIdentifier", "emu-cluster-backtrack".to_string()),
                ("Engine", "aurora-postgresql".to_string()),
                ("BacktrackWindow", "3600".to_string()),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a master username past the 16 characters it allows",
            vec![
                ("DBClusterIdentifier", "emu-cluster-user".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("MasterUsername", "a".repeat(17)),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a master password one character short of the 8 it requires",
            vec![
                ("DBClusterIdentifier", "emu-cluster-pass".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("MasterUserPassword", "short7c".to_string()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a master password carrying a character the member excludes",
            vec![
                ("DBClusterIdentifier", "emu-cluster-pass".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("MasterUserPassword", "has.a.slash/in.it".to_string()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "one capacity unit past the engine version's Serverless v2 maximum",
            vec![
                ("DBClusterIdentifier", "emu-cluster-acu".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                (
                    "ServerlessV2ScalingConfiguration.MaxCapacity",
                    "256.5".to_string(),
                ),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a capacity that is not a half-step increment",
            vec![
                ("DBClusterIdentifier", "emu-cluster-acu".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                (
                    "ServerlessV2ScalingConfiguration.MaxCapacity",
                    "1.2".to_string(),
                ),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a minimum capacity above the maximum",
            vec![
                ("DBClusterIdentifier", "emu-cluster-acu".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                (
                    "ServerlessV2ScalingConfiguration.MinCapacity",
                    "8".to_string(),
                ),
                (
                    "ServerlessV2ScalingConfiguration.MaxCapacity",
                    "4".to_string(),
                ),
            ],
            400,
            // A capacity floor above the ceiling is a VALUE the member does not accept.
            "InvalidParameterValue",
        ),
        (
            "one second below the lowest auto-pause delay",
            vec![
                ("DBClusterIdentifier", "emu-cluster-acu".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                (
                    "ServerlessV2ScalingConfiguration.SecondsUntilAutoPause",
                    "299".to_string(),
                ),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "an engine mode the resolved version does not support",
            vec![
                ("DBClusterIdentifier", "emu-cluster-mode".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("EngineMode", "serverless".to_string()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "an engine mode the member does not publish",
            vec![
                ("DBClusterIdentifier", "emu-cluster-mode".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("EngineMode", "parallelquery".to_string()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "an engine version the catalogue does not publish",
            vec![
                ("DBClusterIdentifier", "emu-cluster-version".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("EngineVersion", "8.0.99".to_string()),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "an HTTP endpoint on an engine that provides none",
            vec![
                ("DBClusterIdentifier", "emu-cluster-http".to_string()),
                ("Engine", "mysql".to_string()),
                ("AllocatedStorage", "100".to_string()),
                ("DBClusterInstanceClass", "db.m6gd.xlarge".to_string()),
                ("StorageType", "io1".to_string()),
                ("Iops", "1000".to_string()),
                ("EnableHttpEndpoint", "true".to_string()),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a Multi-AZ DB cluster missing the settings it requires",
            vec![
                ("DBClusterIdentifier", "emu-cluster-partial".to_string()),
                ("Engine", "postgres".to_string()),
                ("AllocatedStorage", "100".to_string()),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "provisioned IOPS outside the ratio the member documents",
            vec![
                ("DBClusterIdentifier", "emu-cluster-iops".to_string()),
                ("Engine", "postgres".to_string()),
                ("AllocatedStorage", "100".to_string()),
                ("DBClusterInstanceClass", "db.m6gd.xlarge".to_string()),
                ("StorageType", "io1".to_string()),
                ("Iops", "5001".to_string()),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a Multi-AZ-only setting on an Aurora cluster",
            vec![
                ("DBClusterIdentifier", "emu-cluster-aurora".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("AllocatedStorage", "100".to_string()),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a storage type the cluster type does not offer",
            vec![
                ("DBClusterIdentifier", "emu-cluster-storage".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("StorageType", "gp3".to_string()),
            ],
            400,
            "StorageTypeNotSupported",
        ),
        (
            "a network type the member does not publish",
            vec![
                ("DBClusterIdentifier", "emu-cluster-network".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("NetworkType", "IPV6".to_string()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "the advanced insights mode without what it requires",
            vec![
                ("DBClusterIdentifier", "emu-cluster-insights".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("DatabaseInsightsMode", "advanced".to_string()),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a lifecycle type the member does not publish",
            vec![
                ("DBClusterIdentifier", "emu-cluster-lifecycle".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("EngineLifecycleSupport", "open-source-rds-no-support".to_string()),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "an associated role carrying a feature name past what this emulator stores",
            vec![
                ("DBClusterIdentifier", "emu-cluster-role".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                (
                    "AssociatedRoles.DBClusterAssociatedRole.1.RoleArn",
                    "arn:aws:iam::000000000000:role/emu-s3-import".to_string(),
                ),
                (
                    "AssociatedRoles.DBClusterAssociatedRole.1.FeatureName",
                    "f".repeat(513),
                ),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a DB subnet group no create stored",
            vec![
                ("DBClusterIdentifier", "emu-cluster-subnets".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("DBSubnetGroupName", "emu-absent-subnets".to_string()),
            ],
            404,
            "DBSubnetGroupNotFoundFault",
        ),
        (
            "a global cluster no create stored",
            vec![
                ("DBClusterIdentifier", "emu-cluster-noglobal".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("GlobalClusterIdentifier", "emu-absent-global".to_string()),
            ],
            404,
            "GlobalClusterNotFoundFault",
        ),
        (
            "a replication source no create stored",
            vec![
                ("DBClusterIdentifier", "emu-cluster-replica".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                (
                    "ReplicationSourceIdentifier",
                    format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:emu-absent"),
                ),
            ],
            404,
            "DBClusterNotFoundFault",
        ),
    ];

    for (case, params, status, code) in rows {
        expect_error(case, call(&router, "CreateDBCluster", &params).await, status, code);
        // A refused create leaves nothing behind for a later read to find.
        let identifier = params
            .iter()
            .find(|(name, _)| *name == "DBClusterIdentifier")
            .map(|(_, value)| value.clone())
            .expect("every row names an identifier");
        if identifier != "EMU-Cluster-Taken" {
            expect_error(
                case,
                call(
                    &router,
                    "DescribeDBClusters",
                    &[("DBClusterIdentifier", identifier.as_str())],
                )
                .await,
                404,
                "DBClusterNotFoundFault",
            );
        }
    }
}

#[tokio::test]
async fn the_storage_type_a_cluster_reports_is_the_one_its_member_documents() {
    let router = emulator();

    // The class the rule spans: every value `StorageType` publishes for each cluster type
    // ("Aurora DB clusters - aurora | aurora-iopt1", "Multi-AZ DB clusters - io1 | io2 |
    // gp3"), the request that names none, and one value belonging to the other type. The
    // Aurora default is the one value documented as NOT reported back: "When you create an
    // Aurora DB cluster with the storage type set to aurora-iopt1, the storage type is
    // returned in the response. The storage type isn't returned when you set it to aurora."
    let rows: [(&str, &str, Option<&str>, Option<&str>); 7] = [
        (
            "an Aurora cluster that names no storage type",
            "aurora-mysql",
            None,
            None,
        ),
        (
            "an Aurora cluster on the default storage type, which is not reported",
            "aurora-mysql",
            Some("aurora"),
            None,
        ),
        (
            "an Aurora I/O-Optimized cluster, which is reported",
            "aurora-mysql",
            Some("aurora-iopt1"),
            Some("aurora-iopt1"),
        ),
        (
            "a Multi-AZ DB cluster on io1",
            "postgres",
            Some("io1"),
            Some("io1"),
        ),
        (
            "a Multi-AZ DB cluster on io2",
            "postgres",
            Some("io2"),
            Some("io2"),
        ),
        (
            "a Multi-AZ DB cluster on gp3",
            "postgres",
            Some("gp3"),
            Some("gp3"),
        ),
        (
            "an Aurora storage type on a Multi-AZ DB cluster is refused",
            "postgres",
            Some("aurora-iopt1"),
            // Refused: the Multi-AZ value list does not carry it.
            Some("!refused"),
        ),
    ];

    for (index, (case, engine, storage_type, expected)) in rows.iter().enumerate() {
        let identifier = format!("emu-storage-{index}");
        let mut params: Vec<(&str, String)> = vec![
            ("DBClusterIdentifier", identifier.clone()),
            ("Engine", (*engine).to_string()),
        ];
        if let Some(storage_type) = storage_type {
            params.push(("StorageType", (*storage_type).to_string()));
        }
        // A Multi-AZ DB cluster carries the other three settings it requires.
        if *engine == "postgres" {
            params.extend([
                ("AllocatedStorage", "100".to_string()),
                ("DBClusterInstanceClass", "db.m6gd.xlarge".to_string()),
                ("Iops", "1000".to_string()),
            ]);
        }
        let response = call(&router, "CreateDBCluster", &params).await;
        if *expected == Some("!refused") {
            expect_error(case, response, 400, "StorageTypeNotSupported");
            continue;
        }
        let created = the_cluster(case, &expect_ok(case, response));
        let read_back = describe_one(&router, case, &identifier).await;
        for (label, xml) in [
            ("CreateDBCluster", &created),
            ("DescribeDBClusters", &read_back),
        ] {
            let reported = optional_text(xml, "StorageType");
            assert_eq!(
                reported.as_deref(),
                *expected,
                "case {case:?} ({label}): expected <StorageType> {expected:?}, got \
                 {reported:?} in {xml}"
            );
        }
    }
}

#[tokio::test]
async fn a_replica_cluster_names_its_source_and_the_source_lists_it() {
    let router = emulator();

    expect_ok(
        "the source cluster",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-cluster-source"),
                ("Engine", "aurora-mysql"),
            ],
        )
        .await,
    );
    let replica = expect_ok(
        "the replica cluster",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-cluster-replica"),
                ("Engine", "aurora-mysql"),
                ("ReplicationSourceIdentifier", "emu-cluster-source"),
            ],
        )
        .await,
    );
    let source_arn = format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:emu-cluster-source");
    assert_eq!(
        text(&replica, "ReplicationSourceIdentifier"),
        source_arn,
        "expected the replica to name the ARN of the source it resolved, got {replica}"
    );
    let source = describe_one(&router, "the source cluster", "emu-cluster-source").await;
    assert_eq!(
        texts(&source, "ReadReplicaIdentifier"),
        vec!["emu-cluster-replica".to_string()],
        "expected the source to list the replica created from it, got {source}"
    );
}

#[tokio::test]
async fn the_region_quota_bounds_how_many_clusters_one_region_holds() {
    let router = emulator();

    for index in 0..DB_CLUSTER_QUOTA_PER_REGION {
        expect_ok(
            "a cluster within the quota",
            call(
                &router,
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", format!("emu-quota-{index}")),
                    ("Engine", "aurora-mysql".to_string()),
                ],
            )
            .await,
        );
    }
    expect_error(
        "one cluster past the region's quota",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-quota-past"),
                ("Engine", "aurora-mysql"),
            ],
        )
        .await,
        403,
        "DBClusterQuotaExceededFault",
    );
}
