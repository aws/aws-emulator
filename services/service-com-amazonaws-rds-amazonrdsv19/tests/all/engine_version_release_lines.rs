// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the RELEASE LINES the `DescribeDBEngineVersions` catalogue
//! publishes per engine: which major-version families an engine carries, which builds of
//! a family it carries, in which order, and what each entry says about the builds it can
//! be upgraded to.
//!
//! Every call goes over the wire through the router the production emulator mounts
//! (`crate::db_instances::emulator`), and every assertion reads the XML the emulator
//! answered with.
//!
//! The rules under test, each stated over a TABLE or over the report's OWN answer rather
//! than over one recorded listing:
//!
//! 1. An engine's entries are grouped by major-version family, each family's builds
//!    reported together and in ascending build order, so the report reads as the engine's
//!    release lines rather than as an unordered set.
//! 2. A family publishes the SUCCESSIVE builds of its line: a build that is not the last
//!    of its family reports every later build of that family as a minor upgrade target,
//!    which is what a client computes a minor-version upgrade path from.
//! 3. An entry's `ValidUpgradeTarget` is exactly the engine's later entries, each
//!    classified (`AutoUpgrade`, `IsMajorVersionUpgrade`, `SupportsSnapshotUpgrade`) and
//!    each carrying the capability flags of the version it would move the caller ONTO.
//! 4. Every entry carries the whole member set this report publishes, with the values
//!    that engine and that build publish — the anchors table names one build per engine
//!    family with the members a client reads off it, including builds no issue cited.

use std::collections::BTreeMap;

use crate::db_engine_versions::{
    all_versions_of, flag, list_members, version_key, walk_every_version, Version,
};
use crate::db_instances::{emulator, optional_text, subtrees, text};

/// The members every entry of this report carries, whatever its engine: the version's
/// identity and family, the description, the CA certificates it can be launched with, its
/// status, and each capability flag a client gates on before it creates with the version.
///
/// Source: the `DBEngineVersion` members this service's model declares, plus the ones RDS
/// reports beside them (`MinorEngineVersion`, `SupportsRDSProxy`, `SupportsCluster`,
/// `SupportsDataGroups`), as `OPERATIONS.md` lists them for this operation's output.
const EVERY_ENTRY_MEMBER: [&str; 20] = [
    "Engine",
    "EngineVersion",
    "MajorEngineVersion",
    "MinorEngineVersion",
    "DBParameterGroupFamily",
    "DBEngineDescription",
    "DBEngineVersionDescription",
    "Status",
    "SupportedCACertificateIdentifiers",
    "SupportedEngineModes",
    "SupportedFeatureNames",
    "ExportableLogTypes",
    "SupportsLogExportsToCloudwatchLogs",
    "SupportsReadReplica",
    "SupportsGlobalDatabases",
    "SupportsParallelQuery",
    "SupportsBabelfish",
    "SupportsLimitlessDatabase",
    "SupportsLocalWriteForwarding",
    "SupportsIntegrations",
];

/// The members every `UpgradeTarget` of an entry carries: what the target IS, how the
/// upgrade onto it is classified, and the capabilities the caller would be running after
/// taking it.
const EVERY_TARGET_MEMBER: [&str; 11] = [
    "Engine",
    "EngineVersion",
    "MajorEngineVersion",
    "MinorEngineVersion",
    "Description",
    "AutoUpgrade",
    "IsMajorVersionUpgrade",
    "SupportsSnapshotUpgrade",
    "SupportedEngineModes",
    "SupportsGlobalDatabases",
    "SupportsParallelQuery",
];

