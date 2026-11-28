// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two things the GRAPH engine's control plane publishes about
//! itself that this one `rds` front door answers for it: the DB instance classes a member
//! of one of its clusters can be ordered in, and the RELEASE a published engine version
//! names.
//!
//! This front door serves three control planes (`src/service.rs`): Amazon RDS, Amazon
//! DocumentDB and Amazon Neptune publish one wire identity, so a graph client's catalog
//! reads land here. The rules these tables pin:
//!
//! 1. the classes an option is offered in are the ones the engine's OWN control plane
//!    publishes — so a class that control plane publishes is reported as orderable, and a
//!    class it does not publish is reported in no option, rather than this door's RDS-wide
//!    cluster class list standing in for the engine's;
//! 2. a published version of that engine is described by the RELEASE of it the service
//!    runs — the version plus the build the engine publishes for it — which is the one
//!    release both this control plane and the engine's own status report name, read from
//!    the shared table of published releases rather than restated here.
//!
//! Both rules are asserted over tables: the classes span every family the engine publishes
//! (including sizes and families the issue that prompted them cited nothing about) and the
//! releases span every published major-version line, with a version whose published build
//! is NOT its first beside versions whose build is.
//!
//! The harness (`emulator`, `call`, `expect_ok` and the element readers) is the
//! [`crate::db_instances`] suite's, and the option reader is
//! [`crate::orderable_db_instance_options`]'s, reused rather than copied.

use std::collections::{BTreeMap, BTreeSet};

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_ok, subtrees, text, texts, REGION};
use crate::neptune_engine_catalog::{scalars_of, LICENSE_MODEL, STORAGE_TYPES};
use crate::orderable_db_instance_options::options;

/// The engine these tables are about: the graph engine of the Amazon Neptune control plane
/// this front door serves.
const ENGINE: &str = "neptune";

/// One DB instance class row: (case, the class).
///
/// The rows span what the rule generalises over: the burstable classes and the serverless
/// capacity class, the Intel and Graviton memory-optimised families, the local-NVMe (`d`)
/// line, and the smallest and largest size of the lines whose sizes run past the ones this
/// door's RDS-wide cluster list carries (`db.r5.24xlarge`, `db.r6i.32xlarge`,
/// `db.x2iedn.32xlarge`). Only `db.r5.12xlarge` was cited by the issue that prompted them.
const PUBLISHED_CLASSES: &[(&str, &str)] = &[
    ("the burstable class", "db.t3.medium"),
    ("the Graviton burstable class", "db.t4g.medium"),
    ("the serverless capacity class", "db.serverless"),
    ("the smallest memory-optimised class", "db.r5.large"),
    ("the class the issue cited", "db.r5.12xlarge"),
    ("the largest size of the r5 line", "db.r5.24xlarge"),
    ("a local-NVMe memory-optimised class", "db.r5d.4xlarge"),
    ("a Graviton memory-optimised class", "db.r6g.8xlarge"),
    ("the largest size of the r6i line", "db.r6i.32xlarge"),
    ("a later Graviton memory-optimised class", "db.r7g.2xlarge"),
    ("the smallest x2g class", "db.x2g.large"),
    ("the largest size of the x2iedn line", "db.x2iedn.32xlarge"),
];

/// Classes this engine's control plane publishes NO option in: the general-purpose and
/// compute families, which only an engine that owns its own storage is ordered in, and a
/// burstable size below the one a cluster member takes.
const CLASSES_IT_PUBLISHES_NO_OPTION_IN: &[(&str, &str)] = &[
    ("a general-purpose family", "db.m5.large"),
    ("a compute family", "db.c6gd.4xlarge"),
    ("a burstable size below a cluster member's", "db.t3.micro"),
];

/// One published release row: (case, the engine version, the release the engine publishes
/// for it).
///
/// The rows span every published major-version line and both halves of the release rule: a
/// version whose published build is a LATER one (its release was patched after it shipped)
/// and versions whose build is the first. `1.2.1.2`, `1.3.4.0` and `1.4.8.0` are versions
/// the issue cited nothing about.
const PUBLISHED_RELEASES: &[(&str, &str, &str)] = &[
    ("a version patched after it shipped", "1.1.1.0", "1.1.1.0.R7"),
    ("the second release of the first line", "1.1.2.0", "1.1.2.0.R1"),
    ("the first release of the second line", "1.2.0.0", "1.2.0.0.R4"),
    ("a later release of the second line", "1.2.1.2", "1.2.1.2.R1"),
    ("the last release of the third line", "1.3.4.0", "1.3.4.0.R1"),
    ("the engine's default release", "1.4.8.0", "1.4.8.0.R1"),
];

/// The engine versions `DescribeDBEngineVersions` reports as available for [`ENGINE`].
async fn available_versions(router: &MultiServiceRouter) -> BTreeSet<String> {
    let body = expect_ok(
        "the published versions",
        call(
            router,
            "DescribeDBEngineVersions",
            &[("Engine", ENGINE), ("MaxRecords", "100")],
        )
        .await,
    );
    subtrees(&body, "DBEngineVersion")
        .iter()
        .map(|entry| text(&scalars_of(entry), "EngineVersion"))
        .collect()
}

/// One engine-version entry's own members are read through the sibling suite's one reader,
/// [`crate::neptune_engine_catalog::scalars_of`], imported above.

