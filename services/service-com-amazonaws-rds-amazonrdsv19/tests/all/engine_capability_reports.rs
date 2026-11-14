// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two rules the published reference reports of this front door
//! obey about VALUES that are the engine's and about ORDER that is the catalog's.
//!
//! 1. An orderable DB instance option's instance-level capability members —
//!    `MultiAZCapable`, `ReadReplicaCapable`, `SupportsEnhancedMonitoring`,
//!    `SupportsPerformanceInsights` and `SupportsIAMDatabaseAuthentication` — are the
//!    ENGINE's own, not one value this front door reports for every option: an engine
//!    whose instances are cluster members is deployed and replicated by its cluster, and
//!    an engine of another control plane served by this same door
//!    (`src/service.rs`) publishes the capability set that control plane's own API
//!    documents. The same answer has to come back from both reports that publish it, so
//!    `ReadReplicaCapable` is cross-checked against the `SupportsReadReplica` of the
//!    engine version the option is offered for.
//! 2. `DescribeEventCategories` publishes its catalog in the catalog's own order — the
//!    source-type entries in the sequence the service publishes them and each entry's
//!    categories in that entry's own sequence — with no sort imposed on either list, so a
//!    client that indexes the report positionally reads the source type the service names
//!    at that position.
//!
//! The tables span what each rule generalises over: rule 1 over the engine kinds (an RDS
//! DB instance engine, one of its burstable sizes, an RDS engine without IAM database
//! authentication, an Aurora cluster engine, and the document and graph engines of the two
//! sibling control planes) and rule 2 over every published source type, including ones no
//! issue cited.
//!
//! Every call goes over the wire through the router the production emulator mounts
//! (`crate::db_instances::emulator`).

use crate::db_instances::{call, emulator, expect_ok, subtrees, text, texts};

/// One engine, one DB instance class it is offered on, and the instance-level
/// capabilities an option of that pair reports.
struct Capabilities {
    case: &'static str,
    engine: &'static str,
    /// A class this report offers that engine, so the row reads a real option.
    class: &'static str,
    /// `MultiAZCapable`: false for a cluster engine, whose members' zones are the
    /// cluster's ("This setting doesn't apply to Amazon Aurora because the DB instance
    /// Availability Zones (AZs) are managed by the DB cluster", on
    /// `CreateDBInstanceMessage$MultiAZ`).
    multi_az_capable: bool,
    /// `ReadReplicaCapable`: whether the engine version serves an RDS read replica — a DB
    /// instance of its own, created by `CreateDBInstanceReadReplica`.
    read_replica_capable: bool,
    /// `SupportsEnhancedMonitoring`: the OS-metric stream this control plane's engines
    /// publish; neither engine of the two sibling control planes offers it.
    supports_enhanced_monitoring: bool,
    /// `SupportsPerformanceInsights`: the engine's answer first, then the class's — the
    /// two smallest burstable sizes are not offered it, and the graph engine's own API
    /// documents the member as "(Not supported by Neptune)".
    supports_performance_insights: bool,
    /// `SupportsIAMDatabaseAuthentication`.
    supports_iam_database_authentication: bool,
}