/// One build of one engine, with the members a client reads off it.
///
/// The rows below are what the report has to publish for that engine's line; each is
/// checked against the entry the report answers for that `(engine, version)`.
struct Anchor {
    engine: &'static str,
    engine_version: &'static str,
    parameter_group_family: &'static str,
    major_engine_version: &'static str,
    /// `DBEngineVersionDescription`.
    description: &'static str,
    /// `DefaultCharacterSet`'s `CharacterSetName`, or `""` for an engine that takes no
    /// `CharacterSetName` and so names no default.
    default_character_set: &'static str,
    /// `ExportableLogTypes`.
    exportable_log_types: &'static [&'static str],
    /// `SupportedFeatureNames`.
    feature_names: &'static [&'static str],
    /// Builds of this engine that must be among this entry's `ValidUpgradeTarget`.
    upgrade_targets: &'static [&'static str],
}

/// The log types the Db2 engines export to CloudWatch Logs.
const DB2_LOG_TYPES: &[&str] = &["diag.log", "notify.log"];

/// The `FeatureName` the Db2, Oracle and SQL Server engines associate an IAM role under
/// for the Amazon S3 integration.
const S3_INTEGRATION: &[&str] = &["S3_INTEGRATION"];

/// The log types the Aurora MySQL engine exports to CloudWatch Logs.
const MYSQL_LOG_TYPES: &[&str] = &["audit", "error", "general", "slowquery"];

/// The `FeatureName` values Aurora MySQL associates an IAM role under: the machine learning
/// integrations, and NO S3 feature — that engine's S3 load and unload are configured through
/// the DB cluster parameter group's import-role parameters, with the role associated under
/// no feature at all.
const AURORA_MYSQL_FEATURES: &[&str] = &["Bedrock", "Comprehend", "SageMaker"];

/// The log types the Aurora PostgreSQL engine exports to CloudWatch Logs.
const AURORA_POSTGRESQL_LOG_TYPES: &[&str] = &["iam-db-auth-error", "instance", "postgresql"];

/// The `FeatureName` values Aurora PostgreSQL associates an IAM role under.
const AURORA_POSTGRESQL_FEATURES: &[&str] = &[
    "Bedrock",
    "Comprehend",
    "Lambda",
    "SageMaker",
    "s3Export",
    "s3Import",
];

/// The default server collation the Microsoft SQL Server engines report, RDS Custom for
/// SQL Server included: `CreateDBInstance$CharacterSetName` is the SQL Server server
/// collation, and this is that engine's own default.
const SQLSERVER_DEFAULT_COLLATION: &str = "SQL_Latin1_General_CP1_CI_AS";

