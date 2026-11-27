// SPDX-License-Identifier: CC0-1.0

//! One front door, ONE published catalog: the read-only catalog reports of the shared `rds`
//! query API answer the same rows, in the same order and with the same per-row member set,
//! whichever of that door's service views a request is addressed at.
//!
//! Amazon RDS, Amazon DocumentDB and Amazon Neptune are three emulated views of one front
//! door. A client reaches it under the `rds` signing name every SDK sends — including an SDK
//! DocumentDB client, which signs `rds` and so lands on the Amazon RDS view — and under each
//! peer's own routing key. The catalog a view answers is therefore not that view's to invent:
//! `DescribeEventCategories`, `DescribeDBEngineVersions` and
//! `DescribeOrderableDBInstanceOptions` are reference reads of the door's own published
//! catalog, so the answer a client gets cannot depend on how it addressed the door.
//!
//! What a view answers is still the members ITS OWN model declares: the shared catalog is the
//! rows, their order, and the members both models declare. A member only the Amazon RDS
//! model declares is reported by that view and left out by the DocumentDB one rather than
//! invented there — the per-member tables below say which is which.
//!
//! The expectations below are written out rather than read back from the catalog the handlers
//! answer from, so a change to that catalog has to be made here too. Their source is the
//! published Amazon DocumentDB engine catalog and the published Amazon RDS event-category
//! table, which the emulator holds in `service_common::docdb_engine_catalog` and
//! `service_common::rds_event_categories`; the rows cover entries the issue that prompted
//! this suite did not cite (`custom-engine-version`, the 4.0 engine versions, the
//! `db.r5.large` class, the IO-optimized storage type).
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and the requests
//! driven — on an explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The awsQuery API version every view of this front door shares.
const API_VERSION: &str = "2014-10-31";

/// The region every request below is signed for. Its published zone list is longer than the
/// three zones one cluster spreads itself over, which is what the option report must carry.
const REGION: &str = "us-west-2";

/// The Availability Zones AWS publishes for [`REGION`].
const REGION_ZONES: [&str; 4] = ["us-west-2a", "us-west-2b", "us-west-2c", "us-west-2d"];

/// The event source types the front door publishes a category map for, in the order it
/// publishes them — which is NOT alphabetical — each with the categories of that source type
/// in their published order.
const EVENT_CATEGORIES: &[(&str, &[&str])] = &[
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
    (
        "blue-green-deployment",
        &["creation", "failure", "deletion", "notification"],
    ),
    (
        "db-shard-group",
        &[
            "creation",
            "deletion",
            "availability",
            "maintenance",
            "failure",
            "notification",
        ],
    ),
    (
        "zero-etl",
        &["creation", "deletion", "failure", "notification"],
    ),
];

/// One published Amazon DocumentDB engine version: the version, its major line, its minor
/// release within that line, its parameter-group family, and the versions it can be upgraded
/// to in place.
struct DocdbVersion {
    version: &'static str,
    major: &'static str,
    minor: &'static str,
    family: &'static str,
    upgrade_targets: &'static [&'static str],
}

/// The engine versions the front door publishes for Amazon DocumentDB, oldest first, with
/// the upgrade path each one has: a later release of its own line, and a release of a later
/// major line from the first line this engine upgrades in place to (5.0) — so a 3.6 row's
/// targets skip the 4.0 one.
const DOCDB_VERSIONS: &[DocdbVersion] = &[
    DocdbVersion {
        version: "3.6.0",
        major: "3.6",
        minor: "0",
        family: "docdb3.6",
        upgrade_targets: &["5.0.0", "5.0.1", "5.0.2", "8.0.0", "8.0.1"],
    },
    DocdbVersion {
        version: "4.0.0",
        major: "4.0",
        minor: "0",
        family: "docdb4.0",
        upgrade_targets: &["4.0.1", "5.0.0", "5.0.1", "5.0.2", "8.0.0", "8.0.1"],
    },
    DocdbVersion {
        version: "4.0.1",
        major: "4.0",
        minor: "1",
        family: "docdb4.0",
        upgrade_targets: &["5.0.0", "5.0.1", "5.0.2", "8.0.0", "8.0.1"],
    },
    DocdbVersion {
        version: "5.0.0",
        major: "5.0",
        minor: "0",
        family: "docdb5.0",
        upgrade_targets: &["5.0.1", "5.0.2", "8.0.0", "8.0.1"],
    },
    DocdbVersion {
        version: "5.0.1",
        major: "5.0",
        minor: "1",
        family: "docdb5.0",
        upgrade_targets: &["5.0.2", "8.0.0", "8.0.1"],
    },
    DocdbVersion {
        version: "5.0.2",
        major: "5.0",
        minor: "2",
        family: "docdb5.0",
        upgrade_targets: &["8.0.0", "8.0.1"],
    },
    DocdbVersion {
        version: "8.0.0",
        major: "8.0",
        minor: "0",
        family: "docdb8.0",
        upgrade_targets: &["8.0.1"],
    },
    DocdbVersion {
        version: "8.0.1",
        major: "8.0",
        minor: "1",
        family: "docdb8.0",
        upgrade_targets: &[],
    },
];

