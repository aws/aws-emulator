// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rows RDS's STATIC reference catalogs publish: the capability and
//! storage members of an orderable DB instance option, and the CLUSTER-level engine defaults
//! of the MySQL-compatible cluster engines.
//!
//! Every call goes over the wire through the same router the production emulator mounts
//! (`crate::db_instances::emulator`), as a real awsQuery form POST, and every assertion reads
//! the XML the emulator answered with.
//!
//! The rules under test, each stated over a TABLE spanning the class it generalises over
//! rather than over one recorded request:
//!
//! 1. A capability Amazon RDS publishes for whole DB instance class FAMILIES — the Multi-AZ DB
//!    cluster deployment, Optimized Reads and Optimized Writes — is offered on the
//!    general-purpose (`m`) and memory-optimized (`r`, `x`, `z`) families and on neither the
//!    burstable (`t`) nor the compute-optimized (`c`) ones, with the two that need the
//!    instance's own local NVMe SSD storage offered only on the `d` generations of those
//!    families.
//! 2. `SupportedNetworkTypes` reports the value list `NetworkType` publishes, in the order
//!    that list names them.
//! 3. The storage FLOOR an option publishes for a (engine, storage type) pair is the engine's
//!    own for `gp2` and the type's own for the types introduced with a floor of their own —
//!    while a create is still held to the range its own member documents, which is a
//!    different question from what the catalog publishes.
//! 4. A DB CLUSTER parameter group family of a MySQL-compatible cluster engine reports that
//!    engine's CLUSTER-level defaults — the cluster-wide `aurora_`/`aws_default_` settings,
//!    not the per-instance set — in parameter-name order, paginated, with the role-ARN
//!    parameters carrying neither a value nor a published domain.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts,
};
use crate::engine_default_parameters::reported;
use crate::orderable_db_instance_options::engine_storage_floor;

/// The orderable-options report.
const OPTIONS_ACTION: &str = "DescribeOrderableDBInstanceOptions";

/// The cluster engine-default report.
const CLUSTER_DEFAULTS_ACTION: &str = "DescribeEngineDefaultClusterParameters";

/// The per-instance engine-default report, which the cluster one must not be a copy of.
const INSTANCE_DEFAULTS_ACTION: &str = "DescribeEngineDefaultParameters";

/// Every `<OrderableDBInstanceOption>` the report answers for one engine, optionally
/// narrowed to one class, as the XML of each option.
async fn options(
    router: &MultiServiceRouter,
    case: &str,
    engine: &str,
    class: Option<&str>,
) -> Vec<String> {
    // "Default: 100 Constraints: Minimum 20, maximum 1000" — the widest page the member
    // admits, so one call carries the options of a class rather than a window onto them.
    let mut params: Vec<(&str, &str)> = vec![("Engine", engine), ("MaxRecords", "1000")];
    if let Some(class) = class {
        params.push(("DBInstanceClass", class));
    }
    let body = expect_ok(case, call(router, OPTIONS_ACTION, &params).await);
    subtrees(&body, "OrderableDBInstanceOption")
}

// ---------------------------------------------------------------------------
// Rule 1: the capabilities published per class family
// ---------------------------------------------------------------------------

/// One (engine, class) row and the three family-scoped capability answers an option of it
/// reports.
struct CapabilityRow {
    /// Why this row is in the table.
    case: &'static str,
    engine: &'static str,
    class: &'static str,
    /// `SupportsClusters`.
    clusters: &'static str,
    /// `SupportsOptimizedReads`.
    optimized_reads: &'static str,
    /// `SupportsOptimizedWrites`.
    optimized_writes: &'static str,
}