/// The rows, one per engine kind this rule distinguishes.
///
/// `db.t4g.micro` is here because Performance Insights is a CLASS answer as well as an
/// engine one; `oracle-se2` because an RDS engine can lack IAM database authentication and
/// a read replica while still being Multi-AZ capable; `aurora-postgresql`, `docdb` and
/// `neptune` because an instance of a cluster engine is a member of its cluster.
const CAPABILITIES: &[Capabilities] = &[
    Capabilities {
        case: "an RDS DB instance engine",
        engine: "mysql",
        class: "db.m6gd.8xlarge",
        multi_az_capable: true,
        read_replica_capable: true,
        supports_enhanced_monitoring: true,
        supports_performance_insights: true,
        supports_iam_database_authentication: true,
    },
    Capabilities {
        case: "a burstable size Performance Insights is not offered on",
        engine: "mysql",
        class: "db.t4g.micro",
        multi_az_capable: true,
        read_replica_capable: true,
        supports_enhanced_monitoring: true,
        supports_performance_insights: false,
        supports_iam_database_authentication: true,
    },
    Capabilities {
        case: "an RDS engine with neither IAM database authentication nor a read replica",
        engine: "oracle-se2",
        class: "db.r5.large",
        multi_az_capable: true,
        read_replica_capable: false,
        supports_enhanced_monitoring: true,
        supports_performance_insights: true,
        supports_iam_database_authentication: false,
    },
    Capabilities {
        case: "an Aurora cluster engine",
        engine: "aurora-postgresql",
        class: "db.r6g.2xlarge",
        multi_az_capable: false,
        read_replica_capable: false,
        supports_enhanced_monitoring: true,
        supports_performance_insights: true,
        supports_iam_database_authentication: true,
    },
    Capabilities {
        case: "the document engine of a sibling control plane",
        engine: "docdb",
        class: "db.r5.2xlarge",
        multi_az_capable: false,
        read_replica_capable: true,
        supports_enhanced_monitoring: false,
        supports_performance_insights: true,
        supports_iam_database_authentication: false,
    },
    Capabilities {
        case: "the graph engine of a sibling control plane",
        engine: "neptune",
        class: "db.serverless",
        multi_az_capable: false,
        read_replica_capable: false,
        supports_enhanced_monitoring: false,
        supports_performance_insights: false,
        supports_iam_database_authentication: true,
    },
];

/// Every orderable option of `engine` on `class`.
async fn options(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    engine: &str,
    class: &str,
) -> Vec<String> {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeOrderableDBInstanceOptions",
            &[
                ("Engine", engine.to_string()),
                ("DBInstanceClass", class.to_string()),
                ("MaxRecords", "1000".to_string()),
            ],
        )
        .await,
    );
    let options = subtrees(&body, "OrderableDBInstanceOption");
    assert!(
        !options.is_empty(),
        "case {case:?}: {engine} is offered on {class}, so the report must carry its \
         options, got {body}"
    );
    options
}