/// The description the front door reports for every version of this engine, and for every
/// upgrade target onto one: the product's own full name.
const DOCDB_DESCRIPTION: &str = "Amazon DocumentDB (with MongoDB compatibility)";

/// The capability flag BOTH views' models declare on an engine VERSION of this engine. A
/// DocumentDB cluster — not its member instance — owns the storage, the readers and the
/// Regions, and the Aurora and Amazon RDS features belong to other engines of the same door,
/// so the one capability a version of this engine publishes is the certificate rotation.
const SHARED_VERSION_CAPABILITIES: &[(&str, bool)] =
    &[("SupportsCertificateRotationWithoutRestart", true)];

/// The engine-version members only the Amazon RDS view's model declares, each with the value
/// that view reports for this engine.
///
/// A view answers the members ITS model declares, so the catalog the views SHARE is the rows
/// and the members both declare: the Amazon DocumentDB model's own `DBEngineVersion` declares
/// none of these, and that view reports none of them rather than inventing one.
const RDS_ONLY_VERSION_MEMBERS: &[(&str, &str)] = &[
    ("SupportsReadReplica", "false"),
    ("SupportsCluster", "false"),
    ("SupportsGlobalDatabases", "false"),
    ("SupportsRDSProxy", "false"),
    ("SupportsParallelQuery", "false"),
    ("SupportsBabelfish", "false"),
    ("SupportsLimitlessDatabase", "false"),
    ("SupportsDataGroups", "false"),
];

/// The capability members only the Amazon RDS view's model declares on an ORDERABLE OPTION
/// of this engine, each with the value that view reports: the instance takes no Multi-AZ
/// standby, no read replica and no Enhanced Monitoring of its own, while the cluster it joins
/// is what makes the class part of a cluster and able to join a global cluster. The Amazon
/// DocumentDB model's own `OrderableDBInstanceOption` declares none of them; see
/// [`RDS_ONLY_VERSION_MEMBERS`].
const RDS_ONLY_OPTION_MEMBERS: &[(&str, bool)] = &[
    ("MultiAZCapable", false),
    ("ReadReplicaCapable", false),
    ("SupportsEnhancedMonitoring", false),
    ("SupportsClusters", true),
    ("SupportsGlobalDatabases", true),
    ("SupportsStorageEncryption", true),
    ("SupportsIops", false),
    ("SupportsStorageThroughput", false),
    ("SupportsHttpEndpoint", false),
    ("SupportsOptimizedReads", false),
    ("SupportsOptimizedWrites", false),
    ("SupportsKerberosAuthentication", false),
    ("SupportsDedicatedLogVolume", false),
    ("SupportsAdditionalStorageVolumes", false),
    ("RequiresCustomProcessorFeatures", false),
    ("OutpostCapable", false),
];

/// The engine modes and network types an option of this engine is offered with, in the order
/// the report publishes them. Both are unnamed `<member>` lists, so they read as one sequence,
/// and both are members only the Amazon RDS view's model declares; see
/// [`RDS_ONLY_VERSION_MEMBERS`].
const OPTION_LIST_MEMBERS: [&str; 3] = ["provisioned", "IPV4", "DUAL"];

/// The storage types an option of this engine is offered with, in the order its own
/// `CreateDBCluster$StorageType` publishes them ("standard | iopt1"): the class is offered
/// once per type, because the type is part of what an order picks.
const OPTION_STORAGE_TYPES: [&str; 2] = ["standard", "iopt1"];

/// POST one awsQuery action to the router under `signing_name`'s routing key and hand back
/// the body.
fn call(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version={API_VERSION}");
    for (name, value) in params {
        body.push_str(&format!("&{name}={value}"));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("host", "localhost")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {signing_name:?} {action}: a catalog read is answered 200, got {status} with \
         {body}"
    );
    body
}

/// Every text carried by `<tag>` elements of `xml`, in document order.
fn texts(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        found.push(after[..end].to_string());
        rest = &after[end + close.len()..];
    }
    found
}

