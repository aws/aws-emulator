// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `DescribeOrderableDBInstanceOptions` reports ABOUT one option:
//! the capability members AWS answers on every orderable option, and the repeated option
//! member a class that can be ordered several ways is reported under.
//!
//! The rules under test:
//!
//! * an option is one ORDERABLE COMBINATION, so a class that can be ordered with more than
//!   one storage type is reported once per type — an Aurora cluster's types are the ones its
//!   cluster's own `StorageType` publishes — and the report is a repeated
//!   `OrderableDBInstanceOption` member rather than one record carrying a single default;
//! * every option answers the capability members the shape publishes: each is a fact about
//!   the engine, the engine version or the class, and none of them is left out — a class with
//!   no configurable processor feature reports the EMPTY list the member documents for it,
//!   and a boolean capability is reported false rather than absent.
//!
//! The tables span what the rules generalise over: the Aurora engines of this control plane
//! beside the engines of the two sibling control planes this one front door serves and a
//! non-cluster RDS engine, the serverless capacity class beside family-and-size classes, and
//! a class with local NVMe storage beside one without.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly as
//! the production router mounts it, each request is a real awsQuery form POST carrying the
//! SigV4 credential scope the router keys on, and every assertion reads the XML the emulator
//! answered with.

use service_com_amazonaws_rds_amazonrdsv19::state::db_cluster_storage_types;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_ok, optional_text, subtrees, text, texts};

/// Every option the report answers for one engine, optionally narrowed to one class.
async fn options(
    router: &MultiServiceRouter,
    case: &str,
    engine: &str,
    class: Option<&str>,
) -> Vec<String> {
    let mut params: Vec<(&str, &str)> = vec![("Engine", engine), ("MaxRecords", "1000")];
    if let Some(class) = class {
        params.push(("DBInstanceClass", class));
    }
    let body = expect_ok(case, call(router, "DescribeOrderableDBInstanceOptions", &params).await);
    let found = subtrees(&body, "OrderableDBInstanceOption");
    assert!(
        !found.is_empty(),
        "case {case:?}: the engine is one this service offers, so it must report options, \
         got {body}"
    );
    found
}

/// A class of an AURORA engine is offered once per storage type the cluster's own
/// `StorageType` publishes, under every engine version the class is offered for — so the
/// report is a repeated member rather than one option carrying a single default type.
#[tokio::test]
async fn a_cluster_class_is_offered_once_per_published_cluster_storage_type() {
    let router = emulator();

    // (engine, class): the serverless capacity class and a family-and-size class, over both
    // Aurora engines. The engines of the two SIBLING control planes this front door serves
    // publish two cluster storage types as well, but an option of one of them is that
    // control plane's own catalogue entry and this door's view of it must agree with that
    // service's own view (`core/router/tests/all/rds_front_door_shared_catalogs.rs` holds the
    // two to one answer), so they are not rows here.
    for (engine, class) in [
        ("aurora-postgresql", "db.serverless"),
        ("aurora-postgresql", "db.r6g.large"),
        ("aurora-mysql", "db.serverless"),
        ("aurora-mysql", "db.t3.medium"),
    ] {
        let case = format!("{engine} {class}");
        let found = options(&router, &case, engine, Some(class)).await;

        // The types the CLUSTER's own member publishes, which is what an order picks
        // between; read from the state the create validates against, so the catalogue and
        // the create cannot disagree.
        let mut published: Vec<String> = db_cluster_storage_types(engine)
            .iter()
            .map(|storage_type| (*storage_type).to_string())
            .collect();
        published.sort();
        assert!(
            published.len() > 1,
            "case {case:?}: this engine's cluster publishes more than one storage type, so \
             the report must repeat the option; got {published:?}"
        );

        // One option per (engine version, storage type), with no version missing a type.
        let mut by_version: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for option in &found {
            assert_eq!(
                text(option, "DBInstanceClass"),
                class,
                "case {case:?}: DBInstanceClass narrows the report, got {option}"
            );
            by_version
                .entry(text(option, "EngineVersion"))
                .or_default()
                .push(text(option, "StorageType"));
        }
        for (version, reported) in &by_version {
            let mut reported = reported.clone();
            reported.sort();
            assert_eq!(
                reported, published,
                "case {case:?}: version {version:?} must be offered once per published \
                 storage type"
            );
        }
    }
}

/// One capability an option reports, as the table below reads it.
struct CapabilityCase {
    /// The case name.
    case: &'static str,
    /// The engine the option is of.
    engine: &'static str,
    /// The class the option is of.
    class: &'static str,
    /// `(member, the value every option of this engine and class must report)`.
    expected: &'static [(&'static str, &'static str)],
    /// Whether `AvailableProcessorFeatures` must be reported EMPTY (a class with no
    /// configurable processor feature) rather than carrying features.
    empty_processor_features: bool,
    /// The members of `SupportedActivityStreamModes`, in the order they are reported.
    activity_stream_modes: &'static [&'static str],
}