#[tokio::test]
async fn an_options_instance_level_capabilities_are_its_engines() {
    let router = emulator();

    for row in CAPABILITIES {
        let case = format!("{} ({} {})", row.case, row.engine, row.class);
        for option in options(&router, &case, row.engine, row.class).await {
            for (member, expected) in [
                ("MultiAZCapable", row.multi_az_capable),
                ("ReadReplicaCapable", row.read_replica_capable),
                (
                    "SupportsEnhancedMonitoring",
                    row.supports_enhanced_monitoring,
                ),
                (
                    "SupportsPerformanceInsights",
                    row.supports_performance_insights,
                ),
                (
                    "SupportsIAMDatabaseAuthentication",
                    row.supports_iam_database_authentication,
                ),
            ] {
                let actual = text(&option, member);
                assert_eq!(
                    actual,
                    expected.to_string(),
                    "case {case:?}: expected {member} {expected}, got {actual:?} in {option}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_two_reports_agree_about_the_read_replica_a_version_serves() {
    let router = emulator();

    // `ReadReplicaCapable` and `SupportsReadReplica` answer the same question about the
    // same engine version, so a client that reads either must get the same answer: the
    // option report is read per (engine version, class) and compared with the engine
    // report's entry for that version.
    for row in CAPABILITIES {
        let case = format!("{} ({} {})", row.case, row.engine, row.class);
        let versions = expect_ok(
            &case,
            call(
                &router,
                "DescribeDBEngineVersions",
                &[
                    ("Engine", row.engine.to_string()),
                    ("IncludeAll", "true".to_string()),
                    ("MaxRecords", "100".to_string()),
                ],
            )
            .await,
        );
        let mut checked = 0usize;
        for option in options(&router, &case, row.engine, row.class).await {
            let engine_version = text(&option, "EngineVersion");
            let entry = subtrees(&versions, "DBEngineVersion")
                .into_iter()
                .find(|entry| text(entry, "EngineVersion") == engine_version)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: an option is offered for {engine_version:?}, so the \
                         engine report must publish that version: {versions}"
                    )
                });
            let reported = text(&entry, "SupportsReadReplica");
            assert_eq!(
                text(&option, "ReadReplicaCapable"),
                reported,
                "case {case:?}: the option for {engine_version:?} must report the read \
                 replica its engine version does ({reported:?}), got {option}"
            );
            checked += 1;
        }
        assert!(
            checked > 0,
            "case {case:?}: no option was compared, so this rule tested nothing"
        );
    }
}

/// One published event-category map: a source type and its categories, in the order the
/// report publishes them.
///
/// This table states the catalog INDEPENDENTLY of `state::EVENT_CATEGORIES`, so a change
/// that re-sorts either list fails here rather than agreeing with itself.
const PUBLISHED_CATALOG: &[(&str, &[&str])] = &[
    (
        "db-snapshot",
        &["notification", "creation", "deletion", "restoration"],
    ),
    ("db-parameter-group", &["configuration change"]),
    (
        "db-instance",
        &[
            "backup",
            "deletion",
            "availability",
            "creation",
            "low storage",
            "restoration",
            "configuration change",
            "failover",
            "maintenance",
            "failure",
            "notification",
            "read replica",
            "recovery",
            "security",
            "backtrack",
            "security patching",
        ],
    ),
    ("db-security-group", &["configuration change", "failure"]),
    ("db-cluster-snapshot", &["backup", "notification"]),
    (
        "db-cluster",
        &[
            "failover",
            "migration",
            "failure",
            "notification",
            "serverless",
            "creation",
            "deletion",
            "maintenance",
            "configuration change",
            "global-failover",
        ],
    ),
    ("custom-engine-version", &["failure"]),
    (
        "db-proxy",
        &["creation", "configuration change", "deletion", "failure"],
    ),
];

#[tokio::test]
async fn the_event_category_report_publishes_the_catalogs_own_order() {
    let router = emulator();

    let whole = expect_ok(
        "every published source type",
        call(&router, "DescribeEventCategories", &[("MaxRecords", "100")]).await,
    );
    let maps = subtrees(&whole, "EventCategoriesMap");
    let expected_source_types: Vec<String> = PUBLISHED_CATALOG
        .iter()
        .map(|(source_type, _)| (*source_type).to_string())
        .collect();
    let reported_source_types: Vec<String> = maps
        .iter()
        .map(|map| text(map, "SourceType"))
        .collect();
    assert_eq!(
        reported_source_types, expected_source_types,
        "case \"every published source type\": the entries must be published in the \
         catalog's own order, got {reported_source_types:?} in {whole}"
    );

    // The order is the catalog's, not a sort: a serializer that sorted either list would
    // answer the sorted sequence, which is NOT what the service publishes.
    let mut sorted = reported_source_types.clone();
    sorted.sort();
    assert_ne!(
        reported_source_types, sorted,
        "case \"every published source type\": the published sequence is not an \
         alphabetical one, so a sorted answer is a defect: got {reported_source_types:?}"
    );

    for ((source_type, expected), map) in PUBLISHED_CATALOG.iter().zip(maps.iter()) {
        let reported = texts(map, "EventCategory");
        assert_eq!(
            reported, *expected,
            "case {source_type:?}: expected the categories this source type publishes, in \
             that order, got {reported:?} in {map}"
        );

        // The same entry, read narrowed to its own source type: one report, one order.
        let narrowed = expect_ok(
            source_type,
            call(
                &router,
                "DescribeEventCategories",
                &[("SourceType", *source_type)],
            )
            .await,
        );
        assert_eq!(
            texts(&narrowed, "SourceType"),
            vec![(*source_type).to_string()],
            "case {source_type:?}: SourceType narrows the report to its own entry, got \
             {narrowed}"
        );
        assert_eq!(
            texts(&narrowed, "EventCategory"),
            *expected,
            "case {source_type:?}: the narrowed entry carries the same categories in the \
             same order as the whole report's, got {narrowed}"
        );
    }

    // At least one entry's own sequence is not the sorted one either, so the per-entry
    // order is pinned as the catalog's rather than as any sort of it.
    let unsorted_entries = PUBLISHED_CATALOG
        .iter()
        .filter(|(_, categories)| {
            let mut sorted = categories.to_vec();
            sorted.sort_unstable();
            sorted != *categories
        })
        .count();
    assert!(
        unsorted_entries >= 3,
        "case \"every published source type\": the published category sequences are not \
         alphabetical ones, so this rule must cover entries whose order a sort would \
         change: got {unsorted_entries}"
    );
}