/// The first text carried by a `<tag>` element of `xml`, if any.
fn text(xml: &str, tag: &str) -> Option<String> {
    texts(xml, tag).into_iter().next()
}

/// The `<tag>…</tag>` subtrees of `xml`, in document order. The reports below nest no tag
/// inside another of the same name, so the first closing tag ends the subtree.
fn subtrees(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start..];
        let Some(end) = after.find(&close) else { break };
        found.push(after[..end + close.len()].to_string());
        rest = &after[end + close.len()..];
    }
    found
}

#[test]
fn every_view_of_the_rds_front_door_publishes_one_catalog() {
    std::thread::Builder::new()
        .name("rds-shared-catalogs".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a current-thread runtime");

            // ---------------------------------------------------------------
            // The event-category catalog: one table, published order, unsorted
            // ---------------------------------------------------------------
            let published: Vec<String> = EVENT_CATEGORIES
                .iter()
                .map(|(source_type, _)| (*source_type).to_string())
                .collect();
            let mut alphabetical = published.clone();
            alphabetical.sort_unstable();
            // The Amazon Neptune view's own `DescribeEventCategories` is still scaffolded
            // (its own stub issue), so the two views that answer it are the ones compared.
            for key in ["rds", "docdb"] {
                let body = call(&router, &runtime, key, "DescribeEventCategories", &[]);
                let reported = texts(&body, "SourceType");
                assert_eq!(
                    reported, published,
                    "case {key:?}: every view publishes the front door's source types in its \
                     own order, got {reported:?}"
                );
                assert_ne!(
                    reported, alphabetical,
                    "case {key:?}: the published sequence is the catalog's own, not a sort, \
                     got {reported:?}"
                );
                // Each map's categories keep the published order too, which is what a client
                // that renders or diffs the table positionally reads.
                for (source_type, categories) in EVENT_CATEGORIES {
                    let one = call(
                        &router,
                        &runtime,
                        key,
                        "DescribeEventCategories",
                        &[("SourceType", source_type)],
                    );
                    let expected: Vec<String> = categories
                        .iter()
                        .map(|category| (*category).to_string())
                        .collect();
                    let got = (texts(&one, "SourceType"), texts(&one, "EventCategory"));
                    assert_eq!(
                        got,
                        (vec![(*source_type).to_string()], expected),
                        "case {key:?} {source_type:?}: expected that source type's published \
                         categories in their published order, got {got:?}"
                    );
                }
            }

            // ---------------------------------------------------------------
            // The engine-version catalog of the `docdb` engine
            // ---------------------------------------------------------------
            let published_versions: Vec<String> = DOCDB_VERSIONS
                .iter()
                .map(|row| row.version.to_string())
                .collect();
            for key in ["rds", "docdb"] {
                // The Amazon RDS view reports every engine of the door, so its answer is
                // narrowed to this engine and read with the deprecated rows included; the
                // DocumentDB view publishes this engine only.
                let body = match key {
                    "rds" => call(
                        &router,
                        &runtime,
                        key,
                        "DescribeDBEngineVersions",
                        &[("Engine", "docdb"), ("IncludeAll", "true")],
                    ),
                    _ => call(
                        &router,
                        &runtime,
                        key,
                        "DescribeDBEngineVersions",
                        &[("Engine", "docdb")],
                    ),
                };
                let rows = subtrees(&body, "DBEngineVersion");
                let reported: Vec<String> = rows
                    .iter()
                    .filter_map(|row| text(row, "EngineVersion"))
                    .collect();
                assert_eq!(
                    reported, published_versions,
                    "case {key:?}: every view reports the engine's published versions, oldest \
                     first, got {reported:?}"
                );
                for (row, expected) in rows.iter().zip(DOCDB_VERSIONS) {
                    let case = format!("{key} {}", expected.version);
                    let rds_only = key == "rds";
                    let got = (
                        text(row, "Engine"),
                        text(row, "DBParameterGroupFamily"),
                        text(row, "DBEngineVersionDescription"),
                    );
                    assert_eq!(
                        got,
                        (
                            Some("docdb".to_string()),
                            Some(expected.family.to_string()),
                            Some(DOCDB_DESCRIPTION.to_string()),
                        ),
                        "case {case:?}: expected the published family and description, got \
                         {got:?}"
                    );
                    // The version split is a member only the Amazon RDS view's model
                    // declares; see `RDS_ONLY_VERSION_MEMBERS`.
                    for (member, value) in [
                        ("MajorEngineVersion", expected.major),
                        ("MinorEngineVersion", expected.minor),
                    ] {
                        let got = text(row, member);
                        let want = rds_only.then(|| value.to_string());
                        assert_eq!(
                            got, want,
                            "case {case:?}: expected {member} {want:?}, got {got:?}"
                        );
                    }
                    for (member, flag) in SHARED_VERSION_CAPABILITIES {
                        let got = text(row, member);
                        assert_eq!(
                            got,
                            Some(flag.to_string()),
                            "case {case:?}: expected {member} {flag}, got {got:?}"
                        );
                    }
                    for (member, value) in RDS_ONLY_VERSION_MEMBERS {
                        let got = text(row, member);
                        let want = rds_only.then(|| (*value).to_string());
                        assert_eq!(
                            got, want,
                            "case {case:?}: expected {member} {want:?}, got {got:?}"
                        );
                    }
                    let targets = subtrees(row, "UpgradeTarget");
                    let named: Vec<String> = targets
                        .iter()
                        .filter_map(|target| text(target, "EngineVersion"))
                        .collect();
                    assert_eq!(
                        named,
                        expected
                            .upgrade_targets
                            .iter()
                            .map(|target| (*target).to_string())
                            .collect::<Vec<_>>(),
                        "case {case:?}: expected the published upgrade targets, got {named:?}"
                    );
                    for target in &targets {
                        let version = text(target, "EngineVersion").unwrap_or_default();
                        let crosses_a_line = !version.starts_with(&format!("{}.", expected.major));
                        let got = (
                            text(target, "IsMajorVersionUpgrade"),
                            text(target, "Description"),
                        );
                        assert_eq!(
                            got,
                            (
                                Some(crosses_a_line.to_string()),
                                Some(DOCDB_DESCRIPTION.to_string()),
                            ),
                            "case {case:?} -> {version}: a target that leaves the version's \
                             line is a major-version upgrade, got {got:?}"
                        );
                        // Whether a snapshot can be upgraded straight to the target is a
                        // member only the Amazon RDS view's model declares; see
                        // `RDS_ONLY_VERSION_MEMBERS`.
                        let got = text(target, "SupportsSnapshotUpgrade");
                        let want = rds_only.then(|| crosses_a_line.to_string());
                        assert_eq!(
                            got, want,
                            "case {case:?} -> {version}: expected SupportsSnapshotUpgrade \
                             {want:?}, got {got:?}"
                        );
                    }
                }
            }

            // ---------------------------------------------------------------
            // The orderable-option catalog of the `docdb` engine
            // ---------------------------------------------------------------
            for key in ["rds", "docdb"] {
                let body = call(
                    &router,
                    &runtime,
                    key,
                    "DescribeOrderableDBInstanceOptions",
                    &[
                        ("Engine", "docdb"),
                        ("EngineVersion", "5.0.0"),
                        ("DBInstanceClass", "db.r5.large"),
                    ],
                );
                let options = subtrees(&body, "OrderableDBInstanceOption");
                let storage_types: Vec<String> = options
                    .iter()
                    .filter_map(|option| text(option, "StorageType"))
                    .collect();
                assert_eq!(
                    storage_types,
                    OPTION_STORAGE_TYPES
                        .iter()
                        .map(|storage_type| (*storage_type).to_string())
                        .collect::<Vec<_>>(),
                    "case {key:?}: one option per (version, class, storage type) this engine \
                     offers, got {body}"
                );
                for option in &options {
                    let zones = texts(option, "Name");
                    assert_eq!(
                        zones,
                        REGION_ZONES
                            .iter()
                            .map(|zone| (*zone).to_string())
                            .collect::<Vec<_>>(),
                        "case {key:?}: an option is offered in the Region's published zones, \
                         got {zones:?}"
                    );
                    for (member, flag) in RDS_ONLY_OPTION_MEMBERS {
                        let got = text(option, member);
                        let want = (key == "rds").then(|| flag.to_string());
                        assert_eq!(
                            got, want,
                            "case {key:?}: expected {member} {want:?}, got {got:?}"
                        );
                    }
                    let list_members = texts(option, "member");
                    let want: Vec<String> = match key {
                        "rds" => OPTION_LIST_MEMBERS
                            .iter()
                            .map(|value| (*value).to_string())
                            .collect(),
                        _ => Vec::new(),
                    };
                    assert_eq!(
                        list_members, want,
                        "case {key:?}: expected {want:?}, got {list_members:?}"
                    );
                }
            }
        })
        .expect("the catalog thread starts")
        .join()
        .expect("the catalog thread finishes");
}