/// One build per engine release line this suite anchors, spanning the five engine
/// families whose listings this change is about and two lines no issue named (the Aurora
/// MySQL 8.0 line, the Db2 Standard Edition 12.1 line), so the rule is asserted beyond
/// the builds that were reported.
const ANCHORS: &[Anchor] = &[
    Anchor {
        engine: "aurora-mysql",
        engine_version: "5.7.mysql_aurora.2.11.1",
        parameter_group_family: "aurora-mysql5.7",
        major_engine_version: "5.7",
        description: "Aurora (MySQL 5.7) 2.11.1",
        default_character_set: "",
        exportable_log_types: MYSQL_LOG_TYPES,
        feature_names: AURORA_MYSQL_FEATURES,
        // The successive builds of its own 2.11 line: the line does not stop at the build
        // this entry carries, so the minor upgrade path through it is readable.
        upgrade_targets: &[
            "5.7.mysql_aurora.2.11.2",
            "5.7.mysql_aurora.2.11.3",
            "5.7.mysql_aurora.2.11.6",
        ],
    },
    Anchor {
        engine: "aurora-mysql",
        engine_version: "8.0.mysql_aurora.3.10.3",
        parameter_group_family: "aurora-mysql8.0",
        major_engine_version: "8.0",
        description: "Aurora (MySQL 8.0) 3.10.3",
        default_character_set: "",
        exportable_log_types: MYSQL_LOG_TYPES,
        feature_names: AURORA_MYSQL_FEATURES,
        // The last build of the engine's last family: nothing to upgrade to.
        upgrade_targets: &[],
    },
    Anchor {
        engine: "aurora-postgresql",
        engine_version: "11.9",
        parameter_group_family: "aurora-postgresql11",
        major_engine_version: "11",
        description: "Aurora PostgreSQL (Compatible with PostgreSQL 11.9)",
        default_character_set: "",
        exportable_log_types: AURORA_POSTGRESQL_LOG_TYPES,
        feature_names: AURORA_POSTGRESQL_FEATURES,
        // A later build of its own family, and the next major-version family the engine
        // publishes — both are upgrades this engine offers from here.
        upgrade_targets: &["11.21", "12.9"],
    },
    Anchor {
        engine: "custom-sqlserver-ee",
        engine_version: "15.00.4073.23.v1",
        parameter_group_family: "custom-sqlserver-ee-15.0",
        major_engine_version: "15.00",
        description: "SQL Server 2019 15.00.4073.23.v1",
        default_character_set: SQLSERVER_DEFAULT_COLLATION,
        // RDS Custom publishes no CloudWatch log export and has no option group, so it
        // names no exportable log type and no managed feature.
        exportable_log_types: &[],
        feature_names: &[],
        upgrade_targets: &["15.00.4385.2.v1", "16.00.4165.4.v1"],
    },
    Anchor {
        engine: "db2-ae",
        engine_version: "11.5.9.0.sb00000000.r1",
        parameter_group_family: "db2-ae-11.5",
        major_engine_version: "11.5",
        description: "Db2 11.5.9.0.sb00000000.r1",
        default_character_set: "",
        exportable_log_types: DB2_LOG_TYPES,
        feature_names: S3_INTEGRATION,
        upgrade_targets: &["11.5.11.0.sb00000000.r1", "12.1.4.0.sb00080714.r1"],
    },
    Anchor {
        engine: "db2-ce",
        engine_version: "12.1.4.0.sb00080714.r1",
        parameter_group_family: "db2-ce-12.1",
        major_engine_version: "12.1",
        description: "Db2 12.1.4.0.sb00080714.r1",
        default_character_set: "",
        exportable_log_types: DB2_LOG_TYPES,
        feature_names: S3_INTEGRATION,
        upgrade_targets: &[],
    },
    Anchor {
        engine: "db2-se",
        engine_version: "12.1.4.0.sb00080714.r1",
        parameter_group_family: "db2-se-12.1",
        major_engine_version: "12.1",
        description: "Db2 12.1.4.0.sb00080714.r1",
        default_character_set: "",
        exportable_log_types: DB2_LOG_TYPES,
        feature_names: S3_INTEGRATION,
        upgrade_targets: &[],
    },
];

/// The engines whose whole release lines this suite walks: the five the reported listings
/// covered, plus two the rule holds for just as much (an RDS DB instance engine and the
/// sibling cluster engine this front door also serves).
const ENGINES: [&str; 7] = [
    "aurora-mysql",
    "aurora-postgresql",
    "custom-sqlserver-ee",
    "db2-ae",
    "db2-ce",
    "mariadb",
    "docdb",
];

/// The `<UpgradeTarget>` subtrees of one entry.
fn upgrade_targets(version: &Version) -> Vec<String> {
    subtrees(&version.xml, "UpgradeTarget")
}