/// The rows: every class PURPOSE this service offers, with and without local NVMe SSD
/// storage, across the engines whose instance owns its storage and the engines whose cluster
/// manages it.
///
/// Source: the published scope of each capability — the Multi-AZ DB cluster deployment of
/// Amazon RDS for MySQL and PostgreSQL, Amazon RDS Optimized Reads (temporary objects on the
/// instance's local NVMe SSD storage) and Amazon RDS Optimized Writes (the MySQL-compatible
/// engines' atomic writes) — each of which AWS publishes for whole general-purpose and
/// memory-optimized class families. The classes include ones no issue cited (`db.r6gd`,
/// `db.x2iedn`, `db.m6g`) beside the two that were (`db.c6gd.12xlarge`, `db.t3.2xlarge`).
const CAPABILITY_ROWS: &[CapabilityRow] = &[
    CapabilityRow {
        case: "a compute-optimized class with local NVMe storage",
        engine: "mysql",
        class: "db.c6gd.12xlarge",
        clusters: "false",
        optimized_reads: "false",
        optimized_writes: "false",
    },
    CapabilityRow {
        case: "a burstable class",
        engine: "mysql",
        class: "db.t3.2xlarge",
        clusters: "false",
        optimized_reads: "false",
        optimized_writes: "false",
    },
    CapabilityRow {
        case: "a memory-optimized class with local NVMe storage",
        engine: "mysql",
        class: "db.r5d.4xlarge",
        clusters: "true",
        optimized_reads: "true",
        optimized_writes: "true",
    },
    CapabilityRow {
        case: "a memory-intensive class with local NVMe storage",
        engine: "mysql",
        class: "db.x2iedn.xlarge",
        clusters: "true",
        optimized_reads: "true",
        optimized_writes: "true",
    },
    CapabilityRow {
        case: "a general-purpose class without local NVMe storage",
        engine: "mysql",
        class: "db.m6g.large",
        clusters: "false",
        optimized_reads: "false",
        optimized_writes: "true",
    },
    CapabilityRow {
        case: "a general-purpose class with local NVMe storage",
        engine: "mysql",
        class: "db.m6gd.2xlarge",
        clusters: "true",
        optimized_reads: "true",
        optimized_writes: "true",
    },
    CapabilityRow {
        case: "the other engine that backs a Multi-AZ DB cluster",
        engine: "postgres",
        class: "db.r6gd.large",
        clusters: "true",
        // Optimized Writes is the MySQL-compatible engines' capability.
        optimized_reads: "true",
        optimized_writes: "false",
    },
    CapabilityRow {
        case: "an engine that backs no Multi-AZ DB cluster",
        engine: "mariadb",
        class: "db.r5d.4xlarge",
        clusters: "false",
        optimized_reads: "true",
        optimized_writes: "true",
    },
    CapabilityRow {
        case: "an engine with neither capability on a local-NVMe class",
        engine: "oracle-se2",
        class: "db.r5d.4xlarge",
        clusters: "false",
        optimized_reads: "true",
        optimized_writes: "false",
    },
    CapabilityRow {
        case: "a cluster engine, whose instance is only ever a cluster member",
        engine: "aurora-mysql",
        class: "db.r6g.large",
        clusters: "true",
        // A cluster member's temporary objects are not placed on the host it runs on.
        optimized_reads: "false",
        optimized_writes: "false",
    },
];