#[tokio::test]
async fn every_class_the_graph_engine_publishes_is_orderable_for_every_published_version() {
    let router = emulator();
    let available = available_versions(&router).await;
    assert!(
        !available.is_empty(),
        "the engine must publish at least one available version to order"
    );
    let zones = ::service_common::aws_availability_zones::availability_zones(REGION);

    for (case, class) in PUBLISHED_CLASSES {
        let body = expect_ok(
            case,
            call(
                &router,
                "DescribeOrderableDBInstanceOptions",
                &[
                    ("Engine", ENGINE.to_string()),
                    ("DBInstanceClass", (*class).to_string()),
                    ("MaxRecords", "1000".to_string()),
                ],
            )
            .await,
        );
        let offered = options(&body);
        assert!(
            !offered.is_empty(),
            "case {case:?}: {class} is a class this engine's control plane publishes, so it \
             must be reported as orderable"
        );

        let mut by_version: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for option in &offered {
            assert_eq!(
                option.engine, ENGINE,
                "case {case:?}: every option is of the engine asked for, got {}",
                option.xml
            );
            assert_eq!(
                option.db_instance_class, *class,
                "case {case:?}: DBInstanceClass narrows the report, got {}",
                option.xml
            );
            assert_eq!(
                option.license_model, LICENSE_MODEL,
                "case {case:?}: expected this engine's own licence model, got {}",
                option.xml
            );
            assert_eq!(
                texts(&option.xml, "Name"),
                zones,
                "case {case:?}: an option is offered in the Region's own zones, got {}",
                option.xml
            );
            // The capability member a class could otherwise be read as changing: the member
            // that says an instance of this engine is only ever a cluster member is reported
            // for every class.
            assert_eq!(
                text(&option.xml, "SupportsClusters"),
                "true",
                "case {case:?}: an instance of this engine is ordered as a member of its \
                 cluster, got {}",
                option.xml
            );
            by_version
                .entry(option.engine_version.clone())
                .or_default()
                .push(option.storage_type.clone());
        }

        let offered_versions: BTreeSet<String> = by_version.keys().cloned().collect();
        assert_eq!(
            offered_versions, available,
            "case {case:?}: a class is orderable for every version this engine publishes as \
             available"
        );
        for (version, storage_types) in &by_version {
            assert_eq!(
                storage_types.as_slice(),
                STORAGE_TYPES.as_slice(),
                "case {case:?}: {version} must be offered once per storage type that engine's \
                 cluster publishes, got {storage_types:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_class_the_graph_engine_publishes_no_option_in_is_reported_for_nothing() {
    let router = emulator();

    for (case, class) in CLASSES_IT_PUBLISHES_NO_OPTION_IN {
        let body = expect_ok(
            case,
            call(
                &router,
                "DescribeOrderableDBInstanceOptions",
                &[
                    ("Engine", ENGINE.to_string()),
                    ("DBInstanceClass", (*class).to_string()),
                    ("MaxRecords", "1000".to_string()),
                ],
            )
            .await,
        );
        let offered = options(&body);
        assert!(
            offered.is_empty(),
            "case {case:?}: {class} is no class this engine's control plane publishes, so it \
             must be offered in no option, got {} of them",
            offered.len()
        );
    }
}

#[tokio::test]
async fn a_published_version_is_described_by_the_release_the_engine_publishes_for_it() {
    let router = emulator();

    for (case, version, release) in PUBLISHED_RELEASES {
        // The release the shared table of this engine's published releases holds, which is
        // also what the engine's own status report names for a cluster running that version:
        // the expectation is read from that table as well as written out, so the two planes
        // cannot drift apart without this failing.
        let published = ::service_common::neptune_engine_releases::release(version);
        assert_eq!(
            published, *release,
            "case {case:?}: the shared release table must publish {release} for {version}, got \
             {published}"
        );

        let body = expect_ok(
            case,
            call(
                &router,
                "DescribeDBEngineVersions",
                &[("Engine", ENGINE), ("EngineVersion", version)],
            )
            .await,
        );
        let entries = subtrees(&body, "DBEngineVersion");
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: expected exactly the version named, got {body}"
        );
        let scalars = scalars_of(&entries[0]);
        assert_eq!(
            text(&scalars, "DBEngineVersionDescription"),
            format!("Neptune {release}"),
            "case {case:?}: a version is described by the release of it the engine runs, got \
             {scalars}"
        );
    }
}

#[tokio::test]
async fn an_upgrade_target_names_the_release_its_own_entry_names() {
    let router = emulator();

    // The whole catalog of this engine, so an upgrade target's description is checked
    // against the entry for that target version rather than against a copy of the rule.
    let body = expect_ok(
        "the whole catalog of this engine",
        call(
            &router,
            "DescribeDBEngineVersions",
            &[("Engine", ENGINE), ("MaxRecords", "100")],
        )
        .await,
    );
    let entries = subtrees(&body, "DBEngineVersion");
    let described: BTreeMap<String, String> = entries
        .iter()
        .map(|entry| {
            let scalars = scalars_of(entry);
            (
                text(&scalars, "EngineVersion"),
                text(&scalars, "DBEngineVersionDescription"),
            )
        })
        .collect();
    assert!(
        described.len() > 1,
        "expected this engine's release line, got {body}"
    );

    let mut checked = 0_usize;
    for entry in &entries {
        let version = text(&scalars_of(entry), "EngineVersion");
        for target in subtrees(entry, "UpgradeTarget") {
            let target_version = text(&target, "EngineVersion");
            let expected = described.get(&target_version).unwrap_or_else(|| {
                panic!(
                    "case {version:?}: an upgrade target must be a version this report \
                     publishes, got {target}"
                )
            });
            assert_eq!(
                &text(&target, "Description"),
                expected,
                "case {version:?}: the target {target_version} must be described the way its \
                 own entry describes it, got {target}"
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "expected at least one upgrade target in this engine's release line, got {body}"
    );
}