#[tokio::test]
async fn an_engines_entries_are_its_families_in_ascending_build_order() {
    let router = emulator();

    // The whole report, so an engine no table here names is covered too.
    let reported = walk_every_version(&router, "every version", 100).await;
    assert!(
        !reported.is_empty(),
        "case \"every version\": the report must carry the catalogue, got nothing"
    );

    let mut by_engine: BTreeMap<String, Vec<Version>> = BTreeMap::new();
    for version in reported {
        by_engine
            .entry(version.engine.clone())
            .or_default()
            .push(version);
    }

    for (engine, builds) in &by_engine {
        // A family's builds are reported together: once another family has started, the
        // earlier one does not come back, so a client reading the report page by page
        // reads whole release lines.
        let mut started: Vec<&str> = Vec::new();
        for build in builds {
            let family = build.major_engine_version.as_str();
            if started.last() != Some(&family) {
                assert!(
                    !started.contains(&family),
                    "case {engine:?}: the builds of the {family:?} family must be \
                     reported together, got the family again after {started:?}"
                );
                started.push(family);
            }
        }

        // …and inside a family the builds count up, which is the order an upgrade path
        // through the line is read in. The key is the shared one
        // (`db_engine_versions::version_key`), so this suite and the sibling ones order
        // two builds the same way.
        for window in builds.windows(2) {
            let (earlier, later) = (&window[0], &window[1]);
            if earlier.major_engine_version != later.major_engine_version {
                continue;
            }
            let case = format!(
                "{engine} {} then {}",
                earlier.engine_version, later.engine_version
            );
            assert!(
                version_key(&earlier.engine_version) <= version_key(&later.engine_version),
                "case {case:?}: a family's builds are reported in ascending build order, \
                 got the keys {:?} then {:?}",
                version_key(&earlier.engine_version),
                version_key(&later.engine_version)
            );
        }

        // Every family of this engine carries at least one build, and the engine carries
        // at least one family: the grouping above cannot be vacuous.
        assert!(
            !started.is_empty(),
            "case {engine:?}: the report must carry this engine's release lines"
        );
    }

    // The comparison key is a real order, not one that accepts anything: the build that
    // reads later lexicographically but is EARLIER in its line sorts earlier by it.
    for (earlier, later) in [
        ("10.5.28", "10.11.9"),
        ("5.7.mysql_aurora.2.11.2", "5.7.mysql_aurora.2.11.6"),
        ("11.5.9.0.sb00000000.r1", "11.5.11.0.sb00000000.r1"),
        ("15.00.4073.23.v1", "15.00.4385.2.v1"),
    ] {
        assert!(
            version_key(earlier) < version_key(later),
            "case {earlier:?} before {later:?}: expected the build key {:?} to sort \
             before {:?}",
            version_key(earlier),
            version_key(later)
        );
    }
}

#[tokio::test]
async fn an_entrys_upgrade_targets_are_the_engines_later_builds_each_classified() {
    let router = emulator();

    for engine in ENGINES {
        let reported = all_versions_of(&router, engine).await;
        assert!(
            !reported.is_empty(),
            "case {engine:?}: the report must carry this engine's versions"
        );
        let order: Vec<&str> = reported
            .iter()
            .map(|version| version.engine_version.as_str())
            .collect();

        for (index, version) in reported.iter().enumerate() {
            let case = format!("{engine} {}", version.engine_version);
            let targets = upgrade_targets(version);
            let named: Vec<String> = targets
                .iter()
                .map(|target| text(target, "EngineVersion"))
                .collect();
            let later: Vec<String> = order[index + 1..]
                .iter()
                .map(|version| (*version).to_string())
                .collect();
            assert_eq!(
                named, later,
                "case {case:?}: an entry's upgrade targets are exactly this engine's \
                 later builds, in the report's own order"
            );

            for target in &targets {
                let target_version = text(target, "EngineVersion");
                let target_case = format!("{case} -> {target_version}");
                for member in EVERY_TARGET_MEMBER {
                    assert!(
                        optional_text(target, member).is_some(),
                        "case {target_case:?}: an upgrade target must report {member}, \
                         got {target}"
                    );
                }
                assert_eq!(
                    text(target, "Engine"),
                    engine,
                    "case {target_case:?}: an upgrade target stays on the same engine, \
                     got {target}"
                );
                let target_major = text(target, "MajorEngineVersion");
                let crosses_a_family = target_major != version.major_engine_version;
                assert_eq!(
                    flag(&target_case, target, "IsMajorVersionUpgrade"),
                    crosses_a_family,
                    "case {target_case:?}: an upgrade is a major-version upgrade exactly \
                     when it leaves the entry's family ({:?} to {target_major:?}), got \
                     {target}",
                    version.major_engine_version
                );
                // A snapshot is upgraded straight to the target only where the upgrade is
                // a major-version one; inside a family a restore needs no upgrade at all.
                assert_eq!(
                    flag(&target_case, target, "SupportsSnapshotUpgrade"),
                    crosses_a_family,
                    "case {target_case:?}: snapshot upgrade is reported for a \
                     major-version upgrade, got {target}"
                );
                // The capabilities a target reports are the TARGET's own: the entry the
                // report carries for that build says the same about it.
                let as_an_entry = reported
                    .iter()
                    .find(|entry| entry.engine_version == target_version)
                    .unwrap_or_else(|| {
                        panic!(
                            "case {target_case:?}: an upgrade target must be a build this \
                             report publishes, got {order:?}"
                        )
                    });
                for member in ["SupportsGlobalDatabases", "SupportsParallelQuery"] {
                    assert_eq!(
                        flag(&target_case, target, member),
                        flag(&target_case, &as_an_entry.scalars, member),
                        "case {target_case:?}: {member} on an upgrade target is the \
                         target build's own value, got {target}"
                    );
                }
            }

            // Inside the family, EVERY later build is offered as a minor upgrade: a line
            // whose published builds stop short of the ones the engine has released would
            // leave a client no path onto them.
            let in_family: Vec<String> = reported[index + 1..]
                .iter()
                .filter(|entry| entry.major_engine_version == version.major_engine_version)
                .map(|entry| entry.engine_version.clone())
                .collect();
            let minor: Vec<String> = targets
                .iter()
                .filter(|target| text(target, "IsMajorVersionUpgrade") == "false")
                .map(|target| text(target, "EngineVersion"))
                .collect();
            assert_eq!(
                minor, in_family,
                "case {case:?}: the minor upgrade targets are the family's own later \
                 builds"
            );
        }
    }
}