/// The rows: the capability answers this front door publishes per engine and class.
///
/// Source: each member's own documentation on `OrderableDBInstanceOption`, plus the
/// capability's own published scope — the Data API HTTP endpoint of an Aurora cluster, the
/// Multi-AZ DB cluster deployment of the general-purpose and memory-optimized families, and
/// the Database Activity Streams modes the member itself lists per engine.
const CAPABILITIES: &[CapabilityCase] = &[
    CapabilityCase {
        case: "an Aurora PostgreSQL serverless option",
        engine: "aurora-postgresql",
        class: "db.serverless",
        expected: &[
            // The Data API serves an Aurora cluster's HTTP endpoint.
            ("SupportsHttpEndpoint", "true"),
            // An instance of a cluster engine is only ever a member of its cluster.
            ("SupportsClusters", "true"),
            ("SupportsGlobalDatabases", "true"),
            // Kerberos authentication is published for this engine.
            ("SupportsKerberosAuthentication", "true"),
            // No class of this engine takes additional storage volumes.
            ("SupportsAdditionalStorageVolumes", "false"),
            // Aurora storage is the cluster's, so no per-instance storage is ordered.
            ("SupportsIops", "false"),
            ("SupportsStorageAutoscaling", "false"),
        ],
        empty_processor_features: true,
        // A database activity stream is collected from the instance's own host, which the
        // serverless capacity class does not give the caller.
        activity_stream_modes: &[],
    },
    CapabilityCase {
        case: "an Aurora PostgreSQL provisioned option",
        engine: "aurora-postgresql",
        class: "db.r6g.large",
        expected: &[
            ("SupportsHttpEndpoint", "true"),
            ("SupportsKerberosAuthentication", "true"),
            ("SupportsClusters", "true"),
        ],
        empty_processor_features: true,
        // "Aurora PostgreSQL returns the value [sync, async]."
        activity_stream_modes: &["sync", "async"],
    },
    CapabilityCase {
        case: "an Aurora MySQL provisioned option",
        engine: "aurora-mysql",
        class: "db.r6g.large",
        expected: &[
            ("SupportsHttpEndpoint", "true"),
            // An instance of a cluster engine is only ever a member of its cluster.
            ("SupportsClusters", "true"),
        ],
        empty_processor_features: true,
        // "Aurora MySQL and RDS for Oracle return [async] only."
        activity_stream_modes: &["async"],
    },
    CapabilityCase {
        case: "an RDS for MySQL option on a class with local NVMe storage",
        engine: "mysql",
        class: "db.m5d.large",
        expected: &[
            // No cluster of this engine has a Data API endpoint.
            ("SupportsHttpEndpoint", "false"),
            // The local NVMe generations of the general-purpose and memory-optimized
            // families are the ones a Multi-AZ DB cluster is offered on.
            ("SupportsClusters", "true"),
            ("SupportsKerberosAuthentication", "true"),
        ],
        empty_processor_features: true,
        // Database Activity Streams is not offered on this engine.
        activity_stream_modes: &[],
    },
    CapabilityCase {
        case: "an RDS for MySQL option on a class without local NVMe storage",
        engine: "mysql",
        class: "db.m6i.large",
        expected: &[("SupportsClusters", "false")],
        empty_processor_features: true,
        activity_stream_modes: &[],
    },
    CapabilityCase {
        // A COMPUTE-optimized class with local NVMe storage: the capabilities AWS publishes
        // on whole general-purpose and memory-optimized families are offered on none of
        // them, local storage or not.
        case: "an RDS for MySQL option on a compute-optimized class with local NVMe storage",
        engine: "mysql",
        class: "db.c6gd.12xlarge",
        expected: &[("SupportsClusters", "false")],
        empty_processor_features: true,
        activity_stream_modes: &[],
    },
    CapabilityCase {
        case: "an RDS for Db2 option",
        engine: "db2-se",
        class: "db.m6i.large",
        expected: &[
            // The Db2 engines publish no domain membership, so no Kerberos.
            ("SupportsKerberosAuthentication", "false"),
            ("SupportsClusters", "false"),
        ],
        empty_processor_features: true,
        activity_stream_modes: &[],
    },
];

/// Every option of a row reports the row's capability answers, carries the processor-feature
/// list the class has (empty where it has none) and the activity-stream modes its engine and
/// class publish.
#[tokio::test]
async fn every_option_answers_the_capability_members_the_shape_publishes() {
    let router = emulator();

    for row in CAPABILITIES {
        let found = options(&router, row.case, row.engine, Some(row.class)).await;
        for option in &found {
            for (member, value) in row.expected {
                let reported = optional_text(option, member);
                assert_eq!(
                    reported.as_deref(),
                    Some(*value),
                    "case {:?}: expected {member} {value:?}, got {reported:?} in {option}",
                    row.case
                );
            }

            let features = subtrees(option, "AvailableProcessorFeatures");
            assert_eq!(
                features.len(),
                1,
                "case {:?}: the processor features are reported once, got {option}",
                row.case
            );
            if row.empty_processor_features {
                assert!(
                    texts(&features[0], "Name").is_empty(),
                    "case {:?}: a class with no configurable processor feature reports the \
                     empty list, got {option}",
                    row.case
                );
            }

            let modes = subtrees(option, "SupportedActivityStreamModes");
            assert_eq!(
                modes.len(),
                1,
                "case {:?}: the activity stream modes are reported once, got {option}",
                row.case
            );
            assert_eq!(
                texts(&modes[0], "member"),
                row.activity_stream_modes
                    .iter()
                    .map(|mode| (*mode).to_string())
                    .collect::<Vec<String>>(),
                "case {:?}: expected the published activity stream modes, got {option}",
                row.case
            );

            // Every option is offered for the network types a resource of this door's own
            // engines can be placed on.
            let network_types = subtrees(option, "SupportedNetworkTypes");
            assert_eq!(
                texts(&network_types[0], "member"),
                vec!["IPV4".to_string(), "DUAL".to_string()],
                "case {:?}: expected both published network types, in the order the value \
                 list names them, got {option}",
                row.case
            );
        }
    }
}