/// Each capability is answered for the class FAMILY's published purpose and local storage,
/// not for the spelling of one class name.
#[tokio::test]
async fn the_family_scoped_capabilities_follow_the_classs_purpose_and_local_storage() {
    let router = emulator();

    for row in CAPABILITY_ROWS {
        let case = format!("{}: {} on {}", row.case, row.engine, row.class);
        let found = options(&router, &case, row.engine, Some(row.class)).await;
        assert!(
            !found.is_empty(),
            "case {case:?}: the class is one this service offers for this engine, so it must \
             be reported"
        );
        for option in &found {
            for (member, expected) in [
                ("SupportsClusters", row.clusters),
                ("SupportsOptimizedReads", row.optimized_reads),
                ("SupportsOptimizedWrites", row.optimized_writes),
            ] {
                let reported = optional_text(option, member);
                assert_eq!(
                    reported.as_deref(),
                    Some(expected),
                    "case {case:?}: expected {member} {expected:?}, got {reported:?} in \
                     {option}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the network types, in the published order
// ---------------------------------------------------------------------------

/// Every option reports the network types a resource can be placed on in the order
/// `NetworkType` publishes them ("IPV4 | DUAL") — the same order for an engine of this
/// door's own control plane and for an engine of another's.
#[tokio::test]
async fn an_option_reports_the_network_types_in_the_order_the_value_list_names_them() {
    let router = emulator();

    for engine in ["mysql", "postgres", "oracle-ee", "aurora-postgresql", "docdb", "neptune"] {
        let case = format!("the network types of {engine}");
        let found = options(&router, &case, engine, None).await;
        assert!(
            !found.is_empty(),
            "case {case:?}: this engine publishes orderable options"
        );
        for option in &found {
            let reported = subtrees(option, "SupportedNetworkTypes");
            assert_eq!(
                reported.len(),
                1,
                "case {case:?}: the network types are reported once, got {option}"
            );
            assert_eq!(
                texts(&reported[0], "member"),
                vec!["IPV4".to_string(), "DUAL".to_string()],
                "case {case:?}: expected the published value list in its own order, got \
                 {option}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the storage floor the catalog publishes, and the floor a create enforces
// ---------------------------------------------------------------------------

/// `MinStorageSize` is the ENGINE's own storage floor on the General Purpose SSD type `gp2`
/// — 5 GiB for the MariaDB, MySQL and PostgreSQL engines, 10 for Oracle, 20 for SQL Server —
/// and the TYPE's own on `gp3` (20 GiB) and the Provisioned IOPS SSD types (100 GiB).
#[tokio::test]
async fn the_catalog_publishes_the_engines_own_floor_for_general_purpose_gp2_storage() {
    let router = emulator();

    // One row per engine family, so the floors differ along the engine rather than along one
    // recorded request; `db2-se` is an engine no issue cited.
    for engine in ["mysql", "mariadb", "postgres", "db2-se", "oracle-se2", "sqlserver-ex"] {
        let case = format!("the storage floors of {engine}");
        let found = options(&router, &case, engine, None).await;
        assert!(
            !found.is_empty(),
            "case {case:?}: this engine publishes orderable options"
        );
        let mut seen: Vec<String> = Vec::new();
        for option in &found {
            let storage_type = text(option, "StorageType");
            let expected = match storage_type.as_str() {
                "gp2" => engine_storage_floor(engine),
                "gp3" => 20,
                "io1" | "io2" => 100,
                other => panic!("case {case:?}: an unpublished storage type {other:?}"),
            };
            let reported = optional_text(option, "MinStorageSize");
            assert_eq!(
                reported.as_deref(),
                Some(expected.to_string().as_str()),
                "case {case:?}: expected {storage_type} MinStorageSize {expected}, got \
                 {reported:?} in {option}"
            );
            if !seen.contains(&storage_type) {
                seen.push(storage_type);
            }
        }
        // The engine was reported with every per-instance type, so the loop above covered
        // each of the four floors rather than one of them.
        assert_eq!(
            seen,
            vec![
                "gp2".to_string(),
                "gp3".to_string(),
                "io1".to_string(),
                "io2".to_string()
            ],
            "case {case:?}: expected one option per published storage type"
        );
    }
}

/// The catalog's floor is not the order's: `CreateDBInstance` holds an `AllocatedStorage` to
/// the range its own member documents per storage type ("General Purpose (SSD) storage (gp2,
/// gp3): Must be an integer from 20 to 65536"), so a size under that documented minimum is
/// refused even though the catalog publishes the engine's lower floor for `gp2`.
#[tokio::test]
async fn a_create_is_held_to_the_documented_range_the_catalog_floor_does_not_widen() {
    let router = emulator();

    // (case, the size asked for, whether the create is served)
    let rows: [(&str, i32, bool); 4] = [
        ("the engine floor the gp2 catalog row publishes", 5, false),
        ("one below the documented minimum", 19, false),
        ("the documented minimum", 20, true),
        ("a size inside the documented range", 400, true),
    ];

    for (index, (case, allocated_storage, served)) in rows.into_iter().enumerate() {
        let identifier = format!("catalog-floor-{index}");
        let response = call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", identifier.as_str()),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.m5.large"),
                ("StorageType", "gp2"),
                ("AllocatedStorage", &allocated_storage.to_string()),
            ],
        )
        .await;
        if served {
            expect_ok(case, response);
            continue;
        }
        let body = expect_error(case, response, 400, "InvalidParameterValue");
        assert!(
            body.contains(&format!(
                "Invalid storage size for engine name mysql and storage type gp2: \
                 {allocated_storage}"
            )),
            "case {case:?}: expected the storage-size refusal naming the engine, the type \
             and the size, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the CLUSTER-level engine defaults of the MySQL-compatible cluster engines
// ---------------------------------------------------------------------------

/// The cluster-wide settings a MySQL-compatible cluster engine configures through its DB
/// CLUSTER parameter group, which the per-instance catalog of the same engine does not
/// publish: the `aurora_` replication, parallel-query and write-forwarding settings and the
/// `aurora_`/`aws_default_` role ARNs of its Amazon S3 and machine-learning integrations.
const CLUSTER_LEVEL_SETTINGS: [&str; 6] = [
    "aurora_binlog_replication_max_yield_seconds",
    "aurora_enable_zdr",
    "aurora_load_from_s3_role",
    "aurora_parallel_query",
    "aws_default_lambda_role",
    "aws_default_sagemaker_role",
];

/// A cluster family of a MySQL-compatible cluster engine publishes the CLUSTER-level
/// settings, in parameter-name order, and the per-instance report of the same family does
/// not.
#[tokio::test]
async fn a_mysql_cluster_family_publishes_the_cluster_level_settings() {
    let router = emulator();

    // Two release lines of the Aurora engine and the RDS engine that runs the same server in
    // a Multi-AZ DB cluster, so the rows span the families rather than one of them.
    for family in ["aurora-mysql5.7", "aurora-mysql8.0", "mysql8.0"] {
        let case = format!("the cluster defaults of {family}");
        let body = expect_ok(
            &case,
            call(
                &router,
                CLUSTER_DEFAULTS_ACTION,
                &[("DBParameterGroupFamily", family)],
            )
            .await,
        );
        let parameters = reported(&body);
        let names: Vec<String> = parameters.iter().map(|p| p.name.clone()).collect();
        for setting in CLUSTER_LEVEL_SETTINGS {
            assert!(
                names.iter().any(|name| name == setting),
                "case {case:?}: expected the cluster-level setting {setting} in {names:?}"
            );
        }
        let mut ordered = names.clone();
        ordered.sort();
        ordered.dedup();
        assert_eq!(
            names, ordered,
            "case {case:?}: expected ascending, unique parameter names, got {names:?}"
        );

        // A parameter whose value is an IAM role ARN is bounded by no published domain and
        // the engine runs it at no default, so neither member is reported.
        for parameter in &parameters {
            if !parameter.name.ends_with("_role") {
                continue;
            }
            assert_eq!(
                (parameter.value.as_deref(), parameter.allowed_values.as_deref()),
                (None, None),
                "case {case:?}: {} is an IAM role ARN, so it carries neither a value nor a \
                 domain, got {parameter:?}",
                parameter.name
            );
            assert_eq!(
                parameter.data_type.as_deref(),
                Some("string"),
                "case {case:?}: {} is an ARN, got {parameter:?}",
                parameter.name
            );
        }

        // The per-instance catalog of the same family is a DIFFERENT catalog: it publishes
        // the server's own per-instance variables and none of the cluster-wide settings.
        let instance = reported(&expect_ok(
            &case,
            call(
                &router,
                INSTANCE_DEFAULTS_ACTION,
                &[("DBParameterGroupFamily", family)],
            )
            .await,
        ));
        let instance_names: Vec<String> = instance.iter().map(|p| p.name.clone()).collect();
        for setting in CLUSTER_LEVEL_SETTINGS {
            assert!(
                !instance_names.iter().any(|name| name == setting),
                "case {case:?}: {setting} applies to the cluster as a whole, so the \
                 per-instance report must not publish it, got {instance_names:?}"
            );
        }
    }
}

/// The report is paged over the catalog's own order: a page of the documented minimum size
/// carries that many parameters and a `Marker`, and the next page resumes BEYOND the last
/// parameter the first page served.
#[tokio::test]
async fn the_cluster_defaults_page_over_the_catalogs_own_order() {
    let router = emulator();

    // "Constraints: Minimum 20, maximum 100" — the smallest page the member admits, so the
    // catalog is read in more than one page.
    const PAGE_SIZE: usize = 20;

    for family in ["aurora-mysql5.7", "aurora-mysql8.0"] {
        let case = format!("the cluster defaults of {family}, paged");
        let whole = reported(&expect_ok(
            &case,
            call(
                &router,
                CLUSTER_DEFAULTS_ACTION,
                &[("DBParameterGroupFamily", family)],
            )
            .await,
        ));
        assert!(
            whole.len() > PAGE_SIZE,
            "case {case:?}: the published cluster catalog must take more than one page of \
             {PAGE_SIZE}, got {} parameters",
            whole.len()
        );

        let first = expect_ok(
            &case,
            call(
                &router,
                CLUSTER_DEFAULTS_ACTION,
                &[
                    ("DBParameterGroupFamily", family),
                    ("MaxRecords", &PAGE_SIZE.to_string()),
                ],
            )
            .await,
        );
        let page = reported(&first);
        let marker = optional_text(&first, "Marker");
        assert_eq!(
            page.iter().map(|p| p.name.clone()).collect::<Vec<String>>(),
            whole
                .iter()
                .take(PAGE_SIZE)
                .map(|p| p.name.clone())
                .collect::<Vec<String>>(),
            "case {case:?}: the first page is the head of the catalog's own order"
        );
        assert!(
            marker.is_some(),
            "case {case:?}: records remain, so a Marker is owed, got {first}"
        );

        let second = expect_ok(
            &case,
            call(
                &router,
                CLUSTER_DEFAULTS_ACTION,
                &[
                    ("DBParameterGroupFamily", family),
                    ("MaxRecords", &PAGE_SIZE.to_string()),
                    ("Marker", marker.as_deref().unwrap_or_default()),
                ],
            )
            .await,
        );
        assert_eq!(
            reported(&second)
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<String>>(),
            whole
                .iter()
                .skip(PAGE_SIZE)
                .take(PAGE_SIZE)
                .map(|p| p.name.clone())
                .collect::<Vec<String>>(),
            "case {case:?}: the second page resumes beyond the parameter the first page \
             ended at, got {second}"
        );
    }
}