#[tokio::test]
async fn every_entry_carries_the_whole_published_member_set() {
    let router = emulator();

    for engine in ENGINES {
        for version in all_versions_of(&router, engine).await {
            let case = format!("{engine} {}", version.engine_version);
            for member in EVERY_ENTRY_MEMBER {
                assert!(
                    optional_text(&version.scalars, member).is_some(),
                    "case {case:?}: an entry must report {member}, got {}",
                    version.scalars
                );
            }
            // The CA certificates an instance of this build can be launched with: a
            // non-empty list, because a client picks its `CACertificateIdentifier` from
            // it.
            let certificates = list_members(&version.scalars, "SupportedCACertificateIdentifiers");
            assert!(
                !certificates.is_empty(),
                "case {case:?}: an entry must name the CA certificates it can be \
                 launched with, got {}",
                version.scalars
            );
            // `SupportsLogExportsToCloudwatchLogs` answers whether the list beside it has
            // anything in it, so the two cannot disagree.
            let log_types = list_members(&version.scalars, "ExportableLogTypes");
            assert_eq!(
                flag(&case, &version.scalars, "SupportsLogExportsToCloudwatchLogs"),
                !log_types.is_empty(),
                "case {case:?}: SupportsLogExportsToCloudwatchLogs must agree with \
                 ExportableLogTypes {log_types:?}, got {}",
                version.scalars
            );
        }
    }
}

#[tokio::test]
async fn each_anchored_build_is_published_with_the_members_its_engine_reports() {
    let router = emulator();

    for anchor in ANCHORS {
        let case = format!("{} {}", anchor.engine, anchor.engine_version);
        let reported = all_versions_of(&router, anchor.engine).await;
        let version = reported
            .iter()
            .find(|version| version.engine_version == anchor.engine_version)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the engine's line must publish this build, got {:?}",
                    reported
                        .iter()
                        .map(|version| version.engine_version.clone())
                        .collect::<Vec<_>>()
                )
            });

        assert_eq!(
            version.parameter_group_family, anchor.parameter_group_family,
            "case {case:?}: expected the DB parameter group family \
             {:?}, got {:?}",
            anchor.parameter_group_family, version.parameter_group_family
        );
        assert_eq!(
            version.major_engine_version, anchor.major_engine_version,
            "case {case:?}: expected the major engine version {:?}, got {:?}",
            anchor.major_engine_version, version.major_engine_version
        );
        assert_eq!(
            text(&version.scalars, "DBEngineVersionDescription"),
            anchor.description,
            "case {case:?}: expected the published version description {:?}, got {}",
            anchor.description,
            version.scalars
        );
        let default_character_set = subtrees(&version.scalars, "DefaultCharacterSet")
            .first()
            .map(|subtree| text(subtree, "CharacterSetName"))
            .unwrap_or_default();
        assert_eq!(
            default_character_set, anchor.default_character_set,
            "case {case:?}: expected the default character set {:?}, got {:?}",
            anchor.default_character_set, default_character_set
        );
        assert_eq!(
            list_members(&version.scalars, "ExportableLogTypes"),
            anchor.exportable_log_types,
            "case {case:?}: expected the engine's exportable log types {:?}, got {}",
            anchor.exportable_log_types,
            version.scalars
        );
        assert_eq!(
            list_members(&version.scalars, "SupportedFeatureNames"),
            anchor.feature_names,
            "case {case:?}: expected the engine's feature names {:?}, got {}",
            anchor.feature_names,
            version.scalars
        );

        let named: Vec<String> = upgrade_targets(version)
            .iter()
            .map(|target| text(target, "EngineVersion"))
            .collect();
        for expected in anchor.upgrade_targets {
            assert!(
                named.iter().any(|version| version == expected),
                "case {case:?}: expected {expected:?} among the builds this entry can be \
                 upgraded to, got {named:?}"
            );
        }
        if anchor.upgrade_targets.is_empty() {
            assert!(
                named.is_empty(),
                "case {case:?}: the engine's last build has nothing to upgrade to, got \
                 {named:?}"
            );
        }
    }
}

/// The Microsoft SQL Server release each engine major version belongs to, written from
/// Microsoft's published SQL Server release history — the mapping the product's own name is
/// read by. Every major version this service's catalogue carries is a row, including the
/// ones no issue cited.
const SQLSERVER_RELEASES: [(&str, &str); 4] = [
    ("13.00", "2016"),
    ("14.00", "2017"),
    ("15.00", "2019"),
    ("16.00", "2022"),
];

/// A Microsoft SQL Server version's `DBEngineVersionDescription` names the PRODUCT by its
/// release year and then the build, while `DBEngineDescription` names the engine's edition:
/// the product and the edition are different facts and a client reads both.
///
/// Asserted over every SQL Server entry of every SQL Server engine the catalogue publishes —
/// the four editions, the two Developer Edition engines and the RDS Custom ones — so the rule
/// is the engine's and not one entry's.
#[tokio::test]
async fn a_sqlserver_version_is_described_by_its_product_release() {
    let router = emulator();

    let mut checked = 0usize;
    for version in walk_every_version(&router, "every engine version", 100).await {
        if !version.engine.contains("sqlserver") {
            continue;
        }
        let case = format!("{} {}", version.engine, version.engine_version);
        let Some((_, year)) = SQLSERVER_RELEASES
            .iter()
            .find(|(major, _)| *major == version.major_engine_version)
        else {
            panic!(
                "case {case:?}: this table must name the release of every published SQL \
                 Server major version, and it does not name {:?}",
                version.major_engine_version
            );
        };
        let described = text(&version.scalars, "DBEngineVersionDescription");
        assert_eq!(
            described,
            format!("SQL Server {year} {}", version.engine_version),
            "case {case:?}: expected the product release and the build, got {described:?}"
        );
        // The EDITION is the engine's own description, not the version's — and for an RDS
        // Custom engine it names that offering's edition.
        let engine_described = text(&version.scalars, "DBEngineDescription");
        assert!(
            engine_described.contains("SQL Server") && engine_described.contains("Edition"),
            "case {case:?}: the engine is described by its edition, got {engine_described:?}"
        );
        checked += 1;
    }
    assert!(
        checked >= 20,
        "case \"every SQL Server version\": the catalogue must publish SQL Server builds for \
         this rule to cover, got {checked}"
    );
}
